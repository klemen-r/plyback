import pytest

from plyback.analysis import winprob as wp


def test_win_pct_symmetry_and_bounds():
    assert wp.win_pct_white(0) == pytest.approx(50)
    for cp in (50, 150, 400, 900):
        assert wp.win_pct_white(cp) + wp.win_pct_white(-cp) == pytest.approx(100)
        assert 50 < wp.win_pct_white(cp) < 100
    # capped at ±1000 cp
    assert wp.win_pct_white(5000) == pytest.approx(wp.win_pct_white(1000))


def test_mate_encoding():
    assert wp.mate_in(wp.MATE_CP - 3) == 3
    assert wp.mate_in(-(wp.MATE_CP - 2)) == -2
    assert wp.mate_in(250) is None
    assert wp.win_pct_white(wp.MATE_CP - 5) == 100
    assert wp.win_pct_white(-(wp.MATE_CP - 5)) == 0
    assert wp.format_eval(wp.MATE_CP - 3) == "M3"
    assert wp.format_eval(-(wp.MATE_CP - 2)) == "-M2"
    assert wp.format_eval(125) == "+1.25"


def test_move_accuracy():
    assert wp.move_accuracy(60, 60) == 100
    assert wp.move_accuracy(60, 70) == 100
    a_small = wp.move_accuracy(60, 55)
    a_big = wp.move_accuracy(60, 20)
    assert 0 <= a_big < a_small < 100


def test_cp_loss_caps_and_perspective():
    assert wp.cp_loss(100, -50, True) == 150
    assert wp.cp_loss(-100, 50, False) == 150
    assert wp.cp_loss(100, 200, True) == 0
    assert wp.cp_loss(wp.MATE_CP - 1, 0, True) == 1000


def test_game_accuracy_perfect_vs_blunder():
    perfect = [20, 25, 20, 30, 25, 30, 20, 25, 30, 20, 25]
    w, b = wp.game_accuracy(perfect)
    assert w > 95 and b > 95
    blunder = [20, 25, 20, 30, 25, 30, 25, -900, -900, -950, -900]  # white blunders on move 4 (ply 7)
    w2, b2 = wp.game_accuracy(blunder)
    assert w2 < w and w2 < b2


def test_game_accuracy_short_game():
    assert wp.game_accuracy([0]) == (None, None)
    w, b = wp.game_accuracy([20, 30])
    assert w is not None and b is None
