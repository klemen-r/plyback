//! Practice: puzzles from the account's own reviewed mistakes.
//!
//! A position qualifies when the account's move there was reviewed as a
//! blunder, mistake or miss, and the engine's best move was clearly better than
//! its second choice (by `MIN_GAP` expected points, from the mover's side).
//! That makes the stored best move the single fair answer, so answers are
//! checked against the stored review without any new engine work.
//!
//! Scheduling is a small Leitner system: a correct answer moves the position up
//! one box and schedules it after that box's interval in days (less
//! `DAY_SLACK_SECONDS`, so it is due that day rather than at that second); a wrong answer sends it
//! back to the first box and brings it back after `RETRY_SECONDS`.
use crate::{
    ApiResult, Query, analysis::expected_points, api_error, db::Database, now, pgn, tactics,
};
use anyhow::Result;
use rand::seq::SliceRandom;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use shakmaty::uci::UciMove;

/// Smallest expected-points margin between the best and second-best engine
/// line for a position to have one fair answer.
pub const MIN_GAP: f64 = 0.05;
/// Days until the next showing after a correct answer from box `i`.
pub const INTERVAL_DAYS: [i64; 6] = [1, 3, 7, 16, 35, 80];
/// A missed position comes back within the same sitting.
pub const RETRY_SECONDS: i64 = 600;
/// A position scheduled `n` days out is due this much earlier, so it is waiting at
/// any time of day `n` days later rather than at the exact second it was answered
/// (answered Monday 21:00, it is due again from Tuesday 09:00).
pub const DAY_SLACK_SECONDS: i64 = 12 * 3600;
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

/// Practice positions for an account, optionally only the one at `(game, ply)`.
fn candidates(
    connection: &Connection,
    account: i64,
    kind: Option<&str>,
    phase: Option<&str>,
    only: Option<(i64, i64)>,
) -> Result<Vec<Candidate>> {
    let mut statement = connection.prepare_cached(
        "SELECT m.game_id, m.ply, m.color, m.fen_before, m.san, m.uci, m.best_uci, m.best_san,
                m.classification, m.phase, m.ep_loss, m.explanation, m.lines_json,
                prev.uci, prev.san, g.end_time, g.time_class, g.rules,
                CASE WHEN m.color='w' THEN g.black_username ELSE g.white_username END,
                CASE WHEN m.color='w' THEN g.black_rating ELSE g.white_rating END,
                p.box, p.due_at, p.attempts, p.correct
         FROM account_games ag
         JOIN games g ON g.id = ag.game_id
         JOIN moves m ON m.game_id = ag.game_id AND m.color = substr(ag.color, 1, 1)
         LEFT JOIN moves prev ON prev.game_id = m.game_id AND prev.ply = m.ply - 1
         LEFT JOIN practice_items p
           ON p.account_id = ag.account_id AND p.game_id = m.game_id AND p.ply = m.ply
         WHERE ag.account_id = ?1
           AND m.classification IN ('blunder', 'mistake', 'miss')
           AND (?2 IS NULL OR m.classification = ?2)
           AND (?3 IS NULL OR m.phase = ?3)
           AND (?4 IS NULL OR (m.game_id = ?4 AND m.ply = ?5))
           AND m.best_uci IS NOT NULL AND m.best_uci <> m.uci
           AND m.lines_json IS NOT NULL
           AND g.rules = 'chess'
         ORDER BY g.end_time DESC, m.ply",
    )?;
    let rows = statement.query_map(
        params![account, kind, phase, only.map(|o| o.0), only.map(|o| o.1)],
        |row| {
            let color: String = row.get(2)?;
            let best_uci: String = row.get(6)?;
            let lines: String = row.get(12)?;
            let box_: Option<i64> = row.get(20)?;
            Ok((
                Candidate {
                    game_id: row.get(0)?,
                    ply: row.get(1)?,
                    fen: row.get(3)?,
                    san: row.get(4)?,
                    best_san: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
                    classification: row.get(8)?,
                    phase: row.get(9)?,
                    ep_loss: row.get::<_, Option<f64>>(10)?.unwrap_or(0.0),
                    explanation: row.get(11)?,
                    line_san: Vec::new(),
                    themes: Vec::new(),
                    last_uci: row.get(13)?,
                    last_san: row.get(14)?,
                    end_time: row.get(15)?,
                    time_class: row.get(16)?,
                    rules: row.get(17)?,
                    opponent: row.get(18)?,
                    opponent_rating: row.get(19)?,
                    state: match box_ {
                        Some(box_) => Some(State {
                            box_,
                            due_at: row.get(21)?,
                            attempts: row.get(22)?,
                            correct: row.get(23)?,
                        }),
                        None => None,
                    },
                    color: color.clone(),
                    best_uci: best_uci.clone(),
                },
                lines,
            ))
        },
    )?;
    let mut out = Vec::new();
    for row in rows {
        let (mut candidate, lines) = row?;
        let Some((gap, line_san)) =
            answer_line(&lines, candidate.color == "w", &candidate.best_uci)
        else {
            continue;
        };
        if gap < MIN_GAP {
            continue;
        }
        candidate.line_san = line_san;
        out.push(candidate);
    }
    Ok(out)
}

fn filter<'a>(q: &'a Query, key: &str, allowed: &[&str]) -> Result<Option<&'a str>> {
    match q.get(key).map(String::as_str) {
        None | Some("") | Some("all") => Ok(None),
        Some(value) if allowed.contains(&value) => Ok(Some(value)),
        Some(_) => Err(api_error(422, format!("Invalid {key}"))),
    }
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
    let mut all = db.read(|c| candidates(c, account, kind, phase, None))?;
    if let Some(since) = since {
        all.retain(|c| c.end_time.is_some_and(|t| t >= since));
    }
    // Pure detection runs after the read lock is released. Counts include scheduled
    // positions and remain scoped to every filter except the selected theme.
    let mut theme_counts = serde_json::Map::new();
    for &name in tactics::THEMES {
        theme_counts.insert(name.to_owned(), json!(0));
    }
    for candidate in &mut all {
        candidate.themes =
            tactics::detect(&candidate.fen, &candidate.best_uci, &candidate.line_san);
        for &name in &candidate.themes {
            if let Some(count) = theme_counts.get_mut(name) {
                *count = json!(count.as_u64().unwrap_or(0) + 1);
            }
        }
    }
    if let Some(theme) = theme {
        all.retain(|c| c.themes.contains(&theme));
    }
    let at = now();
    let items = if since.is_some() {
        let mut picked: Vec<&Candidate> = all.iter().collect();
        picked.shuffle(&mut rand::thread_rng());
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

/// Next box and due time after an answer.
pub fn schedule(box_: i64, correct: bool, at: i64) -> (i64, i64) {
    if correct {
        let index = box_.clamp(0, INTERVAL_DAYS.len() as i64 - 1) as usize;
        (
            box_ + 1,
            at + INTERVAL_DAYS[index] * 86_400 - DAY_SLACK_SECONDS,
        )
    } else {
        (0, at + RETRY_SECONDS)
    }
}

fn answer(db: &Database, account: i64, game: i64, ply: i64, body: &Value) -> ApiResult {
    // `{"reveal": true}` shows the answer without a move and counts as a miss.
    let reveal = match body.get("reveal") {
        None | Some(Value::Bool(false)) => false,
        Some(Value::Bool(true)) => true,
        Some(_) => return Err(api_error(422, "reveal must be true or false")),
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
        .read(|c| candidates(c, account, None, None, Some((game, ply))))?
        .into_iter()
        .next()
        .ok_or_else(|| api_error(404, "Practice position not found"))?;
    candidate.themes = tactics::detect(&candidate.fen, &candidate.best_uci, &candidate.line_san);
    debug_assert_eq!(candidate.rules, "chess");
    let played = raw.map(|raw| normalize(&candidate.fen, raw)).transpose()?;
    let correct = played.as_deref() == Some(candidate.best_uci.as_str());
    let at = now();
    let before = candidate.state;
    let (box_, due_at) = match before {
        // A correct answer before the position is due (the Today / recent ranges list positions
        // whether due or not) is extra practice: it keeps the schedule instead of moving the
        // position up a box, so repeating a day's games cannot mark it mastered within an hour.
        Some(s) if correct && s.due_at > at => (s.box_, s.due_at),
        _ => schedule(before.map_or(0, |s| s.box_), correct, at),
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
    let all = db.read(|c| candidates(c, account, None, None, None))?;
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

fn id(raw: &str, name: &str) -> Result<i64> {
    raw.parse::<i64>()
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(|| api_error(422, format!("{name} must be a positive integer")))
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
            if !body.is_object() {
                return Err(api_error(422, "Expected a JSON object"));
            }
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

    #[test]
    fn schedule_climbs_boxes_and_resets_on_a_miss() {
        assert_eq!(schedule(0, true, 0), (1, 86_400 - DAY_SLACK_SECONDS));
        assert_eq!(schedule(1, true, 0), (2, 3 * 86_400 - DAY_SLACK_SECONDS));
        assert_eq!(schedule(9, true, 0), (10, 80 * 86_400 - DAY_SLACK_SECONDS));
        assert_eq!(schedule(5, false, 100), (0, 100 + RETRY_SECONDS));
    }

    #[test]
    fn a_position_due_tomorrow_is_due_at_any_time_tomorrow() {
        // Answered Monday 2026-11-09 21:00 UTC; Tuesday's 09:00 sitting should include it.
        let monday_evening = 1_794_258_000;
        let tuesday_morning = monday_evening + 12 * 3600;
        let (_, due) = schedule(0, true, monday_evening);
        assert!(due <= tuesday_morning, "due {due} after Tuesday 09:00");
        // ... but not still on Monday evening.
        assert!(due > monday_evening + 6 * 3600);
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
