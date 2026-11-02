"""Engine-accuracy checks against the real Stockfish on well-known games."""

import chess

from plyback.analysis.analyzer import AnalysisOptions, analyze_parsed
from plyback.analysis.pgn import parse_pgn
from plyback.analysis.winprob import mate_in

from .conftest import requires_stockfish


def run(engine, pgn, depth=16):
    return analyze_parsed(parse_pgn(pgn), lambda b, d, m: engine.analyse(b, d, m), AnalysisOptions(depth=depth))


@requires_stockfish
def test_engine_sanity(real_engine):
    lines = real_engine.analyse(chess.Board(), 14, 2)
    assert len(lines) == 2
    assert -80 < lines[0].cp < 120  # start position is roughly equal, slightly better for White
    # Mate in 1 is found and encoded.
    b = chess.Board("6k1/5ppp/8/8/8/8/5PPP/3R2K1 w - - 0 1")
    lines = real_engine.analyse(b, 12, 1)
    assert lines[0].pv[0] == "d1d8" and mate_in(lines[0].cp) == 1


@requires_stockfish
def test_scholars_mate(real_engine):
    r = run(real_engine, "1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0")
    by_san = {m["san"]: m for m in r["moves"]}
    assert by_san["Nf6"]["classification"] == "blunder"
    assert "allowed_mate" in by_san["Nf6"]["tags"]
    assert "mate" in by_san["Nf6"]["explanation"].lower()
    assert by_san["Qxf7#"]["classification"] == "best"
    assert "missed_mate" not in by_san["Qxf7#"]["tags"]
    assert by_san["e4"]["classification"] == "book"
    assert r["opening"] is not None
    assert r["accuracy_white"] > r["accuracy_black"]


@requires_stockfish
def test_legals_mate_brilliant(real_engine):
    r = run(real_engine, "1. e4 e5 2. Nf3 d6 3. Bc4 Bg4 4. Nc3 g6 5. Nxe5 Bxd1 6. Bxf7+ Ke7 7. Nd5# 1-0")
    by_ply = {m["ply"]: m for m in r["moves"]}
    assert by_ply[9]["san"] == "Nxe5" and by_ply[9]["classification"] == "brilliant"
    assert "sacrifice" in by_ply[9]["explanation"].lower()
    assert by_ply[10]["classification"] == "blunder"  # Bxd1?? allows mate
    assert by_ply[12]["classification"] == "forced"  # Ke7 is the only legal move
    assert r["counts"]["white"].get("brilliant") == 1


@requires_stockfish
def test_hanging_queen_explanation(real_engine):
    # 3...Qh4?? simply hangs the queen to the knight on f3.
    r = run(real_engine, "1. e4 e5 2. Nf3 Nc6 3. d4 Qh4 4. Nxh4 *")
    m = next(x for x in r["moves"] if x["san"] == "Qh4")
    assert m["classification"] == "blunder"
    assert "hanging_piece" in m["tags"]
    assert "queen" in m["explanation"]
    assert m["best_san"] and m["best_san"] != "Qh4"
    nxt = next(x for x in r["moves"] if x["san"] == "Nxh4")
    assert nxt["classification"] in ("best", "excellent")  # taking a free queen is not "great"


@requires_stockfish
def test_blackburne_shilling(real_engine):
    r = run(real_engine, "1. e4 e5 2. Nf3 Nc6 3. Bc4 Nd4 4. Nxe5 Qg5 5. Nxf7 Qxg2 6. Rf1 Qxe4+ 7. Be2 Nf3# 0-1")
    by_ply = {m["ply"]: m for m in r["moves"]}
    assert by_ply[9]["classification"] in ("mistake", "blunder")  # 5.Nxf7?
    assert by_ply[10]["classification"] in ("best", "great")  # Qxg2 is strong but not a sacrifice
    assert r["accuracy_black"] > r["accuracy_white"]
