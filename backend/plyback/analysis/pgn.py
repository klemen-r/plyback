"""PGN parsing: moves, clocks, time control, results."""

from __future__ import annotations

import io
import re
from dataclasses import dataclass, field

import chess
import chess.pgn


@dataclass
class TimeControl:
    base: float | None  # seconds
    increment: float
    daily: bool
    raw: str

    @property
    def label(self) -> str:
        if self.daily:
            return self.raw
        if self.base is None:
            return self.raw or "?"
        base_min = self.base / 60
        b = f"{base_min:g}" if base_min >= 1 else f"{int(self.base)}s"
        return f"{b}+{self.increment:g}"


def parse_time_control(tc: str | None) -> TimeControl:
    tc = (tc or "").strip()
    if not tc or tc == "-":
        return TimeControl(None, 0, False, tc)
    if "/" in tc:  # daily: "1/86400"
        return TimeControl(None, 0, True, tc)
    m = re.fullmatch(r"(\d+)(?:\+(\d+(?:\.\d+)?))?", tc)
    if not m:
        return TimeControl(None, 0, False, tc)
    return TimeControl(float(m.group(1)), float(m.group(2) or 0), False, tc)


@dataclass
class ParsedGame:
    headers: dict[str, str]
    initial_board: chess.Board
    moves: list[chess.Move]
    sans: list[str]
    clocks: list[float | None]  # remaining clock after each ply
    chess960: bool
    result: str
    boards: list[chess.Board] = field(default_factory=list)  # position before each ply + final

    @property
    def plies(self) -> int:
        return len(self.moves)


class PGNError(ValueError):
    pass


def parse_pgn(pgn: str) -> ParsedGame:
    game = chess.pgn.read_game(io.StringIO(pgn))
    if game is None:
        raise PGNError("Could not parse PGN")
    if game.errors:
        raise PGNError(f"PGN contains illegal moves: {game.errors[0]}")
    variant = game.headers.get("Variant", "").lower()
    if variant and variant not in ("standard", "chess960", "fischerandom", "from position"):
        raise PGNError(f"Unsupported variant: {game.headers.get('Variant')}")
    board = game.board()
    chess960 = board.chess960
    moves, sans, clocks, boards = [], [], [], [board.copy()]
    for node in game.mainline():
        mv = node.move
        sans.append(board.san(mv))
        moves.append(mv)
        board.push(mv)
        boards.append(board.copy())
        clocks.append(node.clock())
    return ParsedGame(
        dict(game.headers), game.board(), moves, sans, clocks, chess960, game.headers.get("Result", "*"), boards
    )


def time_spent(clocks: list[float | None], tc: TimeControl) -> list[float | None]:
    """Seconds spent on each ply, derived from remaining clocks and the increment."""
    out: list[float | None] = []
    last: dict[int, float | None] = {0: tc.base, 1: tc.base}
    for i, c in enumerate(clocks):
        side = i % 2
        prev = last[side]
        if c is None or prev is None or tc.daily:
            out.append(None)
        else:
            out.append(round(max(0.0, prev - c + tc.increment), 2))
        last[side] = c
    return out


def split_games(text: str) -> list[str]:
    """Split a multi-game PGN file into individual game strings."""
    stream = io.StringIO(text)
    out = []
    while True:
        offset = stream.tell()
        headers = chess.pgn.read_headers(stream)
        if headers is None:
            break
        end = stream.tell()
        out.append(text[offset:end].strip())
    return [g for g in out if g]
