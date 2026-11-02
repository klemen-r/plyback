"""Opening identification from the Lichess ``chess-openings`` dataset (CC0).

Positions (not move orders) are matched, so transpositions are recognised.  The
deepest named position reached during the game determines the opening.
"""

from __future__ import annotations

import csv
import functools
import io
import json
from dataclasses import dataclass
from pathlib import Path

import chess
import chess.pgn

DATA_DIR = Path(__file__).resolve().parent.parent / "data" / "openings"


@dataclass(frozen=True)
class Opening:
    eco: str
    name: str
    family: str
    variation: str
    ply: int  # length of the named line

    def as_dict(self) -> dict:
        return {"eco": self.eco, "name": self.name, "family": self.family, "variation": self.variation}


def _split_name(name: str) -> tuple[str, str]:
    family, _, variation = name.partition(":")
    return family.strip(), variation.strip()


def epd_key(board: chess.Board) -> str:
    # EPD without move counters; en passant only when legal (python-chess default).
    return board.epd()


COMPILED = DATA_DIR.parent / "openings.json"


def build_index() -> tuple[dict[str, Opening], set[str]]:
    """Parse the TSV sources (slow; used to (re)generate ``openings.json``)."""
    named: dict[str, Opening] = {}
    book: set[str] = set()
    for f in sorted(DATA_DIR.glob("*.tsv")):
        with f.open(encoding="utf-8") as fh:
            for row in csv.DictReader(fh, delimiter="	"):
                game = chess.pgn.read_game(io.StringIO(row["pgn"]))
                if game is None:
                    continue
                board = game.board()
                ply = 0
                for move in game.mainline_moves():
                    board.push(move)
                    ply += 1
                    book.add(epd_key(board))
                key = epd_key(board)
                family, variation = _split_name(row["name"])
                existing = named.get(key)
                # Prefer the shortest move order (most canonical) for a given position.
                if existing is None or ply < existing.ply:
                    named[key] = Opening(row["eco"], row["name"], family, variation, ply)
    return named, book


def write_compiled(path: Path = COMPILED) -> None:
    named, book = build_index()
    data = {"named": {k: [o.eco, o.name, o.ply] for k, o in sorted(named.items())}, "book": sorted(book)}
    path.write_text(json.dumps(data, separators=(",", ":")), encoding="utf-8")


@functools.lru_cache(maxsize=1)
def _index() -> tuple[dict[str, Opening], frozenset[str]]:
    if COMPILED.exists():
        data = json.loads(COMPILED.read_text(encoding="utf-8"))
        named = {}
        for key, (eco, name, ply) in data["named"].items():
            family, variation = _split_name(name)
            named[key] = Opening(eco, name, family, variation, ply)
        return named, frozenset(data["book"])
    named, book = build_index()
    return named, frozenset(book)


def warm() -> None:
    _index()


def identify(boards_after_each_ply: list[chess.Board]) -> tuple[Opening | None, int]:
    """Return (deepest named opening, number of leading plies that are opening theory)."""
    named, book = _index()
    best: Opening | None = None
    book_plies = 0
    still_book = True
    for i, b in enumerate(boards_after_each_ply):
        key = epd_key(b)
        if still_book and key in book:
            book_plies = i + 1
        else:
            still_book = False
        op = named.get(key)
        if op is not None:
            best = op
        if i > 40 and not still_book:
            break
    return best, book_plies


def is_book_position(board: chess.Board) -> bool:
    return epd_key(board) in _index()[1]


def from_chesscom_eco_url(url: str | None) -> str | None:
    """Readable opening name from Chess.com's ECOUrl header (fallback only)."""
    if not url or "/openings/" not in url:
        return None
    slug = url.rsplit("/openings/", 1)[1]
    slug = slug.split("...")[0]
    # Drop trailing move sequences like "-3.Nc3" which Chess.com appends.
    parts = []
    for token in slug.split("-"):
        if token and token[0].isdigit():
            break
        parts.append(token)
    return " ".join(parts).strip() or None
