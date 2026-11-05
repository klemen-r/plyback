//! Opening repertoire: the account's games as a move tree for one colour.
//!
//! Each node is a move sequence from the standard starting position, with the
//! results of the games that followed it, how often the move was book, and,
//! for the account's own moves, how often it was an error. Only reviewed games
//! contribute (the tree is built from stored move rows), and coverage is
//! reported. Move sequences are kept apart even when they transpose, so the
//! tree matches the order the moves were played in.
use crate::{ApiResult, Query, api_error, db::Database};
use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use shakmaty::{Chess, Position, san::San};
use std::collections::BTreeMap;

pub const DEFAULT_PLIES: i64 = 16;
const STANDARD_START: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";
const DRAWS: [&str; 6] = [
    "agreed",
    "repetition",
    "stalemate",
    "insufficient",
    "50move",
    "timevsinsufficient",
];
const ERRORS: [&str; 4] = ["inaccuracy", "mistake", "blunder", "miss"];
const TIME_CLASSES: [&str; 4] = ["bullet", "blitz", "rapid", "daily"];
/// A move needs this many games before it can be listed as a problem.
pub const PROBLEM_MIN_GAMES: i64 = 3;

#[derive(Default, Debug)]
struct Node {
    san: String,
    ply: i64,
    games: i64,
    wins: i64,
    draws: i64,
    losses: i64,
    last_played: i64,
    book: i64,
    errors: i64,
    blunders: i64,
    misses: i64,
    loss_sum: f64,
    loss_n: i64,
    left_book: i64,
    /// Named opening at the position after this move, if the dataset names it.
    opening: Option<(String, String)>,
    /// The most recent games through this move: (end time, game id).
    recent: Vec<(i64, i64)>,
}
/// How many recent games each move links to.
const RECENT_GAMES: usize = 3;

struct MoveRow {
    game: i64,
    end_time: i64,
    score: f64,
    ply: i64,
    san: String,
    is_book: bool,
    classification: Option<String>,
    ep_loss: Option<f64>,
}

fn outcome(mine: &str, theirs: &str) -> f64 {
    if mine == "win" {
        1.0
    } else if DRAWS.contains(&mine) || DRAWS.contains(&theirs) {
        0.5
    } else {
        0.0
    }
}

pub fn compute(
    db: &Database,
    account: i64,
    color: &str,
    plies: i64,
    time_class: Option<&str>,
    since: Option<i64>,
) -> Result<Value> {
    let mine = if color == "white" { "w" } else { "b" };
    let (rows, total) = db.read(|c| {
        let total: i64 = c.query_row(
            "SELECT COUNT(*) FROM account_games ag JOIN games g ON g.id = ag.game_id
             WHERE ag.account_id = ?1 AND ag.color = ?2 AND g.rules = 'chess'
               AND (g.initial_fen IS NULL OR g.initial_fen = '' OR g.initial_fen = ?3)
               AND (?4 IS NULL OR g.time_class = ?4) AND (?5 IS NULL OR g.end_time >= ?5)",
            params![account, color, STANDARD_START, time_class, since],
            |r| r.get(0),
        )?;
        let mut statement = c.prepare_cached(
            "SELECT g.id, COALESCE(g.end_time, 0),
                    CASE WHEN ag.color = 'white' THEN g.white_result ELSE g.black_result END,
                    CASE WHEN ag.color = 'white' THEN g.black_result ELSE g.white_result END,
                    m.ply, m.san, COALESCE(m.is_book, 0), m.classification, m.ep_loss
             FROM account_games ag
             JOIN games g ON g.id = ag.game_id
             JOIN moves m ON m.game_id = g.id
             WHERE ag.account_id = ?1 AND ag.color = ?2 AND g.rules = 'chess'
               AND (g.initial_fen IS NULL OR g.initial_fen = '' OR g.initial_fen = ?3)
               AND (?4 IS NULL OR g.time_class = ?4) AND (?5 IS NULL OR g.end_time >= ?5)
               AND m.ply <= ?6
             ORDER BY g.id, m.ply",
        )?;
        let rows = statement
            .query_map(
                params![account, color, STANDARD_START, time_class, since, plies],
                |r| {
                    let mine: Option<String> = r.get(2)?;
                    let theirs: Option<String> = r.get(3)?;
                    Ok(MoveRow {
                        game: r.get(0)?,
                        end_time: r.get(1)?,
                        score: outcome(
                            mine.as_deref().unwrap_or(""),
                            theirs.as_deref().unwrap_or(""),
                        ),
                        ply: r.get(4)?,
                        san: r.get(5)?,
                        is_book: r.get::<_, i64>(6)? != 0,
                        classification: r.get(7)?,
                        ep_loss: r.get(8)?,
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((rows, total))
    })?;

    let mut nodes: BTreeMap<String, Node> = BTreeMap::new();
    let (mut games, mut wins, mut draws, mut losses) = (0, 0, 0, 0);
    let mut start = 0;
    while start < rows.len() {
        let game = rows[start].game;
        let end = rows[start..]
            .iter()
            .position(|r| r.game != game)
            .map_or(rows.len(), |n| start + n);
        let moves = &rows[start..end];
        start = end;
        // A game only contributes the contiguous moves from ply 1.
        let contiguous = moves
            .iter()
            .enumerate()
            .take_while(|(i, r)| r.ply == *i as i64 + 1)
            .count();
        let moves = &moves[..contiguous];
        let Some(first) = moves.first() else { continue };
        let score = first.score;
        games += 1;
        match score {
            s if s >= 1.0 => wins += 1,
            s if s > 0.0 => draws += 1,
            _ => losses += 1,
        }
        let mut path = String::new();
        let mut in_book = true;
        let mut position = Chess::default();
        for m in moves {
            // Follow the game on a board to name the positions; stop at anything unreadable.
            let Some(played) = m
                .san
                .parse::<San>()
                .ok()
                .and_then(|san| san.to_move(&position).ok())
            else {
                break;
            };
            position.play_unchecked(played);
            if !path.is_empty() {
                path.push(' ');
            }
            path.push_str(&m.san);
            let node = nodes.entry(path.clone()).or_default();
            if node.games == 0 {
                node.opening = crate::analysis::opening_at(&crate::pgn::epd(&position));
            }
            node.recent.push((m.end_time, m.game));
            node.recent.sort_unstable_by(|a, b| b.cmp(a));
            node.recent.truncate(RECENT_GAMES);
            node.san.clone_from(&m.san);
            node.ply = m.ply;
            node.games += 1;
            match score {
                s if s >= 1.0 => node.wins += 1,
                s if s > 0.0 => node.draws += 1,
                _ => node.losses += 1,
            }
            node.last_played = node.last_played.max(m.end_time);
            if m.is_book {
                node.book += 1;
            } else if in_book {
                node.left_book += 1;
                in_book = false;
            }
            let own = (m.ply % 2 == 1) == (mine == "w");
            if own {
                if m.classification
                    .as_deref()
                    .is_some_and(|c| ERRORS.contains(&c))
                {
                    node.errors += 1;
                }
                match m.classification.as_deref() {
                    Some("blunder") => node.blunders += 1,
                    Some("miss") => node.misses += 1,
                    _ => {}
                }
                if let Some(loss) = m.ep_loss {
                    node.loss_sum += loss;
                    node.loss_n += 1;
                }
            }
        }
    }

    let score = |n: &Node| (n.wins as f64 + 0.5 * n.draws as f64) / n.games.max(1) as f64;
    let own = |n: &Node| (n.ply % 2 == 1) == (mine == "w");
    let public = |path: &str, n: &Node| {
        json!({
            "path": path,
            "san": n.san,
            "ply": n.ply,
            "mine": own(n),
            "games": n.games,
            "wins": n.wins,
            "draws": n.draws,
            "losses": n.losses,
            "score": score(n),
            "last_played": n.last_played,
            "book": n.book,
            "left_book": n.left_book,
            "eco": n.opening.as_ref().map(|o| o.0.clone()),
            "opening": n.opening.as_ref().map(|o| o.1.clone()),
            "recent_games": n.recent.iter().map(|r| r.1).collect::<Vec<_>>(),
            "errors": if own(n) { json!(n.errors) } else { Value::Null },
            "blunders": if own(n) { json!(n.blunders) } else { Value::Null },
            "misses": if own(n) { json!(n.misses) } else { Value::Null },
            "avg_loss": if own(n) && n.loss_n > 0 { json!(n.loss_sum / n.loss_n as f64) } else { Value::Null },
        })
    };
    // Own moves that went wrong repeatedly: an error in at least half of the
    // games, or a score of 35% or less over four or more games.
    let mut problems: Vec<(&String, &Node)> = nodes
        .iter()
        .filter(|(_, n)| {
            own(n)
                && n.games >= PROBLEM_MIN_GAMES
                && (n.errors * 2 >= n.games || (n.games >= 4 && score(n) <= 0.35))
        })
        .collect();
    problems.sort_by(|a, b| {
        (b.1.errors, b.1.losses, b.1.games)
            .cmp(&(a.1.errors, a.1.losses, a.1.games))
            .then_with(|| a.0.cmp(b.0))
    });
    Ok(json!({
        "color": color,
        "plies": plies,
        "games": games,
        "total_games": total,
        "wins": wins,
        "draws": draws,
        "losses": losses,
        "nodes": nodes.iter().map(|(p, n)| public(p, n)).collect::<Vec<_>>(),
        "problems": problems.iter().take(8).map(|(p, n)| public(p, n)).collect::<Vec<_>>(),
    }))
}

pub fn handle(db: &Database, method: &str, path: &str, q: &Query) -> Option<ApiResult> {
    let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
    let ["api", "accounts", account, "repertoire"] = parts.as_slice() else {
        return None;
    };
    Some((|| {
        if method != "GET" {
            return Err(api_error(405, "Method not allowed"));
        }
        let account = account
            .parse::<i64>()
            .ok()
            .filter(|v| *v > 0)
            .ok_or_else(|| api_error(422, "account_id must be a positive integer"))?;
        let exists = db.read(|c| {
            Ok(
                c.query_row("SELECT 1 FROM accounts WHERE id=?", [account], |_| Ok(()))
                    .optional()?,
            )
        })?;
        exists.ok_or_else(|| api_error(404, "Account not found"))?;
        let color = match q.get("color").map(String::as_str) {
            None | Some("white") => "white",
            Some("black") => "black",
            Some(_) => return Err(api_error(422, "color must be white or black")),
        };
        let plies = match q.get("plies") {
            None => DEFAULT_PLIES,
            Some(raw) => raw
                .parse::<i64>()
                .ok()
                .filter(|n| (2..=30).contains(n))
                .ok_or_else(|| api_error(422, "plies must be between 2 and 30"))?,
        };
        let time_class = match q.get("time_class").map(String::as_str) {
            None | Some("") | Some("all") => None,
            Some(t) if TIME_CLASSES.contains(&t) => Some(t),
            Some(_) => return Err(api_error(422, "Invalid time_class")),
        };
        let since = match q.get("since").map(String::as_str) {
            None | Some("") => None,
            Some(raw) => Some(
                raw.parse::<i64>()
                    .map_err(|_| api_error(422, "since must be a Unix timestamp"))?,
            ),
        };
        Ok((200, compute(db, account, color, plies, time_class, since)?))
    })())
}
