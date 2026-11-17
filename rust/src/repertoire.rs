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
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::{Arc, Condvar, Mutex},
    time::{Duration, Instant},
};

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
    fn visit(&mut self, game: i64, score: Option<f64>, path: &str) {
        // Rows are grouped by game, so the last id suffices to deduplicate. A game that
        // comes back to the position (a repetition) is not another move order either.
        if self.games > 0 && self.last_game == game {
            return;
        }
        self.paths.insert(path.to_owned());
        self.last_game = game;
        self.games += 1;
        tally(score, &mut self.wins, &mut self.draws, &mut self.losses);
    }
}

/// Counts a result; a game without one (PGN `*`) is a game but no win, draw or loss.
fn tally(score: Option<f64>, wins: &mut i64, draws: &mut i64, losses: &mut i64) {
    match score {
        Some(s) if s >= 1.0 => *wins += 1,
        Some(s) if s > 0.0 => *draws += 1,
        Some(_) => *losses += 1,
        None => {}
    }
}

/// Points per decided game; games without a result are left out.
fn score_of(wins: i64, draws: i64, losses: i64) -> f64 {
    (wins as f64 + 0.5 * draws as f64) / (wins + draws + losses).max(1) as f64
}

struct MoveRow {
    game: i64,
    end_time: i64,
    score: Option<f64>,
    ply: i64,
    san: String,
    is_book: bool,
    classification: Option<String>,
    ep_loss: Option<f64>,
    epd_before: String,
}

fn outcome(mine: &str, theirs: &str) -> Option<f64> {
    if mine.is_empty() && theirs.is_empty() {
        None
    } else if mine == "win" {
        Some(1.0)
    } else if DRAWS.contains(&mine) || DRAWS.contains(&theirs) {
        Some(0.5)
    } else {
        Some(0.0)
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
    compute_proven(
        db,
        &Request {
            account,
            color: color.to_owned(),
            plies,
            time_class: time_class.map(str::to_owned),
            since,
            merge,
        },
        None,
    )
    .map(|(data, _)| data)
}

type TransitionProofs = HashSet<(i64, i64)>;
type Proof<'a> = (i64, &'a mut TransitionProofs);

fn compute_proven(
    db: &Database,
    request: &Request,
    proof: Option<Proof<'_>>,
) -> Result<(Value, usize)> {
    let account = request.account;
    let color = request.color.as_str();
    let plies = request.plies;
    let time_class = request.time_class.as_deref();
    let since = request.since;
    let merge = request.merge;
    let _compute =
        tracing::info_span!("repertoire.compute", account, color, plies, merge).entered();
    let mine = if color == "white" { "w" } else { "b" };
    let load = tracing::info_span!("repertoire.load").entered();
    let (rows, total, revision) = db.read(|c| {
        let revision: i64 =
            c.query_row("SELECT revision FROM data_revision WHERE id=1", [], |r| {
                r.get(0)
            })?;
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
                    m.ply, m.san, COALESCE(m.is_book, 0), m.classification, m.ep_loss, m.epd_before
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
                        epd_before: r.get::<_, Option<String>>(9)?.unwrap_or_default(),
                    })
                },
            )?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok((rows, total, revision))
    })?;
    // The revision and rows share one read lease. A write between the caller
    // reading its cache key and this load must not apply proofs to newer data.
    let mut proven = proof.and_then(|(expected, proven)| (expected == revision).then_some(proven));

    drop(load);
    let aggregate = tracing::info_span!("repertoire.aggregate").entered();
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
    let mut replays = 0;
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
        tally(score, &mut wins, &mut draws, &mut losses);
        let mut path = String::new();
        let mut in_book = true;
        let mut position = Some(Chess::default());
        let mut last_epd = crate::pgn::epd(&Chess::default());
        let mut at = root;
        reached.entry(root).or_default().visit(game, score, "");
        let _replay = tracing::debug_span!("repertoire.replay", game).entered();
        for (i, m) in moves.iter().enumerate() {
            let stored_after = moves.get(i + 1).map(|next| next.epd_before.as_str());
            // A previous computation at this exact database revision has proved
            // this stored transition equals legal SAN replay. Unproven, missing,
            // malformed and terminal rows always retain the original replay path.
            let validated = stored_after.filter(|epd| {
                !epd.is_empty()
                    && proven
                        .as_deref()
                        .is_some_and(|p| p.contains(&(game, m.ply)))
            });
            let epd = if let Some(stored) = validated {
                position = None;
                stored.to_owned()
            } else {
                if position.is_none() {
                    position = Some(
                        crate::pgn::board(&format!("{last_epd} 0 1"), false).unwrap_or_else(|_| {
                            // A stored position the board will not set up again: replay this
                            // game's earlier moves instead, as an uncached computation would.
                            let mut board = Chess::default();
                            for earlier in &moves[..i] {
                                let Some(played) = earlier
                                    .san
                                    .parse::<San>()
                                    .ok()
                                    .and_then(|san| san.to_move(&board).ok())
                                else {
                                    break;
                                };
                                board.play_unchecked(played);
                            }
                            board
                        }),
                    );
                }
                replays += 1;
                let board = position.as_mut().unwrap();
                let Some(played) = m
                    .san
                    .parse::<San>()
                    .ok()
                    .and_then(|san| san.to_move(board).ok())
                else {
                    break;
                };
                board.play_unchecked(played);
                let epd = crate::pgn::epd(board);
                if stored_after == Some(epd.as_str())
                    && let Some(proven) = proven.as_deref_mut()
                    && proven.len() < PROVEN_TRANSITIONS
                {
                    proven.insert((game, m.ply));
                }
                epd
            };
            last_epd.clone_from(&epd);
            if !path.is_empty() {
                path.push(' ');
            }
            path.push_str(&m.san);
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
            tally(score, &mut node.wins, &mut node.draws, &mut node.losses);
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

    drop(aggregate);
    let _public = tracing::info_span!("repertoire.public").entered();
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
    let score = |n: &Node| score_of(n.wins, n.draws, n.losses);
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
                && (n.errors * 2 >= n.games
                    || (n.wins + n.draws + n.losses >= 4 && score(n) <= 0.35))
        })
        .collect();
    problems.sort_by(|a, b| {
        (b.1.errors, b.1.losses, b.1.games)
            .cmp(&(a.1.errors, a.1.losses, a.1.games))
            .then_with(|| a.0.cmp(&b.0))
    });
    tracing::debug!(replays, "Repertoire SAN moves replayed");
    Ok((
        json!({
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
                "score": score_of(p.wins, p.draws, p.losses),
                "orders": p.paths.len(),
            })).collect::<Vec<_>>(),
            "problems": problems.iter().take(8).map(|e| public(&e.0, &e.1)).collect::<Vec<_>>(),
        }),
        replays,
    ))
}

fn request(db: &Database, method: &str, path: &str, q: &Query) -> Option<Result<Request>> {
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
        Ok(Request {
            account,
            color: color.to_owned(),
            plies,
            time_class: time_class.map(str::to_owned),
            since,
            merge,
        })
    })())
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Request {
    account: i64,
    color: String,
    plies: i64,
    time_class: Option<String>,
    since: Option<i64>,
    merge: bool,
}
impl Request {
    fn compute(&self, db: &Database) -> Result<Value> {
        compute(
            db,
            self.account,
            &self.color,
            self.plies,
            self.time_class.as_deref(),
            self.since,
            self.merge,
        )
    }
}

pub fn handle(db: &Database, method: &str, path: &str, q: &Query) -> Option<ApiResult> {
    request(db, method, path, q).map(|r| r.and_then(|r| Ok((200, r.compute(db)?))))
}

const CACHE_ENTRIES: usize = 8;
const CACHE_BYTES: usize = 32 * 1024 * 1024;
const COMPUTATIONS: usize = 2;
const PROVEN_TRANSITIONS: usize = 65_536;

#[derive(Default)]
struct ProvenPositions {
    revision: i64,
    transitions: HashSet<(i64, i64)>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
struct Key {
    request: Request,
    revision: i64,
}
struct Entry {
    bytes: Arc<[u8]>,
    used: u64,
}
#[derive(Default)]
struct Cache {
    entries: HashMap<Key, Entry>,
    pending: HashSet<Key>,
    clock: u64,
    bytes: usize,
    stopped: bool,
}

/// Serialized repertoire responses belong to one application/database. Work runs
/// on the caller's blocking thread; concurrent requests share one computation.
/// There are no detached workers, and shutdown waits for every pending compute.
pub struct Repertoire {
    db: Arc<Database>,
    cache: Mutex<Cache>,
    changed: Condvar,
    positions: Mutex<ProvenPositions>,
    #[cfg(test)]
    before_compute: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    #[cfg(test)]
    waits: std::sync::atomic::AtomicUsize,
}
impl Repertoire {
    pub fn new(db: Arc<Database>) -> Self {
        Self {
            db,
            cache: Mutex::new(Cache::default()),
            changed: Condvar::new(),
            positions: Mutex::new(ProvenPositions::default()),
            #[cfg(test)]
            before_compute: Mutex::new(None),
            #[cfg(test)]
            waits: std::sync::atomic::AtomicUsize::new(0),
        }
    }
    pub fn request_stop(&self) {
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).stopped = true;
        self.changed.notify_all();
    }
    pub fn stop(&self) {
        self.request_stop();
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        while !cache.pending.is_empty() {
            cache = self.changed.wait(cache).unwrap_or_else(|e| e.into_inner());
        }
    }
    pub fn handle(&self, method: &str, path: &str, q: &Query) -> Option<Result<Arc<[u8]>>> {
        request(&self.db, method, path, q).map(|r| r.and_then(|r| self.cached(r)))
    }
    fn revision(&self) -> Result<i64> {
        self.db.read(|c| {
            Ok(
                c.query_row("SELECT revision FROM data_revision WHERE id=1", [], |r| {
                    r.get(0)
                })?,
            )
        })
    }
    fn cached(&self, request: Request) -> Result<Arc<[u8]>> {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            let key = Key {
                request: request.clone(),
                revision: self.revision()?,
            };
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if cache.stopped {
                return Err(api_error(503, "Plyback is shutting down."));
            }
            cache.clock += 1;
            let used = cache.clock;
            if let Some(entry) = cache.entries.get_mut(&key) {
                entry.used = used;
                return Ok(entry.bytes.clone());
            }
            if cache.pending.contains(&key) || cache.pending.len() >= COMPUTATIONS {
                if Instant::now() >= deadline {
                    return Err(api_error(503, "Repertoire is busy. Try again shortly."));
                }
                #[cfg(test)]
                self.waits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                drop(
                    self.changed
                        .wait_timeout(cache, Duration::from_millis(250))
                        .unwrap_or_else(|e| e.into_inner()),
                );
                continue;
            }
            cache.pending.insert(key.clone());
            drop(cache);
            // Release pending state on errors and panics as well as success.
            let pending = Pending {
                owner: self,
                key: key.clone(),
            };
            #[cfg(test)]
            if let Some(hook) = self.before_compute.lock().unwrap().clone() {
                hook();
            }
            let mut proven = {
                let positions = self.positions.lock().unwrap_or_else(|e| e.into_inner());
                if positions.revision == key.revision {
                    positions.transitions.clone()
                } else {
                    HashSet::new()
                }
            };
            let result = compute_proven(&self.db, &request, Some((key.revision, &mut proven)))
                .and_then(|(data, _)| {
                    let _span = tracing::info_span!("repertoire.serialize").entered();
                    Ok(Arc::<[u8]>::from(serde_json::to_vec(&data)?))
                })?;
            let current = self.revision()?;
            if current == key.revision {
                let mut positions = self.positions.lock().unwrap_or_else(|e| e.into_inner());
                if positions.revision != current {
                    positions.revision = current;
                    positions.transitions.clear();
                }
                // The proof cache is an optimization only. Bound it independently
                // of the serialized-response LRU, across every account.
                for transition in proven {
                    if positions.transitions.len() >= PROVEN_TRANSITIONS {
                        break;
                    }
                    positions.transitions.insert(transition);
                }
                tracing::debug!(
                    proven_transitions = positions.transitions.len(),
                    proven_capacity = positions.transitions.capacity(),
                    "Repertoire validated EPD cache"
                );
            }
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            if current == key.revision && result.len() <= CACHE_BYTES {
                // Old revisions cannot be reused; remove them before LRU eviction.
                let old: Vec<_> = cache
                    .entries
                    .keys()
                    .filter(|k| k.revision != current)
                    .cloned()
                    .collect();
                for key in old {
                    if let Some(entry) = cache.entries.remove(&key) {
                        cache.bytes -= entry.bytes.len();
                    }
                }
                cache.clock += 1;
                let used = cache.clock;
                cache.bytes += result.len();
                cache.entries.insert(
                    key.clone(),
                    Entry {
                        bytes: result.clone(),
                        used,
                    },
                );
                while cache.entries.len() > CACHE_ENTRIES || cache.bytes > CACHE_BYTES {
                    let victim = cache
                        .entries
                        .iter()
                        .min_by_key(|(_, entry)| entry.used)
                        .map(|(key, _)| key.clone());
                    if let Some(victim) = victim {
                        if let Some(entry) = cache.entries.remove(&victim) {
                            cache.bytes -= entry.bytes.len();
                        }
                    } else {
                        break;
                    }
                }
            }
            tracing::debug!(
                response_bytes = cache.bytes,
                entries = cache.entries.len(),
                "Repertoire response cache"
            );
            drop(cache);
            drop(pending);
            return Ok(result);
        }
    }
}
struct Pending<'a> {
    owner: &'a Repertoire,
    key: Key,
}
impl Drop for Pending<'_> {
    fn drop(&mut self) {
        self.owner
            .cache
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pending
            .remove(&self.key);
        self.owner.changed.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Barrier,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    };

    fn fixture() -> Arc<Database> {
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.transaction(|tx| {
            tx.execute("INSERT INTO accounts(id,username,added_at) VALUES(1,'fixture',0)", [])?;
            let games: &[&[&str]] = &[
                &["e4", "e5", "Nf3", "Nc6", "Bb5", "a6", "Ba4", "Nf6", "O-O"],
                &["d4", "d5", "c4", "e6", "Nc3", "Nf6"],
                &["e4", "a6", "e5", "d5", "exd6"],
                &["Nf3", "Nf6", "Ng1", "Ng8", "Nf3"],
                &["e4", "e5", "Bh6", "Nc6"], // Illegal SAN stops the tree here.
                &["d4", "Nf6", "c4", "e6", "Nc3"],
                &["e4", "c5", "Nf3"], // A gap stops this game after ply one.
            ];
            for (index, moves) in games.iter().enumerate() {
                let game = index as i64 + 1;
                tx.execute("INSERT INTO games(id,uuid,pgn,rules,end_time,white_result,black_result,time_class,imported_at) VALUES(?1,?1,'','chess',?2,'win','resigned',?3,0)", params![game, game * 100, if game % 2 == 0 { "rapid" } else { "blitz" }])?;
                tx.execute("INSERT INTO account_games(account_id,game_id,color) VALUES(1,?,?)", params![game, if game == 6 { "black" } else { "white" }])?;
                let mut board = Chess::default();
                for (index, san) in moves.iter().enumerate() {
                    let ply = index as i64 + 1;
                    tx.execute("INSERT INTO moves(game_id,ply,color,san,uci,fen_before,epd_before,is_book,classification,ep_loss) VALUES(?,?,?,?,?,?,?,?,?,?)", params![game, ply, if ply % 2 == 0 { "b" } else { "w" }, san, "", crate::pgn::fen(&board), crate::pgn::epd(&board), (ply < 3) as i64, if ply == 3 { "blunder" } else { "best" }, 0.2])?;
                    if let Some(played) = san.parse::<San>().ok().and_then(|san| san.to_move(&board).ok()) { board.play_unchecked(played); }
                }
            }
            tx.execute("UPDATE moves SET epd_before='malformed' WHERE game_id=1 AND ply=3", [])?;
            tx.execute("UPDATE moves SET epd_before='' WHERE game_id=2 AND ply=3", [])?;
            tx.execute("DELETE FROM moves WHERE game_id=7 AND ply=2", [])?;
            Ok(())
        }).unwrap();
        db
    }
    fn params() -> Request {
        Request {
            account: 1,
            color: "white".to_owned(),
            plies: 30,
            time_class: None,
            since: None,
            merge: false,
        }
    }
    fn bytes(value: &Value) -> Vec<u8> {
        serde_json::to_vec(value).unwrap()
    }

    #[test]
    fn proven_stored_epd_and_serialized_cache_match_replay_for_every_variant() {
        let db = fixture();
        let service = Repertoire::new(db.clone());
        let mut proven = HashSet::new();
        let revision = service.revision().unwrap();
        let first = params();
        let (validated, first_replays) =
            compute_proven(&db, &first, Some((revision, &mut proven))).unwrap();
        assert_eq!(bytes(&validated), bytes(&first.compute(&db).unwrap()));
        assert!(!proven.is_empty());
        let mut merged = first.clone();
        merged.merge = true;
        let (fast, subsequent_replays) =
            compute_proven(&db, &merged, Some((revision, &mut proven))).unwrap();
        assert!(
            subsequent_replays < first_replays,
            "a distinct filter must reuse proved EPDs"
        );
        assert_eq!(bytes(&fast), bytes(&merged.compute(&db).unwrap()));
        for color in ["white", "black"] {
            for plies in [2, 5, 16, 30] {
                for merge in [false, true] {
                    for time_class in [None, Some("blitz"), Some("rapid")] {
                        for since in [None, Some(350)] {
                            let request = Request {
                                color: color.to_owned(),
                                plies,
                                merge,
                                time_class: time_class.map(str::to_owned),
                                since,
                                ..params()
                            };
                            let expected = bytes(&request.compute(&db).unwrap());
                            let actual = service.cached(request.clone()).unwrap();
                            assert_eq!(actual.as_ref(), expected);
                            let hit = service.cached(request).unwrap();
                            assert!(Arc::ptr_eq(&actual, &hit));
                            assert_eq!(hit.as_ref(), expected);
                        }
                    }
                }
            }
        }
        // A revision change cannot reuse proofs or response allocations.
        let before = service.cached(params()).unwrap();
        db.execute("UPDATE accounts SET display_name='Changed' WHERE id=1", &[])
            .unwrap();
        let after = service.cached(params()).unwrap();
        assert!(!Arc::ptr_eq(&before, &after));
        assert_eq!(before.as_ref(), after.as_ref());
        let cache = service.cache.lock().unwrap();
        assert!(cache.entries.len() <= CACHE_ENTRIES);
        assert!(cache.bytes <= CACHE_BYTES);
    }

    fn blocked(service: &Repertoire) -> (Arc<AtomicUsize>, mpsc::Receiver<()>, mpsc::Sender<()>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let (started_tx, started) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        *service.before_compute.lock().unwrap() = Some(Arc::new(move || {
            count.fetch_add(1, Ordering::SeqCst);
            started_tx.send(()).unwrap();
            release_rx.lock().unwrap().recv().unwrap();
        }));
        (calls, started, release)
    }

    #[test]
    fn concurrent_cold_requests_share_one_computation() {
        let service = Arc::new(Repertoire::new(fixture()));
        let (calls, started, release) = blocked(&service);
        let barrier = Arc::new(Barrier::new(9));
        let workers = (0..8)
            .map(|_| {
                let service = service.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    service.cached(params()).unwrap()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while service.waits.load(Ordering::SeqCst) < 7 {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        release.send(()).unwrap();
        let results = workers
            .into_iter()
            .map(|worker| worker.join().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(results.iter().all(|data| Arc::ptr_eq(data, &results[0])));
        service.stop();
    }

    #[test]
    fn shutdown_waits_for_in_flight_computation_and_rejects_new_work() {
        let service = Arc::new(Repertoire::new(fixture()));
        let (_, started, release) = blocked(&service);
        let worker_service = service.clone();
        let compute = std::thread::spawn(move || worker_service.cached(params()).unwrap());
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        let stopping = service.clone();
        let (closed_tx, closed) = mpsc::channel();
        let stop = std::thread::spawn(move || {
            stopping.stop();
            closed_tx.send(()).unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !service.cache.lock().unwrap().stopped {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(matches!(
            closed.recv_timeout(Duration::from_millis(50)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ));
        assert_eq!(
            service
                .cached(params())
                .unwrap_err()
                .downcast_ref::<crate::ApiError>()
                .unwrap()
                .0,
            503
        );
        release.send(()).unwrap();
        compute.join().unwrap();
        closed.recv_timeout(Duration::from_secs(5)).unwrap();
        stop.join().unwrap();
        assert!(service.cache.lock().unwrap().pending.is_empty());
    }
}
