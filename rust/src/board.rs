//! Shared board geometry and material helpers, without search or persistence.
use shakmaty::{Chess, Color, Position, Role, Square};
use std::collections::HashSet;

pub(crate) fn piece_value(role: Role, king_value: i32) -> i32 {
    match role {
        Role::Pawn => 1,
        Role::Knight | Role::Bishop => 3,
        Role::Rook => 5,
        Role::Queen => 9,
        Role::King => king_value,
    }
}

pub(crate) fn material_balance(pos: &Chess, side: Color, king_value: i32) -> i32 {
    pos.board()
        .iter()
        .map(|(_, p)| piece_value(p.role, king_value) * if p.color == side { 1 } else { -1 })
        .sum()
}

/// Whether leaving `from`, optionally placing its piece on `to`, exposes its own king.
/// Capturing the pinning piece or staying on the pin line can keep the king safe.
/// Kings and positions where the king is already in check do not count as pins.
pub(crate) fn leaves_king_exposed(pos: &Chess, from: Square, to: Option<Square>) -> bool {
    let board = pos.board();
    let Some(piece) = board.piece_at(from) else {
        return false;
    };
    let Some(king) = board.king_of(piece.color) else {
        return false;
    };
    if piece.role == Role::King || board.attacks_to(king, !piece.color, board.occupied()).any() {
        return false;
    }
    let mut after = board.clone();
    after.discard_piece_at(from);
    if let Some(to) = to {
        after.set_piece_at(to, piece);
    }
    after.attacks_to(king, !piece.color, after.occupied()).any()
}

/// Classify the non-pawn material in a FEN's piece-placement field.
pub(crate) fn endgame_type(piece_placement: &str) -> &'static str {
    let pieces: HashSet<char> = piece_placement
        .to_ascii_lowercase()
        .chars()
        .filter(|c| "nbrq".contains(*c))
        .collect();
    if pieces.is_empty() {
        "pawn"
    } else if pieces.len() == 1 && pieces.contains(&'r') {
        "rook"
    } else if pieces.iter().all(|p| "nb".contains(*p)) {
        "minor piece"
    } else if pieces.len() == 1 && pieces.contains(&'q') {
        "queen"
    } else {
        "mixed"
    }
}
