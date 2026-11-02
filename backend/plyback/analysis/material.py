"""Static board helpers: material, static exchange evaluation, hanging pieces, forks, phase."""

from __future__ import annotations

import chess

VALUES = {chess.PAWN: 1, chess.KNIGHT: 3, chess.BISHOP: 3, chess.ROOK: 5, chess.QUEEN: 9, chess.KING: 0}
PIECE_NAMES = {
    chess.PAWN: "pawn",
    chess.KNIGHT: "knight",
    chess.BISHOP: "bishop",
    chess.ROOK: "rook",
    chess.QUEEN: "queen",
    chess.KING: "king",
}


def material(board: chess.Board, color: chess.Color) -> int:
    return sum(VALUES[piece] * board.pieces_mask(piece, color).bit_count() for piece in range(chess.PAWN, chess.KING))


def balance(board: chess.Board, color: chess.Color) -> int:
    """Material balance in pawns from ``color``'s point of view."""
    return material(board, color) - material(board, not color)


def non_pawn_material(board: chess.Board) -> int:
    return 3 * (board.knights | board.bishops).bit_count() + 5 * board.rooks.bit_count() + 9 * board.queens.bit_count()


def phase(board: chess.Board, ply: int, in_book: bool = False) -> str:
    """opening / middlegame / endgame.

    * endgame: non-pawn material of both sides ≤ 26 points, or queens are off and ≤ 32 points
    * opening: the first 10 moves (20 plies) or while the game is still in opening theory
    * middlegame: everything else
    """
    npm = non_pawn_material(board)
    if npm <= 26 or (not board.queens and npm <= 32):
        return "endgame"
    if ply <= 20 or in_book:
        return "opening"
    return "middlegame"


def endgame_type(fen: str) -> str:
    """pawn / rook / minor piece / queen / mixed, from the pieces left on the board."""
    board_part = fen.split(" ", 1)[0].lower()
    kinds = {ch for ch in board_part if ch in "nbrq"}
    if not kinds:
        return "pawn"
    if kinds == {"r"}:
        return "rook"
    if kinds <= {"n", "b"}:
        return "minor piece"
    if kinds == {"q"}:
        return "queen"
    return "mixed"


def _captured_value(board: chess.Board, move: chess.Move) -> int:
    if board.is_en_passant(move):
        return 1
    piece = board.piece_at(move.to_square)
    gain = VALUES[piece.piece_type] if piece else 0
    if move.promotion:
        gain += VALUES[move.promotion] - 1
    return gain


def see(board: chess.Board, square: chess.Square, depth: int = 0) -> int:
    """Static exchange evaluation for the side to move capturing on ``square``.

    Uses real legal moves at every step, so pins and x-ray attackers are handled.
    Returns the best net material gain (≥ 0, the side may decline to capture).
    """
    target = board.piece_at(square)
    if target is None or target.color == board.turn or depth > 12:
        return 0
    captures = list(board.generate_legal_moves(to_mask=chess.BB_SQUARES[square]))
    if not captures:
        return 0
    # Capture with the least valuable piece first.
    move = min(captures, key=lambda m: (VALUES[board.piece_type_at(m.from_square)] or 100, m.promotion is None))
    if target.piece_type == chess.KING:
        return 0
    gain = _captured_value(board, move)
    board.push(move)
    try:
        reply = see(board, square, depth + 1)
    finally:
        board.pop()
    return max(0, gain - reply)


def hanging_pieces(board: chess.Board, color: chess.Color, min_value: int = 1) -> list[tuple[chess.Square, int]]:
    """Pieces of ``color`` that the opponent (who must be to move) can win by capture.

    Returns ``(square, net_gain_for_opponent)`` sorted by gain descending.
    """
    if board.turn == color:
        return []
    out = []
    for sq, piece in board.piece_map().items():
        if piece.color != color or piece.piece_type == chess.KING or VALUES[piece.piece_type] < min_value:
            continue
        if not board.is_attacked_by(not color, sq):
            continue
        g = see(board, sq)
        if g > 0:
            out.append((sq, g))
    return sorted(out, key=lambda x: -x[1])


def is_sacrifice(board_before: chess.Board, move: chess.Move, min_net: int = 2) -> tuple[bool, str | None]:
    """Whether ``move`` offers material: afterwards the opponent can win at least ``min_net``
    pawns more than the move itself captured.  Returns (flag, description)."""
    mover = board_before.turn
    captured = _captured_value(board_before, move)
    after = board_before.copy(stack=False)
    after.push(move)
    if after.is_checkmate():
        return False, None
    # Pieces that were already en prise before the move don't count (leaving them is not
    # offering them); the moved piece always counts.
    pre = board_before.copy(stack=False)
    pre.turn = not mover
    pre.ep_square = None
    already = dict(hanging_pieces(pre, mover, min_value=3))
    best_sq, best_gain = None, 0
    for sq, gain in hanging_pieces(after, mover, min_value=3):
        if sq != move.to_square and already.get(sq, 0) >= gain:
            continue
        if gain > best_gain:
            best_sq, best_gain = sq, gain
    if best_sq is None:
        return False, None
    net = best_gain - captured
    if net < min_net:
        return False, None
    # A "sacrifice" that the opponent can't accept without immediately losing at least as much
    # (in-between moves, desperados, counter-attacks on the queen) is not a real sacrifice.
    takes = list(after.generate_legal_moves(to_mask=chess.BB_SQUARES[best_sq]))
    if takes:
        take = min(takes, key=lambda m: VALUES[after.piece_type_at(m.from_square)] or 100)
        b2 = after.copy(stack=False)
        b2.push(take)
        if not b2.is_game_over():
            regain = max(
                (see(b2, sq) for sq, p in b2.piece_map().items() if p.color != mover and p.piece_type != chess.KING),
                default=0,
            )
            if regain >= net:
                return False, None
    piece = after.piece_at(best_sq)
    return True, f"{PIECE_NAMES[piece.piece_type]} on {chess.square_name(best_sq)}"


def fork_targets(board_after: chess.Board, move: chess.Move) -> list[chess.Square]:
    """If the piece that just moved attacks two or more valuable enemy targets
    (king, higher-value pieces, or undefended pieces worth ≥ 3), return those squares."""
    piece = board_after.piece_at(move.to_square)
    if piece is None:
        return []
    attacker_value = VALUES[piece.piece_type] or 100
    # The forking piece must not simply be lost for free.
    if board_after.is_attacked_by(not piece.color, move.to_square) and see(board_after, move.to_square) >= attacker_value:
        return []
    targets = []
    for sq in board_after.attacks(move.to_square):
        t = board_after.piece_at(sq)
        if t is None or t.color == piece.color:
            continue
        if (
            t.piece_type == chess.KING
            or VALUES[t.piece_type] > attacker_value
            or (VALUES[t.piece_type] >= 3 and not board_after.is_attacked_by(t.color, sq))
        ):
            targets.append(sq)
    return targets if len(targets) >= 2 else []


def material_words(points: int) -> str:
    points = abs(points)
    return {
        1: "a pawn",
        2: "two pawns",
        3: "a piece",
        4: "a piece and a pawn",
        5: "a rook",
        6: "a rook and a pawn",
        7: "a rook and two pawns",
        8: "a rook and a piece",
    }.get(points, "the queen" if points in (9, 10) else f"{points} points of material")
