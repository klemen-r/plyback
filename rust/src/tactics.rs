//! Conservative labels from legal board geometry and the stored principal variation.
//! No search, score changes, persistence or engine calls. Incomplete lines can leave
//! motifs unlabelled; a geometric threat alone is not evidence of a material win.
use crate::{board, pgn};
use shakmaty::{Chess, Color, Move, Position, Role, Square, san::San, uci::UciMove};

pub const THEMES: &[&str] = &[
    "mate-in-1",
    "mate-in-2",
    "mate-in-3-plus",
    "back-rank-mate",
    "fork",
    "knight-fork",
    "pin",
    "skewer",
    "discovered-attack",
    "hanging-piece",
    "trapped-piece",
    "promotion",
    "defensive",
];

struct Line {
    boards: Vec<Chess>,
    moves: Vec<Move>,
}

impl Line {
    fn parse(fen: &str, best: &str, san: &[String]) -> Option<Self> {
        let mut board = pgn::board(fen, false).ok()?;
        let best = best.parse::<UciMove>().ok()?.to_move(&board).ok()?;
        let mut boards = vec![board.clone()];
        let mut moves = Vec::new();
        if san.len() > 64 {
            return None;
        }
        for raw in san {
            let m = raw.parse::<San>().ok()?.to_move(&board).ok()?;
            if moves.is_empty() && m != best {
                return None;
            }
            board.play_unchecked(m);
            boards.push(board.clone());
            moves.push(m);
        }
        if moves.is_empty() {
            board.play_unchecked(best);
            boards.push(board);
            moves.push(best);
        }
        Some(Self { boards, moves })
    }
    fn before(&self) -> &Chess {
        &self.boards[0]
    }
    fn after(&self) -> &Chess {
        &self.boards[1]
    }
    fn side(&self) -> Color {
        self.before().turn()
    }
    fn first(&self) -> Move {
        self.moves[0]
    }
    fn gain(&self) -> i32 {
        balance(self.boards.last().unwrap(), self.side()) - balance(self.before(), self.side())
    }
    fn material_win(&self) -> bool {
        let last = self.boards.last().unwrap();
        let gain = self.gain();
        gain >= 2
            && !last.is_stalemate()
            && !(last.is_checkmate() && last.turn() == self.side())
            && (last.turn() == self.side()
                || !last
                    .legal_moves()
                    .iter()
                    .any(|m| m.capture().is_some_and(|r| value(r) >= gain)))
    }
    // Confirm that the first tactical follow-up in the PV takes this exact target.
    fn wins_target(&self, from: Square, target: Square) -> bool {
        self.moves.get(2).is_some_and(|m| {
            m.from() == Some(from)
                && m.to() == target
                && m.is_capture()
                && (value(m.capture().unwrap()) > value(m.role())
                    || !capturable(&self.boards[3], target))
                && balance(&self.boards[3], self.side()) - balance(self.before(), self.side()) >= 2
        }) && self.material_win()
    }
}

fn value(role: Role) -> i32 {
    board::piece_value(role, 100)
}
fn balance(pos: &Chess, side: Color) -> i32 {
    board::material_balance(pos, side, 100)
}
fn attacked(pos: &Chess, sq: Square, by: Color) -> bool {
    pos.board().attacks_to(sq, by, pos.board().occupied()).any()
}
fn capturable(pos: &Chess, sq: Square) -> bool {
    pos.legal_moves()
        .iter()
        .any(|m| m.to() == sq && m.is_capture())
}
fn profitable_capture(pos: &Chess, target: Square) -> bool {
    pos.legal_moves().iter().any(|&m| {
        if m.to() != target || !m.is_capture() {
            return false;
        }
        let mut taken = pos.clone();
        taken.play_unchecked(m);
        value(m.capture().unwrap()) > value(m.role()) || !capturable(&taken, target)
    })
}
/// Whether the piece on `sq` may not leave it because that would expose its own king.
fn pinned(pos: &Chess, sq: Square) -> bool {
    board::leaves_king_exposed(pos, sq, None)
}
fn exposes_king(pos: &Chess, from: Square, to: Square) -> bool {
    board::leaves_king_exposed(pos, from, Some(to))
}
/// Whether `by` can retake on `sq`, judged from attacks rather than legal moves, so a
/// check given by the capture does not hide a defender. Pieces pinned to their king and
/// a king retaking into an attack do not count.
fn defended(pos: &Chess, sq: Square, by: Color) -> bool {
    let board = pos.board();
    board
        .attacks_to(sq, by, board.occupied())
        .into_iter()
        .any(|defender| match board.role_at(defender) {
            Some(Role::King) => !board.attacks_to(sq, !by, board.occupied() ^ defender).any(),
            Some(_) => !exposes_king(pos, defender, sq),
            None => false,
        })
}
fn minor_or_better(role: Role) -> bool {
    !matches!(role, Role::Pawn | Role::King)
}

fn mate(line: &Line) -> Option<&'static str> {
    let last = line.boards.last()?;
    if !last.is_checkmate() || last.turn() == line.side() {
        return None;
    }
    Some(match line.moves.len().div_ceil(2) {
        1 => "mate-in-1",
        2 => "mate-in-2",
        _ => "mate-in-3-plus",
    })
}

fn back_rank_mate(line: &Line) -> bool {
    if mate(line).is_none() {
        return false;
    }
    let pos = line.boards.last().unwrap();
    let loser = pos.turn();
    let king = pos.board().king_of(loser).unwrap();
    let back = if loser == Color::White { 0 } else { 7 };
    if i32::from(king.rank()) != back {
        return false;
    }
    let checker = *line.moves.last().unwrap();
    if !pos.checkers().contains(checker.to())
        || i32::from(checker.to().rank()) != back
        || !matches!(checker.role(), Role::Rook | Role::Queen)
    {
        return false;
    }
    // All squares in front of the king are occupied by its own pawn shield.
    let forward = if loser == Color::White { 1 } else { -1 };
    (-1..=1)
        .filter_map(|df| square(i32::from(king.file()) + df, back + forward))
        .all(|sq| {
            pos.board()
                .piece_at(sq)
                .is_some_and(|p| p.color == loser && p.role == Role::Pawn)
        })
}

fn fork(line: &Line) -> bool {
    let m = line.first();
    let Some(from) = m.from() else {
        return false;
    };
    let pos = line.after();
    if m.is_capture() || m.is_promotion() || capturable(pos, m.to()) {
        return false;
    }
    // A target counts when attacking it is a real threat: the king, a piece worth more
    // than the forking piece, or an undefended one. A defended equal or cheaper piece is not.
    let targets: Vec<_> = (pos.board().attacks_from(m.to()) & pos.board().by_color(!line.side()))
        .into_iter()
        .filter(|&sq| {
            pos.board().role_at(sq).is_some_and(|role| {
                role != Role::Pawn
                    && (role == Role::King
                        || value(role) > value(m.role())
                        || !defended(pos, sq, !line.side()))
            })
        })
        .collect();
    let fresh = targets
        .iter()
        .filter(|&&sq| !line.before().board().attacks_from(from).contains(sq))
        .count();
    targets.len() >= 2 && fresh >= 2 && targets.iter().any(|&sq| line.wins_target(m.to(), sq))
}
fn knight_fork(line: &Line) -> bool {
    line.first().role() == Role::Knight && fork(line)
}

fn square(file: i32, rank: i32) -> Option<Square> {
    ((0..8).contains(&file) && (0..8).contains(&rank))
        .then(|| Square::new((rank * 8 + file) as u32))
}
/// The first two occupied squares along each ray of a slider.
fn rays(pos: &Chess, from: Square) -> Vec<(Square, Square)> {
    let Some(role) = pos.board().role_at(from) else {
        return Vec::new();
    };
    let mut pairs = Vec::new();
    for (df, dr) in [
        (-1, -1),
        (-1, 0),
        (-1, 1),
        (0, -1),
        (0, 1),
        (1, -1),
        (1, 0),
        (1, 1),
    ] {
        let diagonal = df != 0 && dr != 0;
        if !matches!(role, Role::Queen)
            && !(role == Role::Bishop && diagonal || role == Role::Rook && !diagonal)
        {
            continue;
        }
        let mut first = None;
        for n in 1..8 {
            let Some(sq) = square(
                i32::from(from.file()) + df * n,
                i32::from(from.rank()) + dr * n,
            ) else {
                break;
            };
            if pos.board().piece_at(sq).is_some() {
                if let Some(front) = first {
                    pairs.push((front, sq));
                    break;
                }
                first = Some(sq);
            }
        }
    }
    pairs
}

fn pin(line: &Line) -> bool {
    let m = line.first();
    if m.is_capture() || capturable(line.after(), m.to()) {
        return false;
    }
    rays(line.after(), m.to()).into_iter().any(|(front, rear)| {
        let a = line.after().board().piece_at(front).unwrap();
        let b = line.after().board().piece_at(rear).unwrap();
        a.color != line.side() && minor_or_better(a.role) && b.color == a.color && b.role == Role::King
            && line.wins_target(m.to(), front)
            // Exclude simply maintaining a pin that already existed.
            && !rays(line.before(), m.from().unwrap()).contains(&(front, rear))
    })
}
fn skewer(line: &Line) -> bool {
    let m = line.first();
    if m.is_capture() || capturable(line.after(), m.to()) {
        return false;
    }
    rays(line.after(), m.to()).into_iter().any(|(front, rear)| {
        let a = line.after().board().piece_at(front).unwrap();
        let b = line.after().board().piece_at(rear).unwrap();
        a.color != line.side()
            && b.color == a.color
            && minor_or_better(b.role)
            && value(a.role) > value(b.role)
            && line
                .moves
                .get(1)
                .is_some_and(|reply| reply.from() == Some(front))
            && line.wins_target(m.to(), rear)
            && !rays(line.before(), m.from().unwrap()).contains(&(front, rear))
    })
}
fn discovered_attack(line: &Line) -> bool {
    let m = line.first();
    let Some(from) = m.from() else {
        return false;
    };
    line.before()
        .board()
        .by_color(line.side())
        .into_iter()
        .filter(|&sq| sq != from)
        .any(|slider| {
            rays(line.before(), slider)
                .into_iter()
                .any(|(blocker, target)| {
                    // The moved piece must leave the line, not slide along it onto the target.
                    blocker == from
                        && m.to() != target
                        && line.after().board().attacks_from(slider).contains(target)
                        && line.before().board().piece_at(target).is_some_and(|p| {
                            p.color != line.side()
                                && (minor_or_better(p.role) && line.wins_target(slider, target)
                                    || p.role == Role::King && mate(line).is_some())
                        })
                })
        })
}
fn hanging_piece(line: &Line) -> bool {
    let m = line.first();
    let Some(capture) = m.capture() else {
        return false;
    };
    if !minor_or_better(capture) || !line.material_win() {
        return false;
    }
    // Hanging means undefended, as on Lichess. Winning a defended piece with a cheaper
    // one is a material win but not this theme.
    !defended(line.after(), m.to(), !line.side())
}
fn trapped_piece(line: &Line) -> bool {
    let m = line.first();
    let pos = line.after();
    if m.is_capture() || pos.is_check() || capturable(pos, m.to()) {
        return false;
    }
    (pos.board().attacks_from(m.to()) & pos.board().by_color(!line.side()))
        .into_iter()
        .any(|target| {
            pos.board().role_at(target).is_some_and(minor_or_better)
                // A pinned piece with no moves is pinned, not trapped.
                && !pinned(pos, target)
                && line.wins_target(m.to(), target)
                && !line
                    .before()
                    .board()
                    .attacks_from(m.from().unwrap())
                    .contains(target)
                && pos
                    .legal_moves()
                    .iter()
                    .filter(|reply| reply.from() == Some(target))
                    .all(|&reply| {
                        let mut escaped = pos.clone();
                        escaped.play_unchecked(reply);
                        profitable_capture(&escaped, reply.to())
                            && reply
                                .capture()
                                .is_none_or(|r| value(r) < value(reply.role()))
                    })
        })
}
fn promotion(line: &Line) -> bool {
    line.first().is_promotion()
}
fn defensive(line: &Line) -> bool {
    let m = line.first();
    let Some(from) = m.from() else {
        return false;
    };
    if !minor_or_better(m.role())
        || m.is_capture()
        || m.is_promotion()
        || line.moves.len() < 2
        || line.gain() != 0
        || mate(line).is_some()
        || line.moves.iter().any(|m| m.is_capture())
        || attacked(line.before(), from, line.side())
        || attacked(line.after(), m.to(), !line.side())
    {
        return false;
    }
    line.before()
        .clone()
        .swap_turn()
        .is_ok_and(|opponent| capturable(&opponent, from))
}

pub fn detect(fen: &str, best_uci: &str, line_san: &[String]) -> Vec<&'static str> {
    let Some(line) = Line::parse(fen, best_uci, line_san) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    if let Some(theme) = mate(&line) {
        out.push(theme);
    }
    for (theme, found) in [
        ("back-rank-mate", back_rank_mate(&line)),
        ("fork", fork(&line)),
        ("knight-fork", knight_fork(&line)),
        ("pin", pin(&line)),
        ("skewer", skewer(&line)),
        ("discovered-attack", discovered_attack(&line)),
        ("hanging-piece", hanging_piece(&line)),
        ("trapped-piece", trapped_piece(&line)),
        ("promotion", promotion(&line)),
        ("defensive", defensive(&line)),
    ] {
        if found {
            out.push(theme);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Hand-made positions; use legal UCI replay to construct stored SAN exactly.
    fn line(fen: &str, pv: &[&str]) -> Line {
        let mut pos = pgn::board(fen, false).unwrap();
        let mut san = Vec::new();
        for raw in pv {
            let m = raw
                .parse::<UciMove>()
                .unwrap()
                .to_move(&pos)
                .unwrap_or_else(|_| panic!("illegal {raw} in {}", pgn::fen(&pos)));
            san.push(San::from_move(&pos, m).to_string());
            pos.play_unchecked(m);
        }
        Line::parse(fen, pv[0], &san).unwrap()
    }
    fn quiet() -> Line {
        line(pgn::STARTING_FEN, &["e2e4", "e7e5", "g1f3"])
    }

    #[test]
    fn mates_require_checkmate_by_the_puzzle_side() {
        let one = line("7k/8/5K2/5Q2/8/8/8/8 w - - 0 1", &["f5g6"]);
        // Qg6 is stalemate, not mate.
        assert_eq!(mate(&one), None);
        let one = line("7k/5Q2/5K2/8/8/8/8/8 w - - 0 1", &["f7g7"]);
        assert_eq!(mate(&one), Some("mate-in-1"));
        let two = line("7k/8/5K2/4Q3/8/8/8/8 w - - 0 1", &["e5g3", "h8h7", "g3g7"]);
        assert_eq!(mate(&two), Some("mate-in-2"));
        let three = line(
            "7k/8/5K2/8/8/8/8/Q7 w - - 0 1",
            &["a1e5", "h8h7", "e5g3", "h7h8", "g3g7"],
        );
        assert_eq!(mate(&three), Some("mate-in-3-plus"));
        assert_eq!(mate(&quiet()), None);
        assert_eq!(mate(&line("7k/8/8/8/8/8/8/R6K w - - 0 1", &["a1a8"])), None);
        let lost = line(pgn::STARTING_FEN, &["f2f3", "e7e5", "g2g4", "d8h4"]);
        assert_eq!(mate(&lost), None);
    }

    #[test]
    fn back_rank_requires_mate_and_own_pawn_confinement() {
        assert!(back_rank_mate(&line(
            "6k1/5ppp/8/8/8/8/8/R6K w - - 0 1",
            &["a1a8"]
        )));
        assert!(back_rank_mate(&line(
            "r6k/8/8/8/8/8/5PPP/6K1 b - - 0 1",
            &["a8a1"]
        )));
        assert!(!back_rank_mate(&quiet()));
        assert!(!back_rank_mate(&line(
            "7k/5Q2/5K2/8/8/8/8/8 w - - 0 1",
            &["f7g7"]
        )));
    }

    #[test]
    fn forks_require_new_targets_and_a_material_follow_up() {
        let knight = line(
            "2r3k1/8/8/5N2/8/8/8/6K1 w - - 0 1",
            &["f5e7", "g8f7", "e7c8"],
        );
        assert!(fork(&knight));
        assert!(knight_fork(&knight));
        let pawn = line(
            "7k/8/3n1r2/8/4P3/8/8/K7 w - - 0 1",
            &["e4e5", "f6f7", "e5d6"],
        );
        assert!(fork(&pawn));
        assert!(!knight_fork(&pawn));
        assert!(!fork(&quiet()));
        assert!(!knight_fork(&quiet()));
        assert!(!fork(&line("2r3k1/8/8/5N2/8/8/8/6K1 w - - 0 1", &["f5e7"])));
        // A geometric fork whose knight is simply capturable is not a theme.
        assert!(!fork(&line(
            "2r2bk1/8/8/5N2/8/8/8/6K1 w - - 0 1",
            &["f5e7", "g8f7", "e7c8"]
        )));
    }

    #[test]
    fn pins_require_an_absolute_pin_and_a_win() {
        let fen = "4k3/7p/8/4b3/8/8/8/R6K w - - 0 1";
        assert!(pin(&line(fen, &["a1e1", "h7h6", "e1e5"])));
        assert!(!pin(&line(fen, &["a1e1"])));
        assert!(!pin(&quiet()));
        // Same alignment with a rook behind the bishop is not an absolute pin.
        assert!(!pin(&line(
            "4r2k/7p/8/4b3/8/8/8/R6K w - - 0 1",
            &["a1e1", "h7h6", "e1e5"]
        )));
    }

    #[test]
    fn skewers_require_a_more_valuable_front_piece_to_move() {
        let fen = "8/8/5r2/4k3/8/8/8/2B4K w - - 0 1";
        assert!(skewer(&line(fen, &["c1b2", "e5e4", "b2f6"])));
        assert!(!skewer(&line(fen, &["c1b2"])));
        assert!(!skewer(&quiet()));
        assert!(!skewer(&line(
            "7k/8/5r2/4b3/8/8/8/2B4K w - - 0 1",
            &["c1b2", "e5f4", "b2f6"]
        )));
    }

    #[test]
    fn discovered_attacks_require_the_vacated_blocker_and_follow_up() {
        let fen = "q7/8/7k/8/8/B7/8/R5K1 w - - 0 1";
        assert!(discovered_attack(&line(fen, &["a3c1", "h6h7", "a1a8"])));
        assert!(!discovered_attack(&line(fen, &["a3c1"])));
        assert!(!discovered_attack(&quiet()));
        // Moving along the rook's file leaves the attack blocked.
        assert!(!discovered_attack(&line(
            "q7/8/7k/8/8/R7/8/R5K1 w - - 0 1",
            &["a3a5", "h6h7"]
        )));
    }

    #[test]
    fn hanging_piece_requires_profitable_non_pawn_capture() {
        assert!(hanging_piece(&line(
            "7k/8/n7/8/8/8/8/R6K w - - 0 1",
            &["a1a6"]
        )));
        // Winning a defended queen with a bishop wins material, but the queen was not hanging.
        assert!(!hanging_piece(&line(
            "7k/8/qr6/8/8/8/8/5B1K w - - 0 1",
            &["f1a6", "b6a6"]
        )));
        assert!(!hanging_piece(&quiet()));
        assert!(!hanging_piece(&line(
            "7k/8/rr6/8/8/8/8/R6K w - - 0 1",
            &["a1a6", "b6a6"]
        )));
        // Truncation just before an obvious recapture must not invent a win.
        assert!(!hanging_piece(&line(
            "7k/8/nr6/8/8/8/8/R6K w - - 0 1",
            &["a1a6"]
        )));
        assert!(!hanging_piece(&line(
            "7k/8/r7/8/R7/8/7K/r7 w - - 0 1",
            &["a4a6"]
        )));
        // A later unrelated queen win does not make the initial exchange free.
        assert!(!hanging_piece(&line(
            "7k/8/r7/7q/R6R/8/7K/r7 w - - 0 1",
            &["a4a6", "a1a6", "h4h5"]
        )));
    }

    #[test]
    fn trapped_piece_requires_no_profitable_escape() {
        let fen = "6k1/1n6/b7/1P6/2P5/7R/8/7K w - - 0 1";
        assert!(trapped_piece(&line(fen, &["h3a3", "g8f8", "a3a6"])));
        assert!(!trapped_piece(&quiet()));
        assert!(!trapped_piece(&line(fen, &["h3a3"])));
        // Remove the pawn shield: the bishop can leave along b7.
        assert!(!trapped_piece(&line(
            "6k1/8/b7/1P6/2P5/7R/8/7K w - - 0 1",
            &["h3a3", "g8f8", "a3a6"]
        )));
        // Pinned defenders cannot make the bishop's escape squares unsafe.
        assert!(!trapped_piece(&line(
            "4r2k/1n6/b7/8/8/8/4B3/2R1K3 w - - 0 1",
            &["c1a1", "h8g8", "a1a6"]
        )));
    }

    #[test]
    fn promotion_only_labels_the_answer_move() {
        assert!(promotion(&line("8/P7/8/8/8/8/8/k6K w - - 0 1", &["a7a8n"])));
        assert!(promotion(&line("7k/8/8/8/8/8/p7/7K b - - 0 1", &["a2a1q"])));
        assert!(!promotion(&quiet()));
    }

    #[test]
    fn defensive_requires_a_threatened_piece_saved_without_winning_material() {
        let fen = "7k/8/8/8/8/2b5/8/R6K w - - 0 1";
        assert!(defensive(&line(fen, &["a1a2", "c3d4"])));
        assert!(!defensive(&line(fen, &["a1e1", "c3d4"])));
        assert!(!defensive(&quiet()));
        assert!(!defensive(&line(
            "7k/8/8/8/8/8/8/R6K w - - 0 1",
            &["a1a2", "h8g8"]
        )));
    }

    #[test]
    fn invalid_or_mismatched_stored_lines_do_not_produce_labels() {
        assert!(detect("broken", "e2e4", &[]).is_empty());
        assert!(detect(pgn::STARTING_FEN, "e2e4", &["d4".into()]).is_empty());
        assert!(detect(pgn::STARTING_FEN, "e2e4", &["e4".into(), "Qa1".into()]).is_empty());
        assert!(detect(pgn::STARTING_FEN, "e2e5", &[]).is_empty());
    }

    fn themes(fen: &str, best: &str, san: &[&str]) -> Vec<&'static str> {
        detect(
            fen,
            best,
            &san.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        )
    }

    // Positions from real reviewed games that an earlier detector mislabelled.
    #[test]
    fn real_positions_that_were_mislabelled() {
        // The queen slides up the file and takes the queen it was blocking: nothing is discovered.
        let t = themes(
            "rn2k2r/p3npp1/2pb4/4q3/2p3Pp/2N1Q3/PPPB1P1P/R3R1K1 w kq - 1 18",
            "e3e5",
            &["Qxe5", "Bxe5", "Rxe5"],
        );
        assert!(!t.contains(&"discovered-attack"), "{t:?}");
        // The knight on e7 is pinned, not trapped.
        let t = themes(
            "r3k2r/pp2n2p/1np5/6B1/3P2q1/6P1/PPPQ1R1P/R5K1 w kq - 0 21",
            "a1e1",
            &["Re1", "O-O-O", "Rxe7"],
        );
        assert!(!t.contains(&"trapped-piece"), "{t:?}");
        // Qd4 hits Ra1 and Ng1, but Rh1 defends the knight: a boxed-in rook, not a fork.
        let t = themes(
            "rnbqkb1r/pp3ppp/2p2n2/4p3/2P1P3/1P1P4/P5PP/RNBQKBNR b KQkq - 0 6",
            "d8d4",
            &["Qd4", "Qc2", "Qxa1"],
        );
        assert!(!t.contains(&"fork"), "{t:?}");
        // Nc6+ wins the queen by the discovered check; the defended b8 knight is no second target.
        let t = themes(
            "rn1qk2r/1p3ppp/5n2/1pbpN3/8/8/PPPBQPPP/R3K2R w KQkq - 0 12",
            "e5c6",
            &["Nc6+", "Kd7", "Nxd8"],
        );
        assert!(!t.contains(&"fork") && !t.contains(&"knight-fork"), "{t:?}");
        // Nc3 defends b5; the recapture is only illegal because of the check from Bc5.
        let t = themes(
            "r2qk2r/pp3ppp/4pn2/1Bbp4/3n1P2/2N2P1P/PPP5/R1BQ1RK1 b kq - 2 11",
            "d4b5",
            &["Nxb5+", "Kg2", "Nxc3"],
        );
        assert!(!t.contains(&"hanging-piece"), "{t:?}");
    }
    // A defender pinned by the capturing piece can still take it back
    // along the pin line, so the captured piece was not hanging.
    #[test]
    fn a_defender_pinned_by_the_capturer_still_defends() {
        let t = themes(
            "2kq1b1r/ppp2ppp/2n1b3/4p3/4P1n1/2NP4/PPP1BPPP/R2Q1RK1 w - - 0 11",
            "e2g4",
            &["Bxg4", "h5", "Bxe6+", "fxe6", "Ne2", "g5"],
        );
        assert!(!t.contains(&"hanging-piece"), "{t:?}");
        let t = themes(
            "rn2kb1r/pp4pp/1qp3b1/4p1Pn/4p3/1NN5/PPP1BP1P/R1BQK2R w KQkq - 1 12",
            "e2h5",
            &["Bxh5", "Bc5", "Bxg6+", "hxg6", "Nxc5", "Qxc5"],
        );
        assert!(!t.contains(&"hanging-piece"), "{t:?}");
    }
}
