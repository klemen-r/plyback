"""Cached moves must follow the owning database and committed analysis revision."""

from plyback.analysis.analyzer import AnalysisOptions, analyze_parsed
from plyback.analysis.pgn import parse_pgn
from plyback.db import Database
from plyback.insights.compute import compute_insights
from plyback.services import store

from .conftest import FakeEngine
from .test_insights import seed


def test_move_cache_isolated_between_databases(db, tmp_path):
    account = seed(db)
    first = compute_insights(db, account, {})
    other = Database(tmp_path / "other.db")
    try:
        other.migrate()
        other_account = seed(other)
        other.execute("UPDATE moves SET classification='best'")
        second = compute_insights(other, other_account, {})
        assert first["overview"]["opp_per_game"]["blunder"] > 0
        assert second["overview"]["opp_per_game"]["blunder"] == 0
    finally:
        other.close_all()


def test_same_second_reanalysis_invalidates_moves(db, monkeypatch):
    monkeypatch.setattr(store.time, "time", lambda: 1_790_000_000)
    account = seed(db)
    first = compute_insights(db, account, {})
    game = db.one("SELECT id, pgn FROM games WHERE black_username='me'")
    engine = FakeEngine()
    result = analyze_parsed(parse_pgn(game["pgn"]), engine.analyse, AnalysisOptions(depth=8))
    for move in result["moves"]:
        move["classification"] = "best"
        move["ep_loss"] = 0
        move["tags"] = {}
    original = db.one("SELECT analyzed_at, revision FROM game_analysis WHERE game_id=?", (game["id"],))
    store.save_analysis(db, game["id"], result, 8, 2, engine.name)
    updated = db.one("SELECT analyzed_at, revision FROM game_analysis WHERE game_id=?", (game["id"],))
    second = compute_insights(db, account, {})
    assert updated["analyzed_at"] == original["analyzed_at"]
    assert updated["revision"] == original["revision"] + 1
    assert second["overview"]["per_game"]["blunder"] < first["overview"]["per_game"]["blunder"]
