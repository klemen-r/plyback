import asyncio
import threading
import time
from types import SimpleNamespace

import chess
import pytest
from fastapi import HTTPException

from plyback.analysis.engine import AnalysisCancelled, Engine
from plyback.api.engine import EngineIn, engine_analyse
from plyback.services.interactive import InteractiveEngine

from .conftest import requires_stockfish


def test_cancelled_request_does_not_start_engine():
    def unexpected_engine():
        raise AssertionError("An abandoned request must not start Stockfish")

    service = InteractiveEngine(unexpected_engine)
    cancel = threading.Event()
    cancel.set()
    with pytest.raises(AnalysisCancelled):
        service.analyse(chess.STARTING_FEN, 20, cancel=cancel)


def test_queued_request_cancels_without_waiting_for_current_search():
    calls = []
    service = InteractiveEngine(lambda: calls.append("engine started"))
    cancel = threading.Event()
    finished = threading.Event()

    def analyse():
        try:
            with pytest.raises(AnalysisCancelled):
                service.analyse(chess.STARTING_FEN, 20, cancel=cancel)
        finally:
            finished.set()

    service._lock.acquire()
    worker = threading.Thread(target=analyse)
    try:
        worker.start()
        cancel.set()
        assert finished.wait(1)
        assert not calls
    finally:
        service._lock.release()
        worker.join(5)


@requires_stockfish
def test_real_search_stops_promptly_and_engine_remains_usable():
    engine = Engine(threads=1, hash_mb=16)
    cancel = threading.Event()
    timer = threading.Timer(0.1, cancel.set)
    try:
        timer.start()
        started = time.monotonic()
        with pytest.raises(AnalysisCancelled):
            engine.analyse(chess.Board(), 32, 3, time_cap=4, cancel=cancel)
        assert time.monotonic() - started < 2
        lines = engine.analyse(chess.Board(), 6, 1, time_cap=0.2)
        assert lines and chess.Move.from_uci(lines[0].pv[0]) in chess.Board().legal_moves
    finally:
        timer.cancel()
        timer.join()
        engine.close()


def test_cancellable_terminal_position_needs_no_search():
    engine = Engine.__new__(Engine)
    board = chess.Board("7k/8/6QK/8/8/8/8/8 b - - 0 1")
    assert board.is_stalemate()
    assert engine.analyse(board, 20, cancel=threading.Event())[0].cp == 0


def test_terminal_interactive_position_needs_no_installed_engine():
    def unexpected_engine():
        raise AssertionError("A terminal position needs no engine process")

    service = InteractiveEngine(unexpected_engine)
    result = service.analyse("7k/8/6QK/8/8/8/8/8 b - - 0 1", 20, cancel=threading.Event())
    assert result["lines"] == [{"cp": 0, "eval": "+0.00", "win_white": 50.0, "uci": [], "san": [], "depth": 0}]


class WaitingInteractive:
    def __init__(self):
        self.started = threading.Event()
        self.finished = threading.Event()

    def analyse(self, fen, depth, multipv, cancel):
        self.started.set()
        try:
            assert cancel.wait(5), "endpoint did not signal cancellation"
            raise AnalysisCancelled()
        finally:
            self.finished.set()


def request_for(db, interactive, disconnected):
    async def is_disconnected():
        return disconnected

    return SimpleNamespace(
        app=SimpleNamespace(state=SimpleNamespace(services=SimpleNamespace(db=db, interactive=interactive))),
        is_disconnected=is_disconnected,
    )


def test_disconnected_endpoint_signals_and_joins_worker(db):
    service = WaitingInteractive()
    request = request_for(db, service, True)

    async def scenario():
        with pytest.raises(HTTPException) as error:
            await engine_analyse(request, EngineIn(fen=chess.STARTING_FEN))
        assert error.value.status_code == 499
        assert service.finished.is_set()

    asyncio.run(scenario())


def test_cancelled_endpoint_joins_worker_before_propagating(db):
    service = WaitingInteractive()
    request = request_for(db, service, False)

    async def scenario():
        task = asyncio.create_task(engine_analyse(request, EngineIn(fen=chess.STARTING_FEN)))
        while not service.started.is_set():
            await asyncio.sleep(0.001)
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert service.finished.is_set()

    asyncio.run(scenario())
