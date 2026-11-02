"""Curated free training resources (Lichess) mapped to weakness categories."""

from __future__ import annotations

import re
from urllib.parse import quote


def lichess_analysis_url(fen: str) -> str:
    return "https://lichess.org/analysis/" + quote(fen.replace(" ", "_"), safe="/_-")


def lichess_opening_key(name: str) -> str:
    key = name.replace(":", "").replace(",", "").replace("'", "")
    key = re.sub(r"\s+", "_", key.strip())
    return key


def opening_resources(family: str, name: str | None = None) -> list[dict]:
    out = [
        {
            "title": f"{family} — Lichess opening explorer",
            "url": f"https://lichess.org/opening/{quote(lichess_opening_key(name or family))}",
            "kind": "opening",
        },
        {
            "title": f"Puzzles from the {family}",
            "url": f"https://lichess.org/training/{quote(lichess_opening_key(family))}",
            "kind": "puzzles",
        },
        {
            "title": f"Community studies on the {family}",
            "url": f"https://lichess.org/study/search?q={quote(family)}",
            "kind": "study",
        },
    ]
    return out


_P = "https://lichess.org/training/"
RESOURCES: dict[str, list[dict]] = {
    "hanging_piece": [
        {"title": "Puzzles: Hanging piece", "url": _P + "hangingPiece", "kind": "puzzles"},
        {"title": "Lichess Learn: piece protection basics", "url": "https://lichess.org/learn", "kind": "lesson"},
        {"title": "Puzzle Streak (accuracy over speed)", "url": "https://lichess.org/streak", "kind": "puzzles"},
    ],
    "threat_ignored": [
        {"title": "Puzzles: Defensive move", "url": _P + "defensiveMove", "kind": "puzzles"},
        {"title": "Puzzles: Intermezzo (in-between moves)", "url": _P + "intermezzo", "kind": "puzzles"},
        {"title": "Puzzles: Quiet move", "url": _P + "quietMove", "kind": "puzzles"},
    ],
    "allowed_mate": [
        {"title": "Puzzles: Back-rank mate", "url": _P + "backRankMate", "kind": "puzzles"},
        {"title": "Puzzles: Mate in 2", "url": _P + "mateIn2", "kind": "puzzles"},
        {"title": "Lichess Practice: checkmate patterns", "url": "https://lichess.org/practice", "kind": "practice"},
    ],
    "allows_fork": [
        {"title": "Puzzles: Fork", "url": _P + "fork", "kind": "puzzles"},
        {"title": "Puzzles: Double attack / attraction", "url": _P + "attraction", "kind": "puzzles"},
    ],
    "missed_tactics": [
        {"title": "Puzzles: Advantage (seize your chance)", "url": _P + "advantage", "kind": "puzzles"},
        {"title": "Puzzles: Crushing", "url": _P + "crushing", "kind": "puzzles"},
        {"title": "Puzzles: Fork", "url": _P + "fork", "kind": "puzzles"},
        {"title": "Puzzles: Pin", "url": _P + "pin", "kind": "puzzles"},
    ],
    "missed_mate": [
        {"title": "Puzzles: Mate in 1", "url": _P + "mateIn1", "kind": "puzzles"},
        {"title": "Puzzles: Mate in 2", "url": _P + "mateIn2", "kind": "puzzles"},
        {"title": "Puzzles: Mate in 3", "url": _P + "mateIn3", "kind": "puzzles"},
    ],
    "endgame": [
        {
            "title": "Lichess Practice: basic & intermediate endgames",
            "url": "https://lichess.org/practice",
            "kind": "practice",
        },
        {"title": "Puzzles: Rook endgames", "url": _P + "rookEndgame", "kind": "puzzles"},
        {"title": "Puzzles: Pawn endgames", "url": _P + "pawnEndgame", "kind": "puzzles"},
        {"title": "Puzzles: Endgame (all)", "url": _P + "endgame", "kind": "puzzles"},
    ],
    "opening": [
        {"title": "Lichess opening explorer", "url": "https://lichess.org/analysis#explorer", "kind": "opening"},
        {"title": "Puzzles: Opening tactics", "url": _P + "opening", "kind": "puzzles"},
    ],
    "time": [
        {"title": "Puzzle Storm (fast pattern recognition)", "url": "https://lichess.org/storm", "kind": "puzzles"},
        {"title": "Lichess Practice", "url": "https://lichess.org/practice", "kind": "practice"},
    ],
    "conversion": [
        {"title": "Puzzles: Crushing (finish the job)", "url": _P + "crushing", "kind": "puzzles"},
        {"title": "Puzzles: Equality → advantage", "url": _P + "advantage", "kind": "puzzles"},
        {
            "title": "Practice converting: play the critical positions vs. the computer from the Lichess analysis board",
            "url": "https://lichess.org/analysis",
            "kind": "practice",
        },
    ],
    "positional": [
        {
            "title": "Lichess studies: pawn structures",
            "url": "https://lichess.org/study/search?q=pawn+structure",
            "kind": "study",
        },
        {
            "title": "Lichess studies: middlegame plans",
            "url": "https://lichess.org/study/search?q=middlegame+plans",
            "kind": "study",
        },
        {"title": "Puzzles: Quiet move", "url": _P + "quietMove", "kind": "puzzles"},
    ],
    "tilt": [
        {
            "title": "Take a break after a loss — play a few Puzzle Streak puzzles instead",
            "url": "https://lichess.org/streak",
            "kind": "habit",
        },
    ],
}
