// Frozen candidate, queue and answer implementations from the pre-cache revision.
// Only names, fixed-time/RNG injection and the new internal gap field differ.
// Keeping the original SQL and response assembly independent detects ordering,
// state-join, filtering and response changes introduced by the optimized path.
/// Practice positions for an account, optionally only the one at `(game, ply)`.
fn legacy_candidates(
    connection: &Connection,
    account: i64,
    kind: Option<&str>,
    phase: Option<&str>,
    only: Option<(i64, i64)>,
) -> Result<Vec<Candidate>> {
    // The original query left games with equal end times in query-plan order; the reference
    // adds the same tiebreak the cache uses, so the comparison stays deterministic.
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
         ORDER BY g.end_time DESC, m.game_id DESC, m.ply",
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
                    gap: 0.0,
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
        candidate.gap = gap;
        candidate.line_san = line_san;
        out.push(candidate);
    }
    Ok(out)
}

fn legacy_queue(
    db: &Database,
    account: i64,
    q: &Query,
    rng: &mut impl rand::Rng,
    at: i64,
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
    let mut all = db.read(|c| legacy_candidates(c, account, kind, phase, None))?;
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

fn legacy_answer(
    db: &Database,
    account: i64,
    game: i64,
    ply: i64,
    body: &Value,
    at: i64,
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
        .read(|c| legacy_candidates(c, account, None, None, Some((game, ply))))?
        .into_iter()
        .next()
        .ok_or_else(|| api_error(404, "Practice position not found"))?;
    candidate.themes = tactics::detect(&candidate.fen, &candidate.best_uci, &candidate.line_san);
    debug_assert_eq!(candidate.rules, "chess");
    let played = raw.map(|raw| normalize(&candidate.fen, raw)).transpose()?;
    let correct = played.as_deref() == Some(candidate.best_uci.as_str());
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
    let all = db.read(|c| legacy_candidates(c, account, None, None, None))?;
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
