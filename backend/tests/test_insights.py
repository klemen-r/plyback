"""Insight computations on a small synthetic dataset where the answers are known."""

import time

import chess
import pytest

from plyback.analysis.analyzer import AnalysisOptions, analyze_parsed
from plyback.analysis.pgn import parse_pgn
from plyback.insights.compute import Filters, _load, _time, compute_insights, confidence, wilson
from plyback.services import store

from .conftest import FakeEngine

GAMES = [
    # (pgn moves, white, black, result)
    ("1. e4 e5 2. Nf3 Nc6 3. d4 Qh4 4. Nxh4 1-0", "me", "opp1", "1-0"),
    ("1. e4 e5 2. Nf3 Nc6 3. d4 Qh4 4. Nxh4 1-0", "me", "opp2", "1-0"),
    ("1. d4 d5 2. Bf4 Qd6 3. Bxd6 0-1", "opp3", "me", "1-0"),
    ("1. e4 c5 2. Nf3 d6 3. d4 cxd4 4. Nxd4 1/2-1/2", "me", "opp4", "1/2-1/2"),
]


def seed(db):
    with db.tx() as c:
        acc = c.execute("INSERT INTO accounts (username, added_at) VALUES ('me', ?)", (int(time.time()),)).lastrowid
    eng = FakeEngine()
    for i, (moves, w, b, res) in enumerate(GAMES):
        pgn = f'[White "{w}"]\n[Black "{b}"]\n[Result "{res}"]\n[TimeControl "180"]\n[UTCDate "2026.10.0{i + 1}"]\n[UTCTime "12:00:00"]\n\n{moves}\n'
        row = store.game_row_from_pgn(pgn)
        row["end_time"] = 1790000000 + i * 3600
        row["time_class"] = "blitz"
        row["rated"] = 1
        row["white_rating"], row["black_rating"] = 600 + i, 610
        with db.tx() as c:
            ((gid, _),) = store.upsert_games(c, [row])
            store.link_account_games(c, acc, "me", [row], [gid])
        result = analyze_parsed(parse_pgn(pgn), lambda bd, d, m: eng.analyse(bd, d, m), AnalysisOptions(depth=8))
        store.save_analysis(db, gid, result, 8, 2, eng.name)
    return acc


def test_overview_and_openings(db):
    acc = seed(db)
    ins = compute_insights(db, acc, {})
    o = ins["overview"]
    assert (o["games"], o["wins"], o["draws"], o["losses"]) == (4, 2, 1, 1)
    assert o["score"] == 62.5
    assert o["white"]["games"] == 3 and o["black"]["games"] == 1
    fams = {f["family"]: f for f in ins["openings"]["white"]["families"]}
    assert sum(f["games"] for f in fams.values()) == 3
    assert ins["openings"]["white"]["first_moves"][0]["move"] == "e4"
    assert ins["data_quality"]["analyzed"] == 4


def test_recurring_mistake_found(db):
    acc = seed(db)
    ins = compute_insights(db, acc, {})
    # The opponent hangs the queen twice, but 'me' never errs in the same position twice,
    # so recurring mistakes must not list opponent moves.
    for r in ins["recurring"]:
        assert all(p["san"] != "Qh4" for p in r["played"])


def test_filters(db):
    acc = seed(db)
    ins = compute_insights(db, acc, Filters(color="black"))
    assert ins["overview"]["games"] == 1 and ins["overview"]["losses"] == 1
    ins = compute_insights(db, acc, {"since": 1790000000 + 2 * 3600})
    assert ins["overview"]["games"] == 2
    ins = compute_insights(db, acc, {"time_classes": "rapid"})
    assert ins["overview"]["games"] == 0


def test_error_tags_and_opponent_baseline(db):
    acc = seed(db)
    ins = compute_insights(db, acc, {})
    assert ins["overview"]["opp_per_game"]["blunder"] > 0  # opponents hung their queen
    assert ins["missed"]["opponent_errors"] >= 2
    assert ins["missed"]["punished"] >= 2  # Nxh4 / Bxd6 took the queen


def test_statistics_helpers():
    assert confidence(3) == "insufficient" and confidence(50) == "high"
    lo, hi = wilson(5, 10)
    assert lo < 0.5 < hi
    assert wilson(0, 0) == (0.0, 1.0)


def test_board_fen_examples_are_valid(db):
    acc = seed(db)
    ins = compute_insights(db, acc, {})
    for p in ins["priorities"]:
        for ex in p["examples"]:
            assert chess.Board(ex["fen"]).is_valid()
            assert ex["lichess_url"].startswith("https://lichess.org/analysis/")


@pytest.mark.parametrize("filters", [
    [], "blitz", 0,
    {"since": []}, {"since": True}, {"since": 1.5}, {"since": -1},
    {"until": 253_402_300_800}, {"since": "x"}, {"since": "9" * 1000},
    {"since": 2, "until": 1},
    {"time_classes": {}}, {"time_classes": 1}, {"time_classes": [["blitz"]]},
    {"time_classes": ["blitz"] * 5}, {"time_classes": "unknown"},
    {"color": ["white"]}, {"color": "invalid"},
    {"rated": {}}, {"rated": 2}, {"rated": "sometimes"},
    {"opening": ["Italian"]}, {"opening": "x" * 513},
    {"opponent": {}}, {"opponent": "x" * 257},
])
def test_invalid_filter_input_is_rejected(filters):
    with pytest.raises(ValueError):
        Filters.from_dict(filters)


def test_filter_normalization_and_timestamp_boundaries():
    filters = Filters.from_dict({
        "since": "0", "until": 253_402_300_799,
        "time_classes": "blitz, rapid", "color": "white", "rated": "false",
        "opening": "Italian Game", "opponent": "player",
    })
    assert filters == Filters(0, 253_402_300_799, ["blitz", "rapid"], "white", False, "Italian Game", "player")
    assert Filters.from_dict({"time_classes": ["blitz", "blitz"], "color": "any", "rated": "any"}) == Filters(
        time_classes=["blitz"],
    )
    assert Filters.from_dict({"since": "", "color": "", "rated": "", "time_classes": ""}) == Filters()


def test_zero_timestamp_until_is_applied(db):
    account = seed(db)
    assert compute_insights(db, account, {"until": 0})["overview"]["games"] == 0


def test_time_speed_boundaries_and_low_clock_exclusion(db):
    account = seed(db)
    game = _load(db, account, Filters())[0]
    game.user_moves = [
        move._replace(time_spent=spent, clock=clock, is_book=False, phase="opening")
        for move, spent, clock in zip(game.user_moves, [1.8, 5.4, 14.4, 1.0], [100, 100, 100, 18], strict=True)
    ]
    result = _time([game], [game])
    assert [bucket["moves"] for bucket in result["speed_vs_quality"]] == [0, 1, 1, 1]
    assert result["time_trouble"]["moves_low"] == 1
    assert result["time_trouble"]["games"] == 1
    assert result["avg_time_per_move"]["opening"]["user"] == 5.65
