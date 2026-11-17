//! Practice: puzzles from the account's own reviewed mistakes.
//!
//! A position qualifies when the account's move there was reviewed as a
//! blunder, mistake or miss, and the engine's best move was clearly better than
//! its second choice (by `MIN_GAP` expected points, from the mover's side).
//! That makes the stored best move the single fair answer, so answers are
//! checked against the stored review without any new engine work.
//!
//! Scheduling is a small Leitner system: a correct answer moves the position up
//! one box and schedules it for the start of the day that box's interval of days later, in the
//! answering client's time zone (so it is due at any time that day, and never again on the day
//! it was answered); a wrong answer sends it back to the first box and brings it back after
//! `RETRY_SECONDS`.
use crate::{
    ApiResult, Query,
    analysis::expected_points,
    api_error,
    db::Database,
    now,
    params::{filter, named_id as id, object},
    pgn, tactics,
};
use anyhow::Result;
use chrono::{DateTime, Days, NaiveTime, TimeZone};
use chrono_tz::{GapInfo, Tz, UTC};
use rand::seq::SliceRandom;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use shakmaty::uci::UciMove;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

/// Smallest expected-points margin between the best and second-best engine
/// line for a position to have one fair answer.
pub const MIN_GAP: f64 = 0.05;
/// Days until the next showing after a correct answer from box `i`.
pub const INTERVAL_DAYS: [i64; 6] = [1, 3, 7, 16, 35, 80];
/// A missed position comes back within the same sitting.
pub const RETRY_SECONDS: i64 = 600;
/// Positions at or above this box count as mastered.
pub const MASTERED_BOX: i64 = 4;
const KINDS: [&str; 3] = ["blunder", "mistake", "miss"];
const PHASES: [&str; 3] = ["opening", "middlegame", "endgame"];

#[derive(Debug, Clone)]
struct Candidate {
    game_id: i64,
    ply: i64,
    color: String,
    fen: String,
    san: String,
    best_uci: String,
    best_san: String,
    classification: String,
    phase: Option<String>,
    ep_loss: f64,
    gap: f64,
    explanation: Option<String>,
    line_san: Vec<String>,
    themes: Vec<&'static str>,
    last_uci: Option<String>,
    last_san: Option<String>,
    end_time: Option<i64>,
    time_class: Option<String>,
    opponent: Option<String>,
    opponent_rating: Option<i64>,
    rules: String,
    state: Option<State>,
}

#[derive(Debug, Clone, Copy)]
struct State {
    box_: i64,
    due_at: i64,
    attempts: i64,
    correct: i64,
}

/// The mover-perspective margin between the first two stored engine lines, and
/// the best line's SAN. `None` when the lines cannot establish a single answer.
fn answer_line(lines_json: &str, white: bool, best_uci: &str) -> Option<(f64, Vec<String>)> {
    let lines: Vec<Value> = serde_json::from_str(lines_json).ok()?;
    let score = |line: &Value| -> Option<f64> {
        let cp = line["cp"].as_i64()?;
        Some(expected_points(cp.clamp(-100_000, 100_000) as i32, white))
    };
    let first = lines.first()?;
    if first["uci"].as_str()? != best_uci {
        return None;
    }
    let gap = score(first)? - score(lines.get(1)?)?;
    let san = first["san"]
        .as_array()
        .map(|moves| {
            moves
                .iter()
                .filter_map(|m| m.as_str().map(str::to_owned))
                .collect()
        })
        .unwrap_or_default();
    Some((gap, san))
}

/// Static candidates belong to the database, not a process-global path cache.
/// Keep at most eight accounts and 16 MiB of candidate allocations.
#[derive(Default)]
pub(crate) struct CandidateCache(Mutex<Cache>);
#[derive(Default)]
struct Cache {
    entries: HashMap<i64, CandidateEntry>,
    clock: u64,
}
struct CandidateEntry {
    revision: i64,
    candidates: Arc<Vec<Candidate>>,
    bytes: usize,
    used: u64,
}

const CANDIDATE_SELECT: &str = "SELECT m.game_id, m.ply, m.color, m.fen_before, m.san, m.best_uci,
                m.best_san, m.classification, m.phase, m.ep_loss, m.explanation, m.lines_json,
                prev.uci, prev.san, g.end_time, g.time_class, g.rules,
                CASE WHEN m.color='w' THEN g.black_username ELSE g.white_username END,
                CASE WHEN m.color='w' THEN g.black_rating ELSE g.white_rating END
         FROM account_games ag
         JOIN games g ON g.id = ag.game_id
         JOIN moves m ON m.game_id = ag.game_id AND m.color = substr(ag.color, 1, 1)
         LEFT JOIN moves prev ON prev.game_id = m.game_id AND prev.ply = m.ply - 1
         WHERE ag.account_id = ?1
           AND m.classification IN ('blunder', 'mistake', 'miss')
           AND m.best_uci IS NOT NULL AND m.best_uci <> m.uci
           AND m.lines_json IS NOT NULL AND g.rules = 'chess'";

/// The single-position statement has direct PK predicates rather than optional ORs.
fn candidates(
    connection: &Connection,
    account: i64,
    only: Option<(i64, i64)>,
) -> Result<Vec<Candidate>> {
    let sql = if only.is_some() {
        format!("{CANDIDATE_SELECT} AND m.game_id = ?2 AND m.ply = ?3")
    } else {
        format!("{CANDIDATE_SELECT} ORDER BY g.end_time DESC, m.game_id DESC, m.ply")
    };
    let mut statement = connection.prepare_cached(&sql)?;
    let mut rows = match only {
        Some((game, ply)) => statement.query(params![account, game, ply])?,
        None => statement.query([account])?,
    };
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let color: String = row.get(2)?;
        let best_uci: String = row.get(5)?;
        let lines: String = row.get(11)?;
        let Some((gap, line_san)) = answer_line(&lines, color == "w", &best_uci) else {
            continue;
        };
        if gap < MIN_GAP {
            continue;
        }
        out.push(Candidate {
            game_id: row.get(0)?,
            ply: row.get(1)?,
            color,
            fen: row.get(3)?,
            san: row.get(4)?,
            best_uci,
            best_san: row.get::<_, Option<String>>(6)?.unwrap_or_default(),
            classification: row.get(7)?,
            phase: row.get(8)?,
            ep_loss: row.get::<_, Option<f64>>(9)?.unwrap_or(0.0),
            gap,
            explanation: row.get(10)?,
            line_san,
            themes: Vec::new(),
            last_uci: row.get(12)?,
            last_san: row.get(13)?,
            end_time: row.get(14)?,
            time_class: row.get(15)?,
            rules: row.get(16)?,
            opponent: row.get(17)?,
            opponent_rating: row.get(18)?,
            state: None,
        });
    }
    Ok(out)
}

type States = HashMap<(i64, i64), State>;
fn states(connection: &Connection, account: i64) -> Result<States> {
    let mut statement = connection.prepare_cached(
        "SELECT game_id, ply, box, due_at, attempts, correct FROM practice_items WHERE account_id=?1"
    )?;
    Ok(statement
        .query_map([account], |row| {
            Ok((
                (row.get(0)?, row.get(1)?),
                State {
                    box_: row.get(2)?,
                    due_at: row.get(3)?,
                    attempts: row.get(4)?,
                    correct: row.get(5)?,
                },
            ))
        })?
        .collect::<rusqlite::Result<States>>()?)
}

fn candidate_bytes(all: &[Candidate]) -> usize {
    all.iter()
        .map(|c| {
            std::mem::size_of::<Candidate>()
                + [
                    &c.color,
                    &c.fen,
                    &c.san,
                    &c.best_uci,
                    &c.best_san,
                    &c.classification,
                    &c.rules,
                ]
                .iter()
                .map(|s| s.capacity())
                .sum::<usize>()
                + [
                    &c.phase,
                    &c.explanation,
                    &c.last_uci,
                    &c.last_san,
                    &c.time_class,
                    &c.opponent,
                ]
                .iter()
                .filter_map(|s| s.as_ref())
                .map(|s| s.capacity())
                .sum::<usize>()
                + c.line_san.capacity() * std::mem::size_of::<String>()
                + c.line_san.iter().map(String::capacity).sum::<usize>()
                + c.themes.capacity() * std::mem::size_of::<&str>()
        })
        .sum::<usize>()
}

fn detect_themes(all: &mut [Candidate]) {
    for c in all {
        debug_assert!(c.gap >= MIN_GAP);
        c.themes = tactics::detect(&c.fen, &c.best_uci, &c.line_san);
    }
}

fn with_states(all: &[Candidate], states: &States) -> Vec<Candidate> {
    all.iter()
        .map(|c| {
            let mut c = c.clone();
            c.state = states.get(&(c.game_id, c.ply)).copied();
            c
        })
        .collect()
}

fn cached_candidates(db: &Database, account: i64) -> Result<Vec<Candidate>> {
    // Serialize fills so concurrent queue requests only parse/detect once. The
    // cache lock is never acquired from inside a database read or transaction.
    let mut cache = db
        .practice_candidates
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let (revision, state) = db.read(|connection| {
        Ok((
            connection.query_row("SELECT revision FROM data_revision WHERE id=1", [], |r| {
                r.get::<_, i64>(0)
            })?,
            states(connection, account)?,
        ))
    })?;
    cache.clock += 1;
    let used = cache.clock;
    if let Some(entry) = cache
        .entries
        .get_mut(&account)
        .filter(|e| e.revision == revision)
    {
        entry.used = used;
        return Ok(with_states(&entry.candidates, &state));
    }
    let (revision, mut all, state) = db.read(|connection| {
        Ok((
            connection.query_row("SELECT revision FROM data_revision WHERE id=1", [], |r| {
                r.get::<_, i64>(0)
            })?,
            candidates(connection, account, None)?,
            states(connection, account)?,
        ))
    })?;
    detect_themes(&mut all);
    let bytes =
        candidate_bytes(&all) + (all.capacity() - all.len()) * std::mem::size_of::<Candidate>();
    tracing::debug!(
        account,
        revision,
        candidates = all.len(),
        bytes,
        "Practice candidates loaded"
    );
    let all = Arc::new(all);
    cache.entries.remove(&account);
    if bytes <= 16 * 1024 * 1024 {
        cache.entries.insert(
            account,
            CandidateEntry {
                revision,
                candidates: all.clone(),
                bytes,
                used,
            },
        );
        while cache.entries.len() > 8
            || cache.entries.values().map(|e| e.bytes).sum::<usize>() > 16 * 1024 * 1024
        {
            let victim = *cache.entries.iter().min_by_key(|(_, e)| e.used).unwrap().0;
            cache.entries.remove(&victim);
        }
    }
    Ok(with_states(&all, &state))
}

fn stats(all: &[Candidate], at: i64) -> Value {
    let (mut new, mut due, mut learning, mut mastered) = (0, 0, 0, 0);
    let mut next_due: Option<i64> = None;
    for c in all {
        match c.state {
            None => new += 1,
            Some(s) => {
                if s.due_at <= at {
                    due += 1;
                } else {
                    next_due = Some(next_due.map_or(s.due_at, |n| n.min(s.due_at)));
                }
                if s.box_ >= MASTERED_BOX {
                    mastered += 1;
                } else {
                    learning += 1;
                }
            }
        }
    }
    json!({"total": all.len(), "new": new, "due": due, "learning": learning,
           "mastered": mastered, "next_due_at": next_due})
}

fn public(c: &Candidate, at: i64) -> Value {
    let status = match c.state {
        None => "new",
        Some(s) if s.due_at <= at => "due",
        Some(_) => "scheduled",
    };
    json!({
        "game_id": c.game_id,
        "ply": c.ply,
        "fen": c.fen,
        "color": if c.color == "w" { "white" } else { "black" },
        // From the position's own FEN, so games imported from a set-up position keep their numbers.
        "move_number": c
            .fen
            .split_whitespace()
            .nth(5)
            .and_then(|n| n.parse::<i64>().ok())
            .filter(|n| *n > 0)
            .unwrap_or((c.ply + 1) / 2),
        "classification": c.classification,
        "phase": c.phase,
        "themes": c.themes,
        "ep_loss": c.ep_loss,
        "last_uci": c.last_uci,
        "last_san": c.last_san,
        "end_time": c.end_time,
        "time_class": c.time_class,
        "opponent": c.opponent,
        "opponent_rating": c.opponent_rating,
        "status": status,
        "box": c.state.map_or(0, |s| s.box_),
        "attempts": c.state.map_or(0, |s| s.attempts),
        "correct": c.state.map_or(0, |s| s.correct),
    })
}

/// Without `since`: due positions first (oldest due first), then new ones from
/// the most recent games. With `since` (a Unix time): every position from games
/// that ended since then, answered or not, in random order, so a day or week of
/// games can be gone through. The answer is not included; it is returned after
/// an attempt.
fn queue(db: &Database, account: i64, q: &Query) -> ApiResult {
    queue_with(db, account, q, cached_candidates, &mut rand::rng(), now)
}

fn queue_with(
    db: &Database,
    account: i64,
    q: &Query,
    load: fn(&Database, i64) -> Result<Vec<Candidate>>,
    rng: &mut impl rand::Rng,
    clock: fn() -> i64,
) -> ApiResult {
    let kind = filter(q, "kind", &KINDS)?;
    let phase = filter(q, "phase", &PHASES)?;
    let theme = filter(q, "theme", tactics::THEMES)?;
    let limit = match q.get("limit") {
        None => 10,
        Some(raw) => raw
            .parse::<usize>()
            .ok()
            .filter(|n| (1..=50).contains(n))
            .ok_or_else(|| api_error(422, "limit must be between 1 and 50"))?,
    };
    let since = match q.get("since").map(String::as_str) {
        None | Some("") => None,
        Some(raw) => Some(
            raw.parse::<i64>()
                .map_err(|_| api_error(422, "since must be a Unix timestamp"))?,
        ),
    };
    let mut all = load(db, account)?;
    all.retain(|c| {
        kind.is_none_or(|k| c.classification == k)
            && phase.is_none_or(|p| c.phase.as_deref() == Some(p))
    });
    if let Some(since) = since {
        all.retain(|c| c.end_time.is_some_and(|t| t >= since));
    }
    // Cached themes include scheduled positions. Counts remain scoped to every
    // filter except the selected theme.
    let mut theme_counts = serde_json::Map::new();
    for &name in tactics::THEMES {
        theme_counts.insert(name.to_owned(), json!(0));
    }
    for candidate in &all {
        for &name in &candidate.themes {
            if let Some(count) = theme_counts.get_mut(name) {
                *count = json!(count.as_u64().unwrap_or(0) + 1);
            }
        }
    }
    if let Some(theme) = theme {
        all.retain(|c| c.themes.contains(&theme));
    }
    let at = clock();
    let items = if since.is_some() {
        let mut picked: Vec<&Candidate> = all.iter().collect();
        picked.shuffle(rng);
        picked
            .into_iter()
            .take(limit)
            .map(|c| public(c, at))
            .collect::<Vec<_>>()
    } else {
        let mut due: Vec<&Candidate> = all
            .iter()
            .filter(|c| c.state.is_some_and(|s| s.due_at <= at))
            .collect();
        due.sort_by_key(|c| (c.state.map_or(0, |s| s.due_at), c.game_id, c.ply));
        due.into_iter()
            .chain(all.iter().filter(|c| c.state.is_none()))
            .take(limit)
            .map(|c| public(c, at))
            .collect::<Vec<_>>()
    };
    Ok((
        200,
        json!({"stats": stats(&all, at), "items": items, "theme_counts": theme_counts}),
    ))
}

/// The submitted move in standard UCI, or an error if it is not legal here.
fn normalize(fen: &str, raw: &str) -> Result<String> {
    let position = pgn::board(fen, false)?;
    let parsed: UciMove = raw
        .parse()
        .map_err(|_| api_error(422, "move must be a UCI move such as e2e4"))?;
    let legal = parsed
        .to_move(&position)
        .map_err(|_| api_error(422, "That move is not legal in this position"))?;
    Ok(pgn::uci(legal, false))
}

/// A correct answer is due from the start of the target calendar day in the client's zone.
pub fn schedule(box_: i64, correct: bool, at: i64, time_zone: Tz) -> (i64, i64) {
    if !correct {
        return (0, at + RETRY_SECONDS);
    }
    let index = box_.clamp(0, INTERVAL_DAYS.len() as i64 - 1) as usize;
    let days = INTERVAL_DAYS[index];
    let due = DateTime::from_timestamp(at, 0)
        .and_then(|time| {
            time.with_timezone(&time_zone)
                .date_naive()
                .checked_add_days(Days::new(days as u64))
        })
        .and_then(|date| {
            let midnight = date.and_time(NaiveTime::MIN);
            // Some zones move their clocks at midnight. Use the first valid instant.
            time_zone
                .from_local_datetime(&midnight)
                .earliest()
                .or_else(|| GapInfo::new(&midnight, &time_zone)?.end)
        })
        .map_or(at + days * 86_400, |time| time.timestamp());
    (box_ + 1, due)
}

fn answer(db: &Database, account: i64, game: i64, ply: i64, body: &Value) -> ApiResult {
    answer_with_clock(db, account, game, ply, body, now)
}

fn answer_with_clock(
    db: &Database,
    account: i64,
    game: i64,
    ply: i64,
    body: &Value,
    clock: fn() -> i64,
) -> ApiResult {
    // `{"reveal": true}` shows the answer without a move and counts as a miss.
    let reveal = match body.get("reveal") {
        None | Some(Value::Bool(false)) => false,
        Some(Value::Bool(true)) => true,
        Some(_) => return Err(api_error(422, "reveal must be true or false")),
    };
    let time_zone = match body.get("time_zone") {
        None | Some(Value::Null) => UTC,
        Some(value) => value
            .as_str()
            .filter(|name| name.len() <= 64)
            .and_then(|name| name.parse::<Tz>().ok())
            .ok_or_else(|| api_error(422, "time_zone must be an IANA time zone"))?,
    };
    let raw = if reveal {
        None
    } else {
        Some(
            body.get("uci")
                .and_then(Value::as_str)
                .filter(|s| (4..=5).contains(&s.len()))
                .ok_or_else(|| api_error(422, "uci must be a move such as e2e4"))?,
        )
    };
    let mut candidate = db
        .read(|c| {
            let mut all = candidates(c, account, Some((game, ply)))?;
            if let Some(candidate) = all.first_mut() {
                candidate.state = c.query_row(
                    "SELECT box, due_at, attempts, correct FROM practice_items WHERE account_id=?1 AND game_id=?2 AND ply=?3",
                    params![account, game, ply], |r| Ok(State {
                        box_: r.get(0)?, due_at: r.get(1)?, attempts: r.get(2)?, correct: r.get(3)?,
                    }),
                ).optional()?;
            }
            Ok(all)
        })?
        .into_iter()
        .next()
        .ok_or_else(|| api_error(404, "Practice position not found"))?;
    candidate.themes = tactics::detect(&candidate.fen, &candidate.best_uci, &candidate.line_san);
    debug_assert_eq!(candidate.rules, "chess");
    let played = raw.map(|raw| normalize(&candidate.fen, raw)).transpose()?;
    let correct = played.as_deref() == Some(candidate.best_uci.as_str());
    let at = clock();
    let before = candidate.state;
    let (box_, due_at) = match before {
        // A correct answer before the position is due (the Today / recent ranges list positions
        // whether due or not) is extra practice: it keeps the schedule instead of moving the
        // position up a box, so repeating a day's games cannot mark it mastered within an hour.
        Some(s) if correct && s.due_at > at => (s.box_, s.due_at),
        _ => schedule(before.map_or(0, |s| s.box_), correct, at, time_zone),
    };
    db.transaction(|tx| {
        tx.execute(
            "INSERT INTO practice_items
                 (account_id, game_id, ply, box, due_at, attempts, correct, lapses,
                  first_seen_at, last_seen_at, last_correct)
             VALUES (?1, ?2, ?3, ?4, ?5, 1, ?6, ?7, ?8, ?8, ?6)
             ON CONFLICT(account_id, game_id, ply) DO UPDATE SET
                 box = excluded.box, due_at = excluded.due_at,
                 attempts = attempts + 1, correct = correct + excluded.correct,
                 lapses = lapses + excluded.lapses,
                 last_seen_at = excluded.last_seen_at, last_correct = excluded.last_correct
             -- Answering again before the position is due, within 10 minutes of the last
             -- answer (a second tab, a repeated request), only shows the solution: the first
             -- answer already set the schedule and must not gain a second lapse.
             WHERE NOT (practice_items.due_at > excluded.last_seen_at
                        AND practice_items.last_seen_at > excluded.last_seen_at - 600)",
            params![
                account,
                game,
                ply,
                box_,
                due_at,
                correct as i64,
                (!correct && before.is_some()) as i64,
                at
            ],
        )?;
        Ok(())
    })?;
    let (box_, due_at): (i64, i64) = db.read(|c| {
        Ok(c.query_row(
            "SELECT box, due_at FROM practice_items WHERE account_id=?1 AND game_id=?2 AND ply=?3",
            params![account, game, ply],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?)
    })?;
    let all = cached_candidates(db, account)?;
    let san = played.as_ref().map(|played| {
        pgn::board(&candidate.fen, false)
            .ok()
            .and_then(|position| {
                let m = played.parse::<UciMove>().ok()?.to_move(&position).ok()?;
                Some(shakmaty::san::San::from_move(&position, m).to_string())
            })
            .unwrap_or_else(|| played.clone())
    });
    Ok((
        200,
        json!({
            "correct": correct,
            "uci": played,
            "san": san,
            "best_uci": candidate.best_uci,
            "best_san": candidate.best_san,
            "line_san": candidate.line_san,
            "themes": candidate.themes,
            "game_san": candidate.san,
            "explanation": candidate.explanation,
            "box": box_,
            "due_at": due_at,
            "stats": stats(&all, at),
        }),
    ))
}

fn account_exists(db: &Database, account: i64) -> Result<()> {
    let found = db.read(|c| {
        Ok(
            c.query_row("SELECT 1 FROM accounts WHERE id=?", [account], |_| Ok(()))
                .optional()?,
        )
    })?;
    found.ok_or_else(|| api_error(404, "Account not found"))
}

pub fn handle(
    db: &Database,
    method: &str,
    path: &str,
    q: &Query,
    body: &Value,
) -> Option<ApiResult> {
    let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
    match (method, parts.as_slice()) {
        ("GET", ["api", "accounts", account, "practice"]) => Some((|| {
            let account = id(account, "account_id")?;
            account_exists(db, account)?;
            queue(db, account, q)
        })()),
        ("POST", ["api", "accounts", account, "practice", game, ply]) => Some((|| {
            let account = id(account, "account_id")?;
            account_exists(db, account)?;
            object(body)?;
            answer(db, account, id(game, "game_id")?, id(ply, "ply")?, body)
        })()),
        (_, ["api", "accounts", _, "practice", ..]) => {
            Some(Err(api_error(405, "Method not allowed")))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    include!("../tests/fixtures/practice_legacy.rs");

    fn fixed_time() -> i64 {
        2_000_000_000
    }

    fn cache_fixture() -> Database {
        let db = Database::open(":memory:").unwrap();
        db.transaction(|tx| {
            tx.execute_batch("INSERT INTO accounts(id, username, added_at) VALUES(1,'me',0),(2,'other',0)")?;
            let fen = "r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 4 4";
            let lines = json!([{"cp":99999,"uci":"h5f7","san":["Qxf7#"]},{"cp":0,"uci":"d2d3","san":["d3"]}]).to_string();
            for game in [8, 1, 6, 3, 9, 2, 5, 4, 7] {
                tx.execute("INSERT INTO games(id,uuid,pgn,rules,end_time,time_class,white_username,black_username,imported_at)
                    VALUES(?1,?2,'','chess',CASE WHEN ?1=5 THEN 11 ELSE 10 END,'blitz','me','rival',0)", params![game, format!("g{game}")])?;
                tx.execute("INSERT INTO account_games(account_id,game_id,color) VALUES(1,?1,'white')", [game])?;
                tx.execute("INSERT INTO moves(game_id,ply,color,san,uci,fen_before,epd_before,classification,phase,best_uci,best_san,lines_json,ep_loss)
                    VALUES(?1,7,'w','Nc3','b1c3',?2,'',?3,?4,'h5f7','Qxf7#',?5,0.4)",
                    params![game, fen, match game % 3 { 0 => "mistake", 1 => "blunder", _ => "miss" }, match game % 3 { 0 => "endgame", 1 => "opening", _ => "middlegame" }, lines])?;
            }
            // New, overdue learning, future learning, overdue mastered and future mastered.
            for (game, box_, due_at) in [(2,0,0),(3,1,i64::MAX-1),(4,4,0),(5,5,i64::MAX-2)] {
                tx.execute("INSERT INTO practice_items(account_id,game_id,ply,box,due_at,attempts,correct,lapses,first_seen_at,last_seen_at,last_correct)
                    VALUES(1,?1,7,?2,?3,7,4,1,0,0,1)", params![game, box_, due_at])?;
            }
            Ok(())
        }).unwrap();
        db
    }

    fn assert_queue_bytes(db: &Database, filters: &[(&str, &str)]) {
        use rand::SeedableRng;
        let q = filters
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let cached = queue_with(
            db,
            1,
            &q,
            cached_candidates,
            &mut rand::rngs::StdRng::seed_from_u64(42),
            fixed_time,
        )
        .unwrap();
        let uncached = legacy_queue(
            db,
            1,
            &q,
            &mut rand::rngs::StdRng::seed_from_u64(42),
            fixed_time(),
        )
        .unwrap();
        assert_eq!(cached.0, uncached.0);
        assert_eq!(
            serde_json::to_vec(&cached.1).unwrap(),
            serde_json::to_vec(&uncached.1).unwrap(),
            "{filters:?}"
        );
    }

    #[test]
    fn cached_and_uncached_queue_bytes_match_across_states_filters_and_revisions() {
        let db = cache_fixture();
        for filters in [
            vec![],
            vec![("kind", "miss")],
            vec![("phase", "opening")],
            vec![("theme", "mate-in-1")],
            vec![("theme", "fork")],
            vec![("limit", "1")],
            // Seed the same random shuffle to compare recent ranges including scheduled items.
            vec![("since", "0")],
            vec![("since", "5")],
        ] {
            assert_queue_bytes(&db, &filters);
            assert_queue_bytes(&db, &filters);
        }
        let all = cached_candidates(&db, 1).unwrap();
        let counts = stats(&all, now());
        assert_eq!(counts["new"], 5);
        assert_eq!(counts["due"], 2);
        assert_eq!(counts["mastered"], 2);
        let shared = db.practice_candidates.0.lock().unwrap().entries[&1]
            .candidates
            .clone();
        // State updates do not bump data_revision or rebuild static candidates.
        db.execute(
            "UPDATE practice_items SET due_at=0,box=2 WHERE game_id=5",
            &[],
        )
        .unwrap();
        assert_queue_bytes(&db, &[]);
        assert!(Arc::ptr_eq(
            &shared,
            &db.practice_candidates.0.lock().unwrap().entries[&1].candidates
        ));
        db.transaction(|tx| {
            tx.execute("UPDATE moves SET classification='good' WHERE game_id=1", [])?;
            tx.execute(
                "UPDATE data_revision SET revision=revision+1 WHERE id=1",
                [],
            )?;
            Ok(())
        })
        .unwrap();
        assert_queue_bytes(&db, &[]);
        assert_eq!(cached_candidates(&db, 1).unwrap().len(), 8);
        assert!(!Arc::ptr_eq(
            &shared,
            &db.practice_candidates.0.lock().unwrap().entries[&1].candidates
        ));
        assert!(cached_candidates(&db, 2).unwrap().is_empty());
        assert_eq!(cached_candidates(&cache_fixture(), 1).unwrap().len(), 9);
    }

    #[test]
    fn original_queue_sql_matches_cached_bytes_with_ties_and_combined_filters() {
        let db = cache_fixture();
        for kind in ["all", "blunder", "mistake", "miss"] {
            for phase in ["all", "opening", "middlegame", "endgame"] {
                for theme in ["all", "mate-in-1", "fork"] {
                    for since in ["", "0", "11"] {
                        assert_queue_bytes(
                            &db,
                            &[
                                ("kind", kind),
                                ("phase", phase),
                                ("theme", theme),
                                ("since", since),
                                ("limit", "3"),
                            ],
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn original_and_cached_answer_bytes_match_for_every_scheduling_state() {
        for warm in [false, true] {
            for game in 1..=5 {
                for body in [
                    json!({"reveal":true}),
                    json!({"uci":"h5f7","time_zone":"Europe/Ljubljana"}),
                    json!({"uci":"d2d3"}),
                ] {
                    let db = cache_fixture();
                    let baseline = cache_fixture();
                    if warm {
                        cached_candidates(&db, 1).unwrap();
                    }
                    let actual = answer_with_clock(&db, 1, game, 7, &body, fixed_time).unwrap();
                    let expected =
                        legacy_answer(&baseline, 1, game, 7, &body, fixed_time()).unwrap();
                    assert_eq!(actual.0, expected.0);
                    assert_eq!(
                        serde_json::to_vec(&actual.1).unwrap(),
                        serde_json::to_vec(&expected.1).unwrap(),
                        "warm={warm} game={game} body={body}"
                    );
                    // Compare the second answer too: it exercises the duplicate-answer guard.
                    let actual = answer_with_clock(&db, 1, game, 7, &body, fixed_time).unwrap();
                    let expected =
                        legacy_answer(&baseline, 1, game, 7, &body, fixed_time()).unwrap();
                    assert_eq!(
                        serde_json::to_vec(&actual.1).unwrap(),
                        serde_json::to_vec(&expected.1).unwrap()
                    );
                }
            }
        }
    }

    #[test]
    fn single_candidate_uses_the_move_primary_key_and_matches_bulk() {
        let db = cache_fixture();
        db.read(|c| {
            let sql =
                format!("EXPLAIN QUERY PLAN {CANDIDATE_SELECT} AND m.game_id = ?2 AND m.ply = ?3");
            let mut statement = c.prepare(&sql)?;
            let plan = statement
                .query_map(params![1, 3, 7], |r| r.get::<_, String>(3))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            assert!(
                plan.iter().any(
                    |s| s.contains("SEARCH m USING INDEX") && s.contains("game_id=? AND ply=?")
                ),
                "{plan:?}"
            );
            let bulk = candidates(c, 1, None)?;
            let one = candidates(c, 1, Some((3, 7)))?;
            assert_eq!(one.len(), 1);
            assert_eq!(
                format!("{:?}", bulk.iter().find(|c| c.game_id == 3).unwrap()),
                format!("{:?}", &one[0])
            );
            assert!(candidates(c, 2, Some((3, 7)))?.is_empty());
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn schedule_climbs_boxes_and_resets_on_a_miss() {
        assert_eq!(schedule(0, true, 0, UTC), (1, 86_400));
        assert_eq!(schedule(1, true, 100, UTC), (2, 3 * 86_400));
        assert_eq!(schedule(9, true, 86_399, UTC), (10, 80 * 86_400));
        assert_eq!(schedule(5, false, 100, UTC), (0, 100 + RETRY_SECONDS));
    }

    #[test]
    fn a_position_due_tomorrow_is_due_all_of_tomorrow_and_not_today() {
        // Monday 2026-11-09 in UTC+1 runs from Sunday 23:00 UTC.
        let zone = chrono_tz::Europe::Ljubljana;
        let monday = 1_794_178_800;
        let tuesday = monday + 86_400;
        // Answered at 00:00, 09:00 or 23:59 on Monday: due from Tuesday 00:00 local time, so
        // neither an evening sitting on Monday nor one 12 hours later can promote it again,
        // and any sitting on Tuesday includes it.
        for at in [monday, monday + 9 * 3600, tuesday - 1] {
            assert_eq!(schedule(0, true, at, zone), (1, tuesday), "at {at}");
            assert_eq!(schedule(1, true, at, zone).1, monday + 3 * 86_400);
        }
        // Monday at midnight in Ljubljana is still Sunday in New York.
        let new_york = chrono_tz::America::New_York;
        let monday_in_new_york = new_york.with_ymd_and_hms(2026, 11, 9, 0, 0, 0).unwrap();
        assert_eq!(
            schedule(0, true, monday, new_york).1,
            monday_in_new_york.timestamp()
        );
    }

    #[test]
    fn the_due_day_uses_the_offset_in_effect_on_that_day() {
        let zone = chrono_tz::Europe::Ljubljana;
        let spring = UTC.with_ymd_and_hms(2026, 3, 28, 12, 0, 0).unwrap();
        let spring_due = UTC.with_ymd_and_hms(2026, 3, 30, 22, 0, 0).unwrap();
        assert_eq!(
            schedule(1, true, spring.timestamp(), zone).1,
            spring_due.timestamp()
        );
        let autumn = UTC.with_ymd_and_hms(2026, 10, 25, 0, 0, 0).unwrap();
        let autumn_due = UTC.with_ymd_and_hms(2026, 10, 25, 23, 0, 0).unwrap();
        assert_eq!(
            schedule(0, true, autumn.timestamp(), zone).1,
            autumn_due.timestamp()
        );
    }

    #[test]
    fn answer_line_needs_a_clear_best_move_from_the_movers_side() {
        let lines = r#"[{"cp":-152,"uci":"d8d4","san":["Qxd4+","Qxd4"]},{"cp":-24,"uci":"e7e6","san":["e6"]}]"#;
        let (gap, san) = answer_line(lines, false, "d8d4").unwrap();
        assert!((0.10..0.12).contains(&gap), "{gap}");
        assert_eq!(san, ["Qxd4+", "Qxd4"]);
        // Same lines from White's side would make the "best" line the worse one.
        assert!(answer_line(lines, true, "d8d4").unwrap().0 < 0.0);
        // The stored best move must be the first line, and two lines are needed.
        assert!(answer_line(lines, false, "e7e6").is_none());
        assert!(answer_line(r#"[{"cp":10,"uci":"e2e4","san":[]}]"#, true, "e2e4").is_none());
        // Two forced mates compare as equal, so there is no single answer.
        let mates = r#"[{"cp":99990,"uci":"d1h5","san":[]},{"cp":99980,"uci":"d1f3","san":[]}]"#;
        assert!(answer_line(mates, true, "d1h5").unwrap().0 < MIN_GAP);
    }

    #[test]
    fn normalize_accepts_legal_moves_only() {
        let fen = "r3k2r/8/8/8/8/8/8/R3K2R w KQkq - 0 1";
        assert_eq!(normalize(fen, "e1g1").unwrap(), "e1g1");
        assert!(normalize(fen, "e1e3").is_err());
        assert!(normalize(fen, "zz").is_err());
        let promo = "8/P7/8/8/8/8/8/k6K w - - 0 1";
        assert_eq!(normalize(promo, "a7a8n").unwrap(), "a7a8n");
    }
}
