"""A dedicated engine for interactive exploration on the board (kept separate from the
background pool so it always responds quickly)."""

from __future__ import annotations

import threading

import chess

from ..analysis.analyzer import pv_to_san
from ..analysis.engine import AnalysisCancelled, Engine, Line, analysis_lock, terminal_eval
from ..analysis.winprob import format_eval, win_pct_white


class InteractiveEngine:
    def __init__(self, engine_factory=None):
        self._factory = engine_factory or (lambda: Engine(threads=2, hash_mb=128))
        self._engine: Engine | None = None
        self._lock = threading.Lock()

    def analyse(
        self, fen: str, depth: int, multipv: int = 3, time_cap: float = 4.0, cancel: threading.Event | None = None
    ) -> dict:
        board = chess.Board(fen)
        if not board.is_valid():
            raise ValueError("Illegal position")
        if cancel is not None and cancel.is_set():
            raise AnalysisCancelled()
        term = terminal_eval(board)
        if term is not None:
            lines = [Line(term, [], 0)]
        else:
            with analysis_lock(self._lock, cancel):
                if self._engine is None:
                    self._engine = self._factory()
                lines = self._engine.analyse(board, depth, multipv, time_cap=time_cap, cancel=cancel)
        return {
            "fen": fen,
            "lines": [
                {
                    "cp": ln.cp,
                    "eval": format_eval(ln.cp),
                    "win_white": round(win_pct_white(ln.cp), 1),
                    "uci": ln.pv[:12],
                    "san": pv_to_san(board, ln.pv, 12),
                    "depth": ln.depth,
                }
                for ln in lines
            ],
        }

    def close(self) -> None:
        with self._lock:
            if self._engine:
                self._engine.close()
                self._engine = None
