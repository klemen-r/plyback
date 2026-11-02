"""On-demand account analysis reports.

Creating a report queues every unanalysed game in its scope (reusing analyses that
already exist), tracks progress, and stores an immutable snapshot of the insights
once the scope is fully analysed (or when the user asks to finalise early).
"""

from __future__ import annotations

import logging
import threading
import time

from ..db import Database, dumps, loads
from ..insights.compute import Filters, compute_insights
from .analysis import PRIORITY_REPORT, AnalysisService

log = logging.getLogger(__name__)


def scope_game_ids(db: Database, account_id: int, f: Filters) -> list[int]:
    where = ["ag.account_id = ?", "g.rules IN ('chess','chess960')"]
    params: list = [account_id]
    if f.since is not None:
        where.append("g.end_time >= ?")
        params.append(f.since)
    if f.until is not None:
        where.append("g.end_time < ?")
        params.append(f.until)
    if f.time_classes:
        where.append(f"g.time_class IN ({','.join('?' for _ in f.time_classes)})")
        params += f.time_classes
    if f.color:
        where.append("ag.color = ?")
        params.append(f.color)
    if f.rated is not None:
        where.append("g.rated = ?")
        params.append(1 if f.rated else 0)
    if f.opening:
        where.append("g.opening_family = ?")
        params.append(f.opening)
    if f.opponent:
        where.append("(LOWER(g.white_username) = LOWER(?) OR LOWER(g.black_username) = LOWER(?))")
        params += [f.opponent, f.opponent]
    return [
        r["id"]
        for r in db.query(
            f"SELECT g.id FROM account_games ag JOIN games g ON g.id = ag.game_id WHERE {' AND '.join(where)}", params
        )
    ]


class ReportService:
    def __init__(self, db: Database, analysis: AnalysisService):
        self.db = db
        self.analysis = analysis
        self._stop = threading.Event()
        self._wake = threading.Event()
        self._thread: threading.Thread | None = None
        self._build_lock = threading.Lock()

    def start(self) -> None:
        self._thread = threading.Thread(target=self._loop, name="reports", daemon=True)
        self._thread.start()

    def stop(self) -> None:
        self._stop.set()
        self._wake.set()
        if self._thread:
            self._thread.join()

    def create(self, account_id: int, filters: dict, title: str | None = None) -> int:
        f = Filters.from_dict(filters)
        ids = scope_game_ids(self.db, account_id, f)
        self.analysis.enqueue(ids, PRIORITY_REPORT)
        with self.db.tx() as c:
            cur = c.execute(
                """INSERT INTO reports (account_id, title, filters_json, status, total_games, created_at)
                               VALUES (?,?,?,?,?,?)""",
                (account_id, title, dumps(f.to_dict()), "analyzing", len(ids), int(time.time())),
            )
            rid = cur.lastrowid
        self._wake.set()
        return rid

    def finalize(self, report_id: int) -> None:
        """Build the snapshot now with whatever is analysed."""
        self._build(report_id)

    def _progress(self, rep: dict) -> tuple[int, int, int]:
        f = Filters.from_dict(loads(rep["filters_json"], {}))
        ids = scope_game_ids(self.db, rep["account_id"], f)
        if not ids:
            return 0, 0, 0
        analyzed = pending = 0
        for i in range(0, len(ids), 500):
            chunk = ids[i : i + 500]
            q = ",".join("?" for _ in chunk)
            analyzed += self.db.scalar(f"SELECT COUNT(*) FROM game_analysis WHERE game_id IN ({q})", chunk) or 0
            pending += (
                self.db.scalar(f"SELECT COUNT(*) FROM jobs WHERE status IN ('queued','running') AND game_id IN ({q})", chunk)
                or 0
            )
        return len(ids), analyzed, pending

    def _build(self, report_id: int) -> None:
        with self._build_lock:
            self._build_snapshot(report_id)

    def _build_snapshot(self, report_id: int) -> None:
        rep = self.db.one("SELECT * FROM reports WHERE id=?", (report_id,))
        if rep is None or rep["status"] == "ready":
            return
        try:
            data = compute_insights(self.db, rep["account_id"], loads(rep["filters_json"], {}))
            data["data_quality"].pop("unanalyzed_ids", None)  # snapshot doesn't need the id list
            total, analyzed, _ = self._progress(rep)
            with self.db.tx() as c:
                c.execute(
                    """UPDATE reports SET status='ready', completed_at=?, report_json=?, total_games=?,
                             analyzed_games=?, error=NULL WHERE id=?""",
                    (int(time.time()), dumps(data), total, analyzed, report_id),
                )
            self.db.log_event(
                "info", "reports", f"Report #{report_id} is ready ({analyzed}/{total} games analysed)", rep["account_id"]
            )
        except Exception as exc:
            log.exception("Report %d failed", report_id)
            self.db.execute("UPDATE reports SET status='failed', error=? WHERE id=?", (str(exc), report_id))

    def tick(self) -> None:
        for rep in self.db.query("SELECT * FROM reports WHERE status='analyzing'"):
            total, analyzed, pending = self._progress(rep)
            self.db.execute(
                "UPDATE reports SET total_games=?, analyzed_games=? WHERE id=? AND status='analyzing'",
                (total, analyzed, rep["id"]),
            )
            if pending == 0:
                self._build(rep["id"])

    def _loop(self) -> None:
        try:
            self._run_loop()
        finally:
            self.db.close_thread()

    def _run_loop(self) -> None:
        while not self._stop.is_set():
            try:
                self.tick()
            except Exception:
                log.exception("Report loop error")
            self._wake.wait(4)
            self._wake.clear()
