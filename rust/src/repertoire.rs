//! Opening repertoire: the account's games as a move tree for one colour.
//!
//! Each node is a move sequence from the standard starting position, with the
//! results of the games that followed it, how often the move was book, and,
//! for the account's own moves, how often it was an error. Only reviewed games
//! contribute (the tree is built from stored move rows), and coverage is
//! reported. By default move sequences are kept apart even when they
//! transpose, so the tree matches the order the moves were played in. With
//! `merge`, moves are grouped by the position they were played from, so every
//! move order that reaches a position shares its statistics.
use crate::{ApiResult, Query, api_error, db::Database};
use anyhow::Result;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use shakmaty::{Chess, Position, san::San};
use std::collections::{BTreeMap, HashMap, HashSet};

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
    /// The most recent games through this move: (end time, game id, ply).
    recent: Vec<(i64, i64, i64)>,
    /// Position ids before and after the move (see `compute`).
    from: usize,
    to: usize,
    /// Games per move order that led here; the most common one is shown.
    paths: BTreeMap<String, i64>,
    /// A game repeating a position counts once per move.
    last_game: i64,
}
/// How many recent games each move links to.
const RECENT_GAMES: usize = 3;

/// Incoming edges overlap when a game returns to a position by another move.
#[derive(Default)]
struct Reached {
    games: i64,
    wins: i64,
    draws: i64,
    losses: i64,
    last_game: i64,
    paths: HashSet<String>,
}
impl Reached {
    fn visit(&mut self, game: i64, score: f64, path: &str) {
        self.paths.insert(path.to_owned());
        // Rows are grouped by game, so the last id suffices to deduplicate.
        if self.games > 0 && self.last_game == game {
            return;
        }
        self.last_game = game;
        self.games += 1;
        match score {
            s if s >= 1.0 => self.wins += 1,
            s if s > 0.0 => self.draws += 1,
            _ => self.losses += 1,
        }
    }
}

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
    merge: bool,
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

    // Moves are edges between positions. A position is identified by its move
    // sequence, or with `merge` by its EPD; ids are small integers, 0 is the start.
    let mut ids: HashMap<String, usize> = HashMap::new();
    let mut id_of = |key: String| {
        let next = ids.len();
        *ids.entry(key).or_insert(next)
    };
    let root = id_of(if merge {
        crate::pgn::epd(&Chess::default())
    } else {
        String::new()
    });
    let mut nodes: HashMap<(usize, String), Node> = HashMap::new();
    let mut reached: BTreeMap<usize, Reached> = BTreeMap::new();
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
        let mut at = root;
        reached.entry(root).or_default().visit(game, score, "");
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
            let epd = crate::pgn::epd(&position);
            let to = id_of(if merge { epd.clone() } else { path.clone() });
            reached.entry(to).or_default().visit(game, score, &path);
            let from = std::mem::replace(&mut at, to);
            let node = nodes.entry((from, m.san.clone())).or_default();
            if node.games > 0 && node.last_game == m.game {
                continue;
            }
            if node.games == 0 {
                node.opening = crate::analysis::opening_at(&epd);
                node.from = from;
                node.to = to;
            }
            node.last_game = m.game;
            *node.paths.entry(path.clone()).or_default() += 1;
            node.recent.push((m.end_time, m.game, m.ply));
            node.recent.sort_unstable_by(|a, b| b.cmp(a));
            node.recent.truncate(RECENT_GAMES);
            node.san.clone_from(&m.san);
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

    // Each move is shown under its most common move order (the shortest on a tie),
    // and listed in that order for stable output.
    let mut nodes: Vec<(String, Node)> = nodes
        .into_values()
        .map(|mut n| {
            let path = n
                .paths
                .iter()
                .max_by(|a, b| {
                    a.1.cmp(b.1)
                        .then_with(|| b.0.len().cmp(&a.0.len()))
                        .then_with(|| b.0.cmp(a.0))
                })
                .map(|(p, _)| p.clone())
                .unwrap_or_default();
            n.ply = path.split(' ').count() as i64;
            (path, n)
        })
        .collect();
    nodes.sort_by(|a, b| a.0.cmp(&b.0));
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
            "recent_plies": n.recent.iter().map(|r| r.2).collect::<Vec<_>>(),
            "from": n.from,
            "to": n.to,
            "orders": n.paths.len(),
            "errors": if own(n) { json!(n.errors) } else { Value::Null },
            "blunders": if own(n) { json!(n.blunders) } else { Value::Null },
            "misses": if own(n) { json!(n.misses) } else { Value::Null },
            "avg_loss": if own(n) && n.loss_n > 0 { json!(n.loss_sum / n.loss_n as f64) } else { Value::Null },
        })
    };
    // Own moves that went wrong repeatedly: an error in at least half of the
    // games, or a score of 35% or less over four or more games.
    let mut problems: Vec<&(String, Node)> = nodes
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
            .then_with(|| a.0.cmp(&b.0))
    });
    Ok(json!({
        "color": color,
        "plies": plies,
        "merged": merge,
        "root": root,
        "games": games,
        "total_games": total,
        "wins": wins,
        "draws": draws,
        "losses": losses,
        "nodes": nodes.iter().map(|(p, n)| public(p, n)).collect::<Vec<_>>(),
        "positions": reached.iter().map(|(id, p)| json!({
            "id": id,
            "games": p.games,
            "wins": p.wins,
            "draws": p.draws,
            "losses": p.losses,
            "score": (p.wins as f64 + 0.5 * p.draws as f64) / p.games.max(1) as f64,
            "orders": p.paths.len(),
        })).collect::<Vec<_>>(),
        "problems": problems.iter().take(8).map(|e| public(&e.0, &e.1)).collect::<Vec<_>>(),
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
        let merge = match q.get("merge").map(String::as_str) {
            None | Some("") | Some("false") | Some("0") => false,
            Some("true") | Some("1") => true,
            Some(_) => return Err(api_error(422, "merge must be true or false")),
        };
        Ok((
            200,
            compute(db, account, color, plies, time_class, since, merge)?,
        ))
    })())
}
