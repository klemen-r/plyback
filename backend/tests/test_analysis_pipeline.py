"""PV presentation must keep complete engine lines while sharing their conversion."""

import json

from bench.analysis import record, replay
from plyback.analysis import analyzer
from plyback.analysis.engine import Line
from plyback.analysis.pgn import parse_pgn

from .conftest import requires_stockfish


def test_reused_pv_preserves_best_alternatives_and_reply(monkeypatch):
    game = parse_pgn("1. e4 e5 2. Nf3 *")
    continuation = ["e2e4", "e7e5", "g1f3", "b8c6", "f1b5", "a7a6", "b5a4", "g8f6", "e1g1", "f8e7", "f1e1", "b7b5", "a4b3"]
    lines = [
        [Line(0, continuation[index:], 16), Line(-1, [], 16)]
        for index in range(len(game.boards))
    ]
    expected = [analyzer.pv_to_san(board, evaluation[0].pv, 12) for board, evaluation in zip(game.boards, lines, strict=True)]
    original = analyzer.pv_to_san
    conversions = []

    def observe(board, pv, limit=10):
        conversions.append((board.fen(), tuple(pv), limit))
        return original(board, pv, limit)

    monkeypatch.setattr(analyzer, "pv_to_san", observe)
    evaluations = iter(lines)
    result = analyzer.analyze_parsed(game, lambda *_: next(evaluations))
    for index, move in enumerate(result["moves"]):
        assert move["lines"][0]["san"] == expected[index]
        assert move["reply"]["san"] == expected[index + 1][:10]
        assert move["best_san"] == expected[index][0]
    assert len(result["moves"][0]["lines"][0]["san"]) == 12
    assert len(conversions) == sum(map(len, lines))


@requires_stockfish
def test_real_engine_recording_replays_and_restores_instrumentation(real_engine):
    original = real_engine._engine.analyse
    opts = analyzer.AnalysisOptions(depth=8, multipv=2)
    recording = record(real_engine, "1. e4 e5 *", opts, 1.0, 2000)
    assert real_engine._engine.analyse == original
    assert sum(call["nodes"] for call in recording["calls"]) > 0
    restored = json.loads(json.dumps(recording))
    result = replay(restored, opts)
    assert result["moves"] == recording["result"]["moves"]
