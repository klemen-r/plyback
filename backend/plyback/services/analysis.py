"""Persistent, resumable, parallel analysis queue.

Jobs live in the ``jobs`` table so nothing is lost on restart: running jobs are put
back in the queue at start-up, and each job checkpoints its evaluated positions
(``partial_json``) so an interrupted game resumes where it stopped.  Opening positions
are additionally cached across games in ``position_cache``.
"""

from __future__ import annotations

import hashlib
import logging
import threading
import time
from collections.abc import Iterable

import chess

from ..analysis.analyzer import AnalysisOptions, analyze_parsed
from ..analysis.engine import Engine, EngineNotFound, Line, locate_stockfish
from ..analysis.pgn import PGNError, parse_pgn
from ..config import ANALYSIS_VERSION
from ..db import Database, dumps, loads
from . import store

log = logging.getLogger(__name__)

PRIORITY_MANUAL = 100
PRIORITY_NEW = 50
PRIORITY_REPORT = 30
PRIORITY_HISTORY = 10
PRIORITY_OUTDATED = 5
CACHE_MAX_PLY = 30
MAX_ATTEMPTS = 3


def _position_key(board: chess.Board, multipv: int, engine_name: str, time_cap: float) -> str:
    # Repetition and the fifty/seventy-five move rules depend on more than EPD.
    # Include the root and move history, and isolate engines and search budgets.
    history = board.root().fen() + "|" + " ".join(move.uci() for move in board.move_stack)
    digest = hashlib.sha256(history.encode()).hexdigest()
    return f"v2|{board.fen()}|{int(board.chess960)}|{multipv}|{engine_name}|{time_cap}|{digest}"


class Interrupted(BaseException):
    """Raised inside a job when its worker is asked to stop. A BaseException so that
    best-effort `except Exception` blocks in the analyser can't swallow it."""


class AnalysisService:
    def __init__(self, db: Database, engine_factory=None):
        self.db = db
        self._uses_stockfish = engine_factory is None
        self._engine_factory = engine_factory or (lambda threads, hash_mb: Engine(threads=threads, hash_mb=hash_mb))
        self._stop = threading.Event()
        self._wake = threading.Event()
        self._workers: dict[int, threading.Thread] = {}
        self._worker_stop: dict[int, threading.Event] = {}
        self._next_worker_id = 0
        self._lock = threading.Lock()
        self.paused = False
        self.progress: dict[int, dict] = {}  # job_id -> {done,total,game_id,worker}
        self.engine_name: str | None = None
        self.engine_error: str | None = None
        self._engine_config: tuple[int, int] | None = None
        self.completed_session = 0

    # --- lifecycle --------------------------------------------------------------------
    def start(self) -> None:
        with self.db.tx() as c:
            c.execute("UPDATE jobs SET status='queued' WHERE status='running'")
        if self._uses_stockfish:
            try:
                locate_stockfish()
                self.engine_error = None
            except EngineNotFound as exc:
                self.engine_error = str(exc)
                log.error("%s", exc)
        self.apply_settings()
        outdated = [
            r["game_id"] for r in self.db.query("SELECT game_id FROM game_analysis WHERE version < ?", (ANALYSIS_VERSION,))
        ]
        if outdated:
            log.info("Re-queueing %d analyses made with an older algorithm version", len(outdated))
            self.enqueue(outdated, PRIORITY_OUTDATED)

    def stop(self) -> None:
        self._stop.set()
        self._wake.set()
        with self._lock:
            for ev in self._worker_stop.values():
                ev.set()
            workers = list(self._workers.values())
        # Retiring workers still own engines and database connections. Wait for
        # their current bounded engine call and checkpoint before closing storage.
        for t in workers:
            t.join()
        with self._lock:
            self._workers.clear()
            self._worker_stop.clear()

    def apply_settings(self) -> None:
        """(Re)size the worker pool to match settings; restart engines if their config changed."""
        s = store.get_settings(self.db)
        want = int(s["workers"])
        cfg = (int(s["threads_per_worker"]), int(s["hash_mb"]))
        with self._lock:
            for wid in list(self._workers):
                if not self._workers[wid].is_alive():
                    del self._workers[wid]
                    del self._worker_stop[wid]
            if self._engine_config is not None and cfg != self._engine_config:
                for wid in list(self._workers):
                    self._worker_stop[wid].set()
            self._engine_config = cfg
            active = [wid for wid in self._workers if not self._worker_stop[wid].is_set()]
            while len(active) > want:
                wid = active.pop()
                self._worker_stop[wid].set()
            while len(active) < want and not self._stop.is_set():
                wid = self._next_worker_id
                self._next_worker_id += 1
                ev = threading.Event()
                self._worker_stop[wid] = ev
                t = threading.Thread(target=self._worker_loop, args=(wid, ev), name=f"analysis-{wid}", daemon=True)
                self._workers[wid] = t
                t.start()
                active.append(wid)
        self._wake.set()

    def pause(self) -> None:
        self.paused = True

    def resume(self) -> None:
        self.paused = False
        self._wake.set()

    # --- queue API --------------------------------------------------------------------
    def enqueue(
        self, game_ids: Iterable[int], priority: int = PRIORITY_HISTORY, depth: int | None = None, force: bool = False
    ) -> int:
        """Queue games for analysis. Skips games already analysed at ≥ depth with the current
        algorithm version (unless ``force``). Raises the priority of already-queued jobs."""
        s = store.get_settings(self.db)
        depth = int(depth or s["analysis_depth"])
        ids = list(dict.fromkeys(game_ids))
        if not ids:
            return 0
        added = 0
        now = int(time.time())
        with self.db.tx() as c:
            for gid in ids:
                game = c.execute("SELECT id, end_time, rules FROM games WHERE id=?", (gid,)).fetchone()
                if game is None or game["rules"] not in ("chess", "chess960"):
                    continue
                if not force:
                    ga = c.execute("SELECT depth, version FROM game_analysis WHERE game_id=?", (gid,)).fetchone()
                    if ga and ga["depth"] >= depth and ga["version"] >= ANALYSIS_VERSION:
                        continue
                active = c.execute(
                    "SELECT id, priority, status FROM jobs WHERE game_id=? AND status IN ('queued','running')", (gid,)
                ).fetchone()
                if active:
                    if active["status"] == "queued":
                        c.execute(
                            "UPDATE jobs SET priority=MAX(priority, ?), depth=MAX(COALESCE(depth,0), ?) WHERE id=?",
                            (priority, depth, active["id"]),
                        )
                    continue
                c.execute("DELETE FROM jobs WHERE game_id=? AND status='failed'", (gid,))
                c.execute(
                    """INSERT INTO jobs (game_id, status, priority, depth, created_at, sort_key)
                             VALUES (?, 'queued', ?, ?, ?, ?)""",
                    (gid, priority, depth, now, game["end_time"] or 0),
                )
                added += 1
        if added:
            self._wake.set()
        return added

    def cancel_for_account(self, account_id: int) -> None:
        with self.db.tx() as c:
            c.execute(
                """DELETE FROM jobs WHERE status='queued' AND game_id IN
                         (SELECT game_id FROM account_games WHERE account_id=?)
                         AND game_id NOT IN (SELECT game_id FROM account_games WHERE account_id != ?)""",
                (account_id, account_id),
            )

    def retry_failed(self) -> int:
        with self.db.tx() as c:
            c.execute("""DELETE FROM jobs WHERE status='failed' AND id NOT IN
                         (SELECT MAX(id) FROM jobs WHERE status='failed' GROUP BY game_id)""")
            cur = c.execute("""UPDATE jobs SET status='queued', attempts=0, error=NULL WHERE status='failed'
                               AND game_id NOT IN (SELECT game_id FROM jobs WHERE status IN ('queued','running'))""")
            n = cur.rowcount
        self._wake.set()
        return n

    def clear_queue(self) -> int:
        with self.db.tx() as c:
            return c.execute("DELETE FROM jobs WHERE status='queued'").rowcount

    def stats(self) -> dict:
        rows = self.db.query("SELECT status, COUNT(*) AS n FROM jobs GROUP BY status")
        counts = {r["status"]: r["n"] for r in rows}
        running = []
        for job_id, p in list(self.progress.items()):
            running.append({"job_id": job_id, **p})
        recent = self.db.query("""SELECT AVG(duration_ms) AS avg_ms, COUNT(*) AS n FROM
                                  (SELECT duration_ms FROM game_analysis ORDER BY analyzed_at DESC LIMIT 30)""")
        avg_ms = (recent[0]["avg_ms"] if recent else None) or None
        with self._lock:
            worker_count = sum(t.is_alive() and not self._worker_stop[wid].is_set() for wid, t in self._workers.items())
        workers = max(1, worker_count)
        queued = counts.get("queued", 0)
        return {
            "paused": self.paused,
            "workers": worker_count,
            "engine": self.engine_name,
            "engine_error": self.engine_error,
            "queued": queued,
            "running": counts.get("running", 0),
            "failed": counts.get("failed", 0),
            "done": counts.get("done", 0),
            "active": running,
            "avg_game_seconds": round(avg_ms / 1000, 1) if avg_ms else None,
            "eta_seconds": int(queued * (avg_ms / 1000) / workers) if avg_ms else None,
        }

    # --- workers ----------------------------------------------------------------------
    def _claim(self) -> dict | None:
        with self.db.tx() as c:
            row = c.execute("""SELECT * FROM jobs WHERE status='queued'
                               ORDER BY priority DESC, sort_key DESC, id LIMIT 1""").fetchone()
            if row is None:
                return None
            c.execute(
                "UPDATE jobs SET status='running', started_at=?, attempts=attempts+1 WHERE id=?",
                (int(time.time()), row["id"]),
            )
            return dict(row)

    def _worker_loop(self, wid: int, stop: threading.Event) -> None:
        engine: Engine | None = None
        try:
            while not stop.is_set() and not self._stop.is_set():
                if self.paused or self.engine_error:
                    self._wake.wait(2)
                    self._wake.clear()
                    if self.engine_error and self._uses_stockfish:
                        try:
                            locate_stockfish()
                            self.engine_error = None
                        except EngineNotFound:
                            pass
                    continue
                job = self._claim()
                if job is None:
                    self._wake.wait(3)
                    self._wake.clear()
                    continue
                if engine is None:
                    try:
                        threads, hash_mb = self._engine_config or (1, 64)
                        engine = self._engine_factory(threads, hash_mb)
                        self.engine_name = engine.name
                    except Exception as exc:
                        log.exception("Could not start engine")
                        self.engine_error = f"Could not start Stockfish: {exc}"
                        self._requeue(job["id"], str(exc), count_attempt=False)
                        continue
                self._run_job(job, engine, wid, stop)
        finally:
            try:
                if engine is not None:
                    engine.close()
            finally:
                self.db.close_thread()

    def _requeue(self, job_id: int, error: str, count_attempt: bool = True) -> None:
        with self.db.tx() as c:
            row = c.execute("SELECT attempts FROM jobs WHERE id=?", (job_id,)).fetchone()
            attempts = row["attempts"] if row else MAX_ATTEMPTS
            if not count_attempt:
                c.execute(
                    "UPDATE jobs SET status='queued', attempts=MAX(0, attempts-1), error=? WHERE id=?", (error, job_id)
                )
            elif attempts >= MAX_ATTEMPTS:
                c.execute(
                    "UPDATE jobs SET status='failed', error=?, finished_at=? WHERE id=?", (error, int(time.time()), job_id)
                )
            else:
                c.execute("UPDATE jobs SET status='queued', priority=priority-1, error=? WHERE id=?", (error, job_id))

    def _run_job(self, job: dict, engine: Engine, wid: int, stop: threading.Event) -> None:
        job_id, game_id = job["id"], job["game_id"]
        s = store.get_settings(self.db)
        depth = int(job["depth"] or s["analysis_depth"])
        multipv = int(s["analysis_multipv"])
        time_cap = float(s["max_seconds_per_position"])
        game = self.db.one("SELECT * FROM games WHERE id=?", (game_id,))
        if game is None:
            self.db.execute("DELETE FROM jobs WHERE id=?", (job_id,))
            return
        try:
            parsed = parse_pgn(game["pgn"])
        except PGNError as exc:
            with self.db.tx() as c:
                c.execute(
                    "UPDATE jobs SET status='failed', error=?, finished_at=?, attempts=? WHERE id=?",
                    (f"Unsupported game: {exc}", int(time.time()), MAX_ATTEMPTS, job_id),
                )
            return
        if parsed.plies == 0:
            with self.db.tx() as c:
                c.execute(
                    "UPDATE jobs SET status='failed', error='Game has no moves', finished_at=? WHERE id=?",
                    (int(time.time()), job_id),
                )
            return
        partial: dict[str, dict] = loads(job.get("partial_json"), {}) or {}
        ply_of = {b.epd(): i for i, b in enumerate(parsed.boards)}
        new_since_save = 0
        self.progress[job_id] = {"game_id": game_id, "done": 0, "total": parsed.plies + 1, "worker": wid}

        def evaluate(board: chess.Board, d: int, mpv: int) -> list[Line]:
            nonlocal new_since_save
            if stop.is_set() or self._stop.is_set():
                raise Interrupted()
            key = _position_key(board, mpv, engine.name, time_cap)
            hit = partial.get(key)
            if hit and hit["depth"] >= d:
                return [Line.from_dict(x) for x in hit["lines"]]
            ply = ply_of.get(board.epd(), 999)
            if ply <= CACHE_MAX_PLY:
                row = self.db.one("SELECT depth, data FROM position_cache WHERE key=?", (key,))
                if row and row["depth"] >= d:
                    return [Line.from_dict(x) for x in loads(row["data"], [])]
            lines = engine.analyse(board, d, mpv, time_cap=time_cap)
            data = [ln.to_dict() for ln in lines]
            partial[key] = {"depth": d, "lines": data}
            if ply <= CACHE_MAX_PLY and lines and lines[0].pv:
                with self.db.tx() as c:
                    c.execute(
                        """INSERT INTO position_cache (key, depth, multipv, data, created_at) VALUES (?,?,?,?,?)
                                 ON CONFLICT(key) DO UPDATE SET depth=excluded.depth, data=excluded.data
                                 WHERE excluded.depth > position_cache.depth""",
                        (key, d, mpv, dumps(data), int(time.time())),
                    )
            new_since_save += 1
            if new_since_save >= 10:
                new_since_save = 0
                with self.db.tx() as c:
                    c.execute("UPDATE jobs SET partial_json=? WHERE id=?", (dumps(partial), job_id))
            return lines

        def progress(done: int, total: int) -> None:
            self.progress[job_id] = {"game_id": game_id, "done": done, "total": total, "worker": wid}

        t0 = time.time()
        try:
            result = analyze_parsed(
                parsed, evaluate, AnalysisOptions(depth=depth, multipv=multipv), progress, time_control=game["time_control"]
            )
            if self.db.one("SELECT id FROM games WHERE id=?", (game_id,)) is None:
                return  # game deleted while it was being analysed (job row is gone too)
            store.save_analysis(self.db, game_id, result, depth, multipv, engine.name)
            with self.db.tx() as c:
                c.execute(
                    """UPDATE jobs SET status='done', finished_at=?, partial_json=NULL, error=NULL,
                             progress=total WHERE id=?""",
                    (int(time.time()), job_id),
                )
                c.execute("DELETE FROM jobs WHERE status='done' AND finished_at < ?", (int(time.time()) - 7 * 86400,))
            self.completed_session += 1
            log.info("Analysed game %d (%d plies, depth %d) in %.1fs", game_id, parsed.plies, depth, time.time() - t0)
        except Interrupted:
            with self.db.tx() as c:
                c.execute(
                    "UPDATE jobs SET status='queued', attempts=MAX(0, attempts-1), partial_json=? WHERE id=?",
                    (dumps(partial), job_id),
                )
        except Exception as exc:
            log.exception("Analysis of game %d failed", game_id)
            with self.db.tx() as c:
                c.execute("UPDATE jobs SET partial_json=? WHERE id=?", (dumps(partial), job_id))
            self._requeue(job_id, f"{type(exc).__name__}: {exc}")
            self.db.log_event("error", "analysis", f"Analysis of game {game_id} failed: {exc}")
        finally:
            self.progress.pop(job_id, None)
