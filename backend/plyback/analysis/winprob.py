"""Evaluation → win-probability and accuracy formulas.

All formulas here are public and documented in METHODOLOGY.md:

* Win probability uses the logistic curve fitted by Lichess on real games
  (``50 + 50 * (2 / (1 + exp(-0.00368208 * cp)) - 1)``) with centipawns capped at ±1000.
* Per-move accuracy uses the Lichess "accuracy percent" curve
  ``103.1668 * exp(-0.04354 * winDrop) - 3.1669`` (+1 uncertainty bonus, clamped 0..100).
* Game accuracy is the mean of a volatility-weighted mean and a harmonic mean of the
  per-move accuracies (again the Lichess method).

Evaluations are stored as a single integer from White's point of view where mate
scores are encoded as ``±(MATE_CP - n)`` (n = moves to mate, 0 = already mated).
"""

from __future__ import annotations

import math
from statistics import pstdev

MATE_CP = 100_000
MATE_THRESHOLD = MATE_CP - 1_000
CP_CAP = 1_000


def is_mate(cp: int) -> bool:
    return abs(cp) >= MATE_THRESHOLD


def mate_in(cp: int) -> int | None:
    """Signed moves-to-mate from White's POV (positive: White mates). 0 = checkmate on board."""
    if not is_mate(cp):
        return None
    n = MATE_CP - abs(cp)
    return n if cp > 0 else -n


def capped_cp(cp: int) -> int:
    return max(-CP_CAP, min(CP_CAP, cp))


def win_pct_white(cp: int) -> float:
    """Probability-like winning chances for White in percent (0..100)."""
    if is_mate(cp):
        return 100.0 if cp > 0 else 0.0
    c = capped_cp(cp)
    return 50 + 50 * (2 / (1 + math.exp(-0.00368208 * c)) - 1)


def win_pct(cp_white: int, white: bool) -> float:
    w = win_pct_white(cp_white)
    return w if white else 100.0 - w


def pov_cp(cp_white: int, white: bool) -> int:
    return cp_white if white else -cp_white


def move_accuracy(win_before: float, win_after: float) -> float:
    """Accuracy (0..100) of a single move given the mover's win% before (best play) and after."""
    if win_after >= win_before:
        return 100.0
    drop = win_before - win_after
    raw = 103.1668100711649 * math.exp(-0.04354415386753951 * drop) - 3.166924740191411
    return max(0.0, min(100.0, raw + 1))


def cp_loss(cp_before_white: int, cp_after_white: int, white: bool) -> int:
    """Centipawn loss of a move from the mover's POV, with evaluations capped at ±1000."""
    before = capped_cp(pov_cp(cp_before_white, white))
    after = capped_cp(pov_cp(cp_after_white, white))
    return max(0, before - after)


def _harmonic_mean(xs: list[float]) -> float:
    xs = [max(x, 0.001) for x in xs]  # guard against division by zero for 0% moves
    return len(xs) / sum(1 / x for x in xs)


def game_accuracy(evals_white: list[int], white_moves_first: bool = True) -> tuple[float | None, float | None]:
    """Return (white_accuracy, black_accuracy) from the list of position evaluations.

    ``evals_white`` holds the evaluation of every position of the game, starting with the
    initial position (len = plies + 1).  Uses volatility weighting so that moves made in
    sharp positions count more than moves made in quiet, already-decided positions.
    """
    if len(evals_white) < 2:
        return None, None
    wins = [win_pct_white(cp) for cp in evals_white]
    n_moves = len(wins) - 1
    window = max(2, min(8, n_moves // 10))
    windows: list[list[float]] = []
    for _ in range(max(0, min(window, len(wins)) - 2)):
        windows.append(wins[:window])
    for i in range(0, len(wins) - window + 1):
        windows.append(wins[i : i + window])
    weights = [max(0.5, min(12.0, pstdev(w))) if len(w) > 1 else 0.5 for w in windows]

    per_color: dict[bool, list[tuple[float, float]]] = {True: [], False: []}
    for i in range(n_moves):
        white_moved = (i % 2 == 0) == white_moves_first
        prev, nxt = wins[i], wins[i + 1]
        if white_moved:
            acc = move_accuracy(prev, nxt)
        else:
            acc = move_accuracy(100 - prev, 100 - nxt)
        weight = weights[i] if i < len(weights) else weights[-1]
        per_color[white_moved].append((acc, weight))

    def combine(items: list[tuple[float, float]]) -> float | None:
        if not items:
            return None
        total_w = sum(w for _, w in items)
        weighted = sum(a * w for a, w in items) / total_w if total_w else sum(a for a, _ in items) / len(items)
        harmonic = _harmonic_mean([a for a, _ in items])
        return round((weighted + harmonic) / 2, 1)

    return combine(per_color[True]), combine(per_color[False])


def format_eval(cp_white: int) -> str:
    """Human readable eval from White's POV: '+1.25', '-0.40', 'M3', '-M2', '#'."""
    m = mate_in(cp_white)
    if m is not None:
        if m == 0:
            return "#"
        return f"M{m}" if m > 0 else f"-M{-m}"
    return f"{cp_white / 100:+.2f}"
