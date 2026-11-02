"""Stockfish process management (UCI via python-chess)."""

from __future__ import annotations

import logging
import os
import shutil
import threading
import time
from contextlib import contextmanager
from dataclasses import dataclass
from pathlib import Path

import chess
import chess.engine

from ..config import DATA_DIR
from .winprob import MATE_CP

log = logging.getLogger(__name__)


class EngineNotFound(RuntimeError):
    pass


class AnalysisCancelled(RuntimeError):
    """An interactive request was abandoned before its result was needed."""


@contextmanager
def analysis_lock(lock, cancel: threading.Event | None):
    if cancel is None:
        with lock:
            yield
        return
    while not cancel.is_set():
        if lock.acquire(timeout=0.025):
            break
    else:
        raise AnalysisCancelled()
    try:
        if cancel.is_set():
            raise AnalysisCancelled()
        yield
    finally:
        lock.release()


def locate_stockfish() -> Path:
    """Find Stockfish: $PLYBACK_STOCKFISH, data/engine/stockfish.path, data/engine/stockfish[.exe], PATH."""
    env = os.environ.get("PLYBACK_STOCKFISH") or os.environ.get("STOCKFISH_PATH")
    candidates: list[Path] = []
    if env:
        candidates.append(Path(env))
    path_file = DATA_DIR / "engine" / "stockfish.path"
    if path_file.exists():
        candidates.append(Path(path_file.read_text().strip()))
    candidates += [DATA_DIR / "engine" / "stockfish.exe", DATA_DIR / "engine" / "stockfish"]
    which = shutil.which("stockfish")
    if which:
        candidates.append(Path(which))
    for c in candidates:
        if c.is_file():
            return c.resolve()
    raise EngineNotFound("Stockfish not found. Run `python scripts/install_stockfish.py` or set PLYBACK_STOCKFISH.")


@dataclass
class Line:
    cp: int  # White POV, mate-encoded (see winprob)
    pv: list[str]  # UCI moves
    depth: int

    def to_dict(self) -> dict:
        return {"cp": self.cp, "pv": self.pv, "depth": self.depth}

    @staticmethod
    def from_dict(d: dict) -> Line:
        return Line(d["cp"], list(d["pv"]), d.get("depth", 0))


def terminal_eval(board: chess.Board) -> int | None:
    """Evaluation for positions where the game is over, else None."""
    if board.is_checkmate():
        return -MATE_CP if board.turn == chess.WHITE else MATE_CP
    if (
        board.is_stalemate()
        or board.is_insufficient_material()
        or board.is_fivefold_repetition()
        or board.is_seventyfive_moves()
    ):
        return 0
    return None


class Engine:
    """A single Stockfish process. Thread-safe (calls are serialised)."""

    def __init__(self, path: Path | None = None, threads: int = 1, hash_mb: int = 64):
        self.path = Path(path) if path else locate_stockfish()
        self.threads = threads
        self.hash_mb = hash_mb
        self._lock = threading.Lock()
        self._engine: chess.engine.SimpleEngine | None = None
        self.name = "Stockfish"
        self._start()

    def _start(self) -> None:
        self._engine = chess.engine.SimpleEngine.popen_uci(str(self.path), timeout=30)
        opts = {"Threads": self.threads, "Hash": self.hash_mb}
        available = self._engine.options
        self._engine.configure({k: v for k, v in opts.items() if k in available})
        self.name = self._engine.id.get("name", "Stockfish")

    def restart(self) -> None:
        try:
            if self._engine:
                self._engine.quit()
        except Exception:
            log.debug("Engine did not quit cleanly", exc_info=True)
        self._start()

    def close(self) -> None:
        with self._lock:
            if self._engine:
                try:
                    self._engine.quit()
                except Exception:
                    try:
                        self._engine.close()
                    except Exception:
                        log.debug("Engine did not close cleanly", exc_info=True)
                self._engine = None

    def analyse(
        self, board: chess.Board, depth: int, multipv: int = 1, time_cap: float | None = None, nodes: int | None = None,
        cancel: threading.Event | None = None,
    ) -> list[Line]:
        if cancel is not None and cancel.is_set():
            raise AnalysisCancelled()
        term = terminal_eval(board)
        if term is not None:
            return [Line(term, [], 0)]
        limit = chess.engine.Limit(depth=depth, time=time_cap, nodes=nodes)
        n_legal = board.legal_moves.count()
        mpv = max(1, min(multipv, n_legal))
        with analysis_lock(self._lock, cancel):
            for attempt in range(2):
                try:
                    assert self._engine is not None
                    if cancel is None:
                        infos = self._engine.analyse(board, limit, multipv=mpv)
                    else:
                        infos = self._analyse_cancellable(board, limit, mpv, cancel)
                    break
                except (chess.engine.EngineTerminatedError, chess.engine.EngineError, AssertionError, TimeoutError) as exc:
                    if cancel is not None and cancel.is_set():
                        raise AnalysisCancelled() from exc
                    log.warning("Engine failure (%s); restarting (attempt %d)", exc, attempt + 1)
                    self.restart()
            else:
                raise RuntimeError("Engine failed repeatedly")
        lines = []
        for info in infos:
            score = info.get("score")
            pv = info.get("pv") or []
            if score is None:
                continue
            lines.append(Line(score.white().score(mate_score=MATE_CP), [m.uci() for m in pv], info.get("depth", 0)))
        if not lines:  # extremely rare: engine returned no score
            raise RuntimeError("Engine returned no evaluation")
        return lines

    def _analyse_cancellable(self, board, limit, multipv, cancel):
        assert self._engine is not None
        analysis = self._engine.analysis(board, limit, multipv=multipv)
        deadline = time.monotonic() + (limit.time or 0) + 30
        stopped = False
        try:
            while True:
                if cancel.is_set() and not stopped:
                    analysis.stop()
                    stopped = True
                    deadline = min(deadline, time.monotonic() + 1)
                if time.monotonic() >= deadline:
                    self._engine.close()
                    self._engine = None
                    if cancel.is_set():
                        raise AnalysisCancelled()
                    raise TimeoutError("Interactive engine did not finish its search")
                if not analysis.would_block():
                    if analysis.next() is None:
                        break
                elif stopped:
                    time.sleep(0.01)
                else:
                    cancel.wait(0.025)
            analysis.wait()
            if cancel.is_set():
                raise AnalysisCancelled()
            return analysis.multipv
        finally:
            if self._engine is not None:
                try:
                    analysis.stop()
                except chess.engine.EngineTerminatedError:
                    log.debug("Engine exited while stopping interactive analysis")
