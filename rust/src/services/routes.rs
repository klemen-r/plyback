use super::*;
use crate::{
    ANALYSIS_VERSION, api_error,
    db::{query_connection, sql_values},
    now, pgn,
};
use rusqlite::params_from_iter;
use store::decode;

fn id(raw: &str) -> Result<i64> {
    raw.parse::<i64>()
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(|| api_error(422, "Invalid identifier"))
}
fn integer(q: &Query, key: &str, default: i64, min: i64, max: i64) -> Result<i64> {
    let n = match q.get(key) {
        Some(v) => v
            .parse()
            .map_err(|_| api_error(422, format!("Invalid {key}")))?,
        None => default,
    };
    if !(min..=max).contains(&n) {
        return Err(api_error(
            422,
            format!("{key} must be between {min} and {max}"),
        ));
    }
    Ok(n)
}
fn bool_value(value: &Value, key: &str, default: bool) -> Result<bool> {
    match value.get(key) {
        None => Ok(default),
        Some(Value::Bool(b)) => Ok(*b),
        _ => Err(api_error(422, format!("{key} must be true or false"))),
    }
}
fn query_bool(q: &Query, key: &str) -> Result<Option<bool>> {
    q.get(key)
        .map(|v| match v.to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Ok(true),
            "false" | "0" | "no" | "off" => Ok(false),
            _ => Err(api_error(422, format!("Invalid {key}"))),
        })
        .transpose()
}
fn depth(body: &Value) -> Result<Option<i64>> {
    match body.get("depth") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_i64()
            .filter(|v| (6..=30).contains(v))
            .map(Some)
            .ok_or_else(|| api_error(422, "Depth must be between 6 and 30")),
    }
}
fn object(body: &Value) -> Result<()> {
    if !body.is_object() {
        return Err(api_error(422, "Expected a JSON object"));
    }
    Ok(())
}

impl Services {
    pub(super) fn route(
        &self,
        method: &str,
        path: &str,
        q: &Query,
        body: &Value,
    ) -> Option<ApiResult> {
        let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
        match (method, parts.as_slice()) {
            ("GET", ["api", "accounts"]) => Some(self.list_accounts()),
            ("POST", ["api", "accounts"]) => Some(self.add_account(body)),
            ("GET", ["api", "accounts", account]) => Some(
                id(account)
                    .and_then(|id| Ok((200, store::account_public(&self.db, &self.account(id)?)?))),
            ),
            ("PATCH", ["api", "accounts", account]) => {
                Some(id(account).and_then(|id| self.patch_account(id, body)))
            }
            ("DELETE", ["api", "accounts", account]) => {
                Some(id(account).and_then(|id| self.delete_account(id)))
            }
            ("POST", ["api", "accounts", account, "sync"]) => Some(id(account).and_then(|id| {
                self.account(id)?;
                self.request_sync(id);
                Ok((200, json!({"requested":true})))
            })),
            ("POST", ["api", "accounts", account, "analyze"]) => {
                Some(id(account).and_then(|id| self.analyze_scope(id, body)))
            }
            ("GET", ["api", "games"]) => Some(self.list_games(q)),
            ("GET", ["api", "games", "openings"]) => Some(self.openings(q)),
            ("POST", ["api", "games", "import"]) => Some(self.import_pgn(body)),
            ("GET", ["api", "games", game]) => Some(id(game).and_then(|id| self.get_game(id, q))),
            ("POST", ["api", "games", game, "analyze"]) => Some(id(game).and_then(|id| {
                object(body)?;
                if self
                    .db
                    .one("SELECT id FROM games WHERE id=?", &[json!(id)])?
                    .is_none()
                {
                    return Err(api_error(404, "Game not found"));
                }
                let added = self.enqueue(
                    &[id],
                    PRIORITY_MANUAL,
                    depth(body)?,
                    bool_value(body, "force", false)?,
                )?;
                Ok((200, json!({"queued":added>0})))
            })),
            ("GET", ["api", "queue"]) => Some(self.queue()),
            ("POST", ["api", "queue", action]) => Some(self.queue_action(action)),
            _ => None,
        }
    }
    fn account(&self, id: i64) -> Result<Value> {
        self.db
            .one("SELECT * FROM accounts WHERE id=?", &[json!(id)])?
            .ok_or_else(|| api_error(404, "Account not found"))
    }
    fn list_accounts(&self) -> ApiResult {
        Ok((
            200,
            Value::Array(
                self.db
                    .query("SELECT * FROM accounts ORDER BY added_at", &[])?
                    .iter()
                    .map(|a| store::account_public(&self.db, a))
                    .collect::<Result<Vec<_>>>()?,
            ),
        ))
    }
    fn add_account(&self, body: &Value) -> ApiResult {
        object(body)?;
        let username = body["username"]
            .as_str()
            .ok_or_else(|| api_error(422, "Username is required"))?
            .trim()
            .trim_start_matches('@');
        if !(2..=40).contains(&username.len())
            || !username
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(api_error(
                400,
                "That doesn't look like a valid Chess.com username",
            ));
        }
        let since = body
            .get("import_since")
            .filter(|v| !v.is_null())
            .map(|v| {
                v.as_str()
                    .ok_or_else(|| api_error(422, "Invalid import_since"))
            })
            .transpose()?;
        if since.is_some_and(|s| !s.is_empty() && !sync::valid_period(s)) {
            return Err(api_error(400, "import_since must look like YYYY/MM"));
        }
        let history = bool_value(body, "analyze_history", true)?;
        let auto = bool_value(body, "auto_analyze", true)?;
        if self
            .db
            .one(
                "SELECT id FROM accounts WHERE username=?",
                &[json!(username)],
            )?
            .is_some()
        {
            return Err(api_error(409, format!("{username} is already connected")));
        }
        let settings = get_settings(&self.db)?;
        let profile = self
            .client
            .profile(
                username,
                settings["contact"].as_str().unwrap_or(""),
                &self.stop,
            )
            .map_err(|error| {
                if error
                    .downcast_ref::<crate::ApiError>()
                    .is_some_and(|e| e.0 == 404)
                {
                    api_error(404, format!("No Chess.com player named '{username}'"))
                } else {
                    api_error(
                        502,
                        format!("Chess.com is not reachable right now: {error}"),
                    )
                }
            })?;
        let canonical = profile["username"]
            .as_str()
            .filter(|s| {
                (2..=40).contains(&s.len())
                    && s.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            })
            .unwrap_or(username);
        let id=self.db.transaction(|tx|{
            let result=tx.execute("INSERT INTO accounts(username,display_name,avatar,profile_json,added_at,auto_analyze,analyze_history,import_since) VALUES(?,?,?,?,?,?,?,?)",rusqlite::params![canonical,canonical,profile["avatar"].as_str(),profile.to_string(),now(),auto,history,since]);
            match result {Ok(_)=>Ok(tx.last_insert_rowid()),Err(rusqlite::Error::SqliteFailure(e,_))if e.code==rusqlite::ErrorCode::ConstraintViolation=>Err(api_error(409,format!("{username} is already connected"))),Err(error)=>Err(error.into())}
        })?;
        self.db.log_event(
            "info",
            "accounts",
            &format!("Connected {username}"),
            Some(id),
        );
        self.request_sync(id);
        Ok((201, store::account_public(&self.db, &self.account(id)?)?))
    }
    fn patch_account(&self, id: i64, body: &Value) -> ApiResult {
        object(body)?;
        self.account(id)?;
        let mut fields = Vec::new();
        let mut values = Vec::new();
        for key in ["sync_enabled", "auto_analyze", "analyze_history"] {
            if let Some(value) = body.get(key) {
                if !value.is_boolean() {
                    return Err(api_error(422, "Account switches must be true or false"));
                }
                fields.push(format!("{key}=?"));
                values.push(value.clone());
            }
        }
        if let Some(value) = body.get("import_since") {
            if !value.is_null() && !value.is_string() {
                return Err(api_error(422, "Invalid import_since"));
            }
            if value
                .as_str()
                .is_some_and(|s| !s.is_empty() && !sync::valid_period(s))
            {
                return Err(api_error(400, "import_since must look like YYYY/MM"));
            }
            fields.push("import_since=?".into());
            values.push(value.clone());
        }
        if !fields.is_empty() {
            values.push(json!(id));
            self.db.transaction(|tx| {
                tx.execute(
                    &format!("UPDATE accounts SET {} WHERE id=?", fields.join(",")),
                    params_from_iter(sql_values(&values)?),
                )?;
                if body.get("import_since").is_some() {
                    tx.execute("UPDATE archives SET complete=0 WHERE account_id=?", [id])?;
                    tx.execute("UPDATE accounts SET history_complete=0 WHERE id=?", [id])?;
                }
                Ok(())
            })?;
        }
        if body.get("import_since").is_some() {
            self.request_sync(id);
        }
        if body["analyze_history"] == true {
            self.backfill(id)?;
        }
        Ok((200, store::account_public(&self.db, &self.account(id)?)?))
    }
    fn delete_account(&self, id: i64) -> ApiResult {
        self.account(id)?;
        let orphans=self.db.query("SELECT game_id FROM account_games WHERE account_id=? AND game_id NOT IN(SELECT game_id FROM account_games WHERE account_id!=?)",&[json!(id),json!(id)])?;
        let orphan_ids = orphans
            .iter()
            .filter_map(|r| r["game_id"].as_i64())
            .collect::<HashSet<_>>();
        for (progress, cancel) in self.progress.lock().unwrap().values() {
            if progress["game_id"]
                .as_i64()
                .is_some_and(|id| orphan_ids.contains(&id))
            {
                cancel.store(true, Ordering::SeqCst);
            }
        }
        let removed=self.db.transaction(|tx|{
            // Re-evaluate ownership inside the same transaction as deletion.
            let orphans=query_connection(tx,"SELECT game_id FROM account_games WHERE account_id=? AND game_id NOT IN(SELECT game_id FROM account_games WHERE account_id!=?)",&[json!(id),json!(id)])?;
            tx.execute("DELETE FROM accounts WHERE id=?",[id])?;
            for row in &orphans {tx.execute("DELETE FROM games WHERE id=?",[row["game_id"].as_i64().unwrap()])?;}Ok(orphans.len())
        })?;
        self.db.log_event(
            "info",
            "accounts",
            &format!("Removed account #{id} and {removed} game(s)"),
            None,
        );
        Ok((200, json!({"deleted":true,"games_removed":removed})))
    }
    fn analyze_scope(&self, id: i64, body: &Value) -> ApiResult {
        object(body)?;
        self.account(id)?;
        let filters =
            crate::insights::normalize_filters(body.get("filters").unwrap_or(&json!({})))?;
        let ids = crate::insights::scope_game_ids(&self.db, id, &filters)?;
        let added = self.enqueue(&ids, PRIORITY_REPORT, depth(body)?, false)?;
        Ok((200, json!({"scope":ids.len(),"queued":added})))
    }
    fn import_pgn(&self, body: &Value) -> ApiResult {
        object(body)?;
        let text = body["pgn"]
            .as_str()
            .ok_or_else(|| api_error(422, "PGN is required"))?;
        if text.len() > 20_000_000 {
            return Err(api_error(
                413,
                "PGN is too large: the limit is 20 MB. Split it into smaller files.",
            ));
        }
        let analyze = bool_value(body, "analyze", true)?;
        let texts = pgn::split_games(text).map_err(|e| api_error(400, e.to_string()))?;
        if texts.is_empty() {
            return Err(api_error(400, "No games found in the PGN"));
        }
        // `split_games` already stops at 10,000 games; keep one limit and one message.
        if texts.len() > pgn::MAX_IMPORT_GAMES {
            return Err(api_error(400, "Import at most 10000 games at a time"));
        }
        let mut rows = Vec::new();
        let mut errors = Vec::new();
        for (index, text) in texts.iter().enumerate() {
            match store::game_row_from_pgn(text) {
                Ok(row) if row["plies"].as_u64().unwrap_or(0) > 0 => rows.push(row),
                Ok(_) => errors.push(json!({"index":index,"error":if text.trim_start().starts_with('[') {
                    "Game has no moves"
                } else {
                    "This doesn't look like PGN: no tags such as [White \"…\"] and no moves such as 1. e4"
                }})),
                Err(error) => errors.push(json!({"index":index,"error":error.to_string()})),
            }
        }
        if rows.is_empty() {
            return Err(api_error(
                400,
                errors
                    .first()
                    .and_then(|e| e["error"].as_str())
                    .unwrap_or("No valid games found"),
            ));
        }
        let results = self.db.transaction(|tx| {
            let result = store::upsert_games(tx, &rows)?;
            for account in query_connection(tx, "SELECT id,username FROM accounts", &[])? {
                store::link_games(
                    tx,
                    account["id"].as_i64().unwrap(),
                    account["username"].as_str().unwrap(),
                    &rows,
                    &result,
                )?;
            }
            // Imports from the Import page appear in Activity next to sync imports.
            let new = result.iter().filter(|(_, new)| *new).count();
            let existing = result.len() - new;
            let message = match (new, existing) {
                (n, 0) => format!("Imported {n} game(s) from PGN"),
                (0, e) => format!("PGN import: {e} game(s) already in Plyback"),
                (n, e) => format!("Imported {n} game(s) from PGN; {e} already in Plyback"),
            };
            tx.execute(
                "INSERT INTO events(ts,level,source,message,account_id) VALUES(?,'info','import',?,NULL)",
                rusqlite::params![crate::now(), message],
            )?;
            // The same 5,000-event window as the other event writers.
            tx.execute(
                "DELETE FROM events WHERE id < (SELECT MAX(id)-5000 FROM events)",
                [],
            )?;
            Ok(result)
        })?;
        let ids = results.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        if analyze {
            self.enqueue(&ids, PRIORITY_MANUAL, None, false)?;
        }
        Ok((
            200,
            json!({"imported":results.iter().filter(|(_,new)|*new).count(),"existing":results.iter().filter(|(_,new)|!*new).count(),"game_ids":ids,"errors":errors}),
        ))
    }
    fn list_games(&self, q: &Query) -> ApiResult {
        let page = integer(q, "page", 1, 1, 1_000_000_000)?;
        let size = integer(q, "page_size", 50, 1, 200)?;
        let account = q.get("account_id").map(|v| id(v)).transpose()?;
        let mut params = Vec::new();
        let mut conditions = Vec::new();
        let (join, color) = if let Some(account) = account {
            params.push(json!(account));
            (
                "JOIN account_games ag ON ag.game_id=g.id AND ag.account_id=?",
                "ag.color",
            )
        } else {
            ("", "NULL")
        };
        if let Some(classes) = q.get("time_class").filter(|s| !s.is_empty()) {
            let classes = classes.split(',').collect::<Vec<_>>();
            if classes.len() > 20 {
                return Err(api_error(422, "Select at most 20 time classes"));
            }
            conditions.push(format!(
                "g.time_class IN({})",
                vec!["?"; classes.len()].join(",")
            ));
            params.extend(classes.into_iter().map(|c| json!(c)));
        }
        // Colour, result and "contains" read a game from a player's side: the account's
        // side when one is given, otherwise any connected account's side (so a request made
        // before the page knows the account still filters, instead of silently not).
        let side = |condition: &str| {
            if account.is_some() {
                condition.to_string()
            } else {
                format!(
                    "EXISTS(SELECT 1 FROM account_games ag WHERE ag.game_id=g.id AND {condition})"
                )
            }
        };
        if let Some(color) = q.get("color").filter(|s| !s.is_empty()) {
            if !matches!(color.as_str(), "white" | "black") {
                return Err(api_error(422, "color must be white or black"));
            }
            conditions.push(side("ag.color=?"));
            params.push(json!(color));
        }
        if let Some(outcome) = q.get("outcome").filter(|s| !s.is_empty()) {
            conditions.push(match outcome.as_str() {
                "win" => side("((ag.color='white' AND g.white_result='win') OR (ag.color='black' AND g.black_result='win'))"),
                "loss" => side("((ag.color='white' AND g.black_result='win') OR (ag.color='black' AND g.white_result='win'))"),
                "draw" => "g.white_result!='win' AND g.black_result!='win'".into(),
                _ => return Err(api_error(422, "outcome must be win, loss or draw")),
            });
        }
        if let Some(class) = q.get("classification").filter(|s| !s.is_empty()) {
            conditions.push(side("EXISTS(SELECT 1 FROM moves m WHERE m.game_id=g.id AND m.classification=? AND m.color=substr(ag.color,1,1))"));
            params.push(json!(class));
        }
        if let Some(ending) = q.get("ending").filter(|e| !e.is_empty()) {
            // The losing side's result code says how a decisive game ended.
            let code = match ending.as_str() {
                "checkmate" => "checkmated",
                "resignation" => "resigned",
                "time" => "timeout",
                "abandoned" => "abandoned",
                _ => {
                    return Err(api_error(
                        422,
                        "ending must be checkmate, resignation, time or abandoned",
                    ));
                }
            };
            conditions.push("(g.white_result=? OR g.black_result=?)".into());
            params.extend([json!(code), json!(code)]);
        }
        if let Some(opening) = q.get("opening") {
            conditions.push("g.opening_family=?".into());
            params.push(json!(opening));
        }
        if let Some(analyzed) = query_bool(q, "analyzed")? {
            conditions.push(
                if analyzed {
                    "ga.game_id IS NOT NULL"
                } else {
                    "ga.game_id IS NULL"
                }
                .into(),
            );
        }
        if let Some(rated) = query_bool(q, "rated")? {
            conditions.push("g.rated=?".into());
            params.push(json!(rated));
        }
        if let Some(search) = q.get("q") {
            if search.len() > 1000 {
                return Err(api_error(422, "Search is too long"));
            }
            conditions.push(
                "(g.white_username LIKE ? OR g.black_username LIKE ? OR g.opening_name LIKE ?)"
                    .into(),
            );
            params.extend(std::iter::repeat_n(json!(format!("%{search}%")), 3));
        }
        for (key, operator) in [("since", ">="), ("until", "<")] {
            if q.contains_key(key) {
                conditions.push(format!("g.end_time {operator} ?"));
                params.push(json!(integer(q, key, 0, 0, 253402300799)?));
            }
        }
        let where_sql = if conditions.is_empty() {
            String::new()
        } else {
            format!("WHERE {}", conditions.join(" AND "))
        };
        let order = match q.get("sort").map(String::as_str).unwrap_or("end_time_desc") {
            "end_time_asc" => "g.end_time ASC".to_string(),
            "accuracy_desc" => format!(
                "CASE {color} WHEN 'black' THEN ga.accuracy_black ELSE ga.accuracy_white END DESC"
            ),
            "accuracy_asc" => format!(
                "CASE {color} WHEN 'black' THEN ga.accuracy_black ELSE ga.accuracy_white END ASC"
            ),
            _ => "g.end_time DESC".into(),
        };
        let base = format!(
            "FROM games g {join} LEFT JOIN game_analysis ga ON ga.game_id=g.id {where_sql}"
        );
        let total = self
            .db
            .scalar(&format!("SELECT COUNT(*) {base}"), &params)?;
        params.push(json!(size));
        params.push(json!((page - 1) * size));
        let rows=self.db.query(&format!("SELECT g.id,g.url,g.source,g.end_time,g.time_class,g.time_control,g.rated,g.rules,g.result,g.termination,g.plies,g.white_username,g.white_rating,g.white_result,g.black_username,g.black_rating,g.black_result,g.chesscom_accuracy_white,g.chesscom_accuracy_black,g.opening_eco,g.opening_name,g.opening_family,{color} AS user_color,ga.accuracy_white,ga.accuracy_black,ga.counts_json,ga.analyzed_at,ga.depth {base} ORDER BY {order},g.id DESC LIMIT ? OFFSET ?"),&params)?;
        let jobs = self.job_map(
            &rows
                .iter()
                .filter_map(|r| r["id"].as_i64())
                .collect::<Vec<_>>(),
        )?;
        let items = rows
            .iter()
            .map(|row| game_summary(row, jobs.get(&row["id"].as_i64().unwrap())))
            .collect::<Vec<_>>();
        Ok((
            200,
            json!({"total":total,"page":page,"page_size":size,"items":items}),
        ))
    }
    fn openings(&self, q: &Query) -> ApiResult {
        let rows = if let Some(account) = q.get("account_id") {
            self.db.query("SELECT g.opening_family AS family,COUNT(*) AS n FROM games g JOIN account_games ag ON ag.game_id=g.id WHERE ag.account_id=? AND g.opening_family IS NOT NULL GROUP BY 1 ORDER BY n DESC",&[json!(id(account)?)])?
        } else {
            self.db.query("SELECT opening_family AS family,COUNT(*) AS n FROM games WHERE opening_family IS NOT NULL GROUP BY 1 ORDER BY n DESC",&[])?
        };
        Ok((200, json!(rows)))
    }
    fn job_map(&self, ids: &[i64]) -> Result<HashMap<i64, Value>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows=self.db.query(&format!("SELECT id,game_id,status,error,claim_token FROM jobs WHERE game_id IN({}) AND status IN('queued','running','failed') ORDER BY id",vec!["?";ids.len()].join(",")),&ids.iter().map(|id|json!(id)).collect::<Vec<_>>())?;
        let progress = self.progress.lock().unwrap();
        let mut out = HashMap::new();
        for mut row in rows {
            if let Some((p, _)) = row["claim_token"]
                .as_str()
                .and_then(|token| progress.get(token))
            {
                row["progress"] = p["done"].clone();
                row["total"] = p["total"].clone();
            }
            out.insert(row["game_id"].as_i64().unwrap(), row);
        }
        Ok(out)
    }
    fn get_game(&self, id: i64, q: &Query) -> ApiResult {
        let mut game = self
            .db
            .one("SELECT * FROM games WHERE id=?", &[json!(id)])?
            .ok_or_else(|| api_error(404, "Game not found"))?;
        let mut color = Value::Null;
        if let Some(account) = q.get("account_id") {
            color = self.db.scalar(
                "SELECT color FROM account_games WHERE account_id=? AND game_id=?",
                &[json!(crate_id(account)?), json!(id)],
            )?;
        }
        if color.is_null() {
            color = self.db.scalar(
                "SELECT color FROM account_games WHERE game_id=? ORDER BY account_id LIMIT 1",
                &[json!(id)],
            )?;
        }
        game["user_color"] = color;
        let analysis = self
            .db
            .one("SELECT * FROM game_analysis WHERE game_id=?", &[json!(id)])?;
        let jobs = self.job_map(&[id])?;
        if let Some(a) = &analysis {
            for key in [
                "accuracy_white",
                "accuracy_black",
                "counts_json",
                "analyzed_at",
                "depth",
            ] {
                game[key] = a[key].clone();
            }
        }
        let mut summary = game_summary(&game, jobs.get(&id));
        summary["pgn"] = game["pgn"].clone();
        summary["initial_fen"] = game["initial_fen"].clone();
        let parsed = pgn::parse(game["pgn"].as_str().unwrap_or(""));
        let mut moves = Vec::new();
        let analysis = if let Some(a) = analysis {
            for mut m in self.db.query(
                "SELECT * FROM moves WHERE game_id=? ORDER BY ply",
                &[json!(id)],
            )? {
                m.as_object_mut().unwrap().remove("game_id");
                m.as_object_mut().unwrap().remove("epd_before");
                m["tags"] = decode(&m["tags"], json!({}));
                m["lines"] = decode(&m["lines_json"], json!([]));
                m["reply"] = decode(&m["reply_json"], Value::Null);
                m["is_book"] = json!(m["is_book"].as_i64() == Some(1));
                m.as_object_mut().unwrap().remove("lines_json");
                m.as_object_mut().unwrap().remove("reply_json");
                moves.push(m);
            }
            let evals = decode(&a["evals_json"], json!([]));
            let win = evals
                .as_array()
                .map(|evals| {
                    evals
                        .iter()
                        .map(|cp| {
                            let cp = cp
                                .as_i64()
                                .unwrap_or(0)
                                .clamp(i32::MIN as i64, i32::MAX as i64)
                                as i32;
                            let win = crate::analysis::win_pct_white(cp);
                            json!((win * 10.).round() / 10.)
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            json!({"depth":a["depth"],"multipv":a["multipv"],"engine":a["engine"],"version":a["version"],"outdated":a["version"].as_i64().unwrap_or(0)<ANALYSIS_VERSION,"analyzed_at":a["analyzed_at"],"duration_ms":a["duration_ms"],"accuracy":{"white":a["accuracy_white"],"black":a["accuracy_black"]},"acpl":{"white":a["acpl_white"],"black":a["acpl_black"]},"counts":decode(&a["counts_json"],json!({})),"opening":{"eco":a["opening_eco"],"name":a["opening_name"],"family":a["opening_family"],"variation":a["opening_variation"]},"book_plies":a["book_plies"],"key_moments":decode(&a["key_moments_json"],json!([])),"evals":evals,"win_white":win})
        } else {
            match &parsed {
                Ok(parsed) => {
                    for m in &parsed.moves {
                        moves.push(json!({"ply":m.ply,"color":m.color,"san":m.san,"uci":m.uci,"fen_before":m.fen_before,"clock":m.clock}));
                    }
                }
                Err(error) => {
                    summary["pgn_error"] = json!(error.to_string());
                }
            }
            Value::Null
        };
        summary["final_fen"] = parsed
            .ok()
            .map(|p| json!(p.final_fen))
            .unwrap_or_else(|| game["initial_fen"].clone());
        Ok((
            200,
            json!({"game":summary,"analysis":analysis,"moves":moves}),
        ))
    }
    fn queue(&self) -> ApiResult {
        let mut stats = self.stats();
        let mut queued=self.db.query("SELECT j.id,j.game_id,j.priority,j.depth,j.status,j.attempts,j.created_at,j.claim_token,g.white_username,g.black_username,g.end_time,g.time_class FROM jobs j JOIN games g ON g.id=j.game_id WHERE j.status IN('queued','running') ORDER BY j.status='running' DESC,j.priority DESC,j.sort_key DESC LIMIT 50",&[])?;
        let progress = self.progress.lock().unwrap();
        for job in &mut queued {
            if let Some((p, _)) = job["claim_token"]
                .as_str()
                .and_then(|token| progress.get(token))
            {
                job["progress"] = p["done"].clone();
                job["total"] = p["total"].clone();
            }
            job.as_object_mut().unwrap().remove("claim_token");
        }
        drop(progress);
        stats["jobs"] = json!(queued);
        stats["failed_jobs"]=json!(self.db.query("SELECT j.id,j.game_id,j.error,j.attempts,j.finished_at,g.white_username,g.black_username FROM jobs j JOIN games g ON g.id=j.game_id WHERE j.status='failed' ORDER BY j.finished_at DESC LIMIT 50",&[])?);
        Ok((200, stats))
    }
    fn queue_action(&self, action: &str) -> ApiResult {
        match action {
            "pause" => self.set_paused(true)?,
            "resume" => self.set_paused(false)?,
            "retry-failed" => return Ok((200, json!({"retried":self.retry_failed()?}))),
            "clear" => {
                return Ok((
                    200,
                    json!({"cleared":self.db.execute("DELETE FROM jobs WHERE status='queued'",&[])?}),
                ));
            }
            _ => return Err(api_error(404, "Unknown action")),
        }
        Ok((200, json!({"paused":self.paused.load(Ordering::SeqCst)})))
    }
}
fn crate_id(raw: &str) -> Result<i64> {
    id(raw)
}
fn game_summary(r: &Value, job: Option<&Value>) -> Value {
    let outcome = r["user_color"].as_str().map(|color| {
        let (res, other) = if color == "white" {
            (&r["white_result"], &r["black_result"])
        } else {
            (&r["black_result"], &r["white_result"])
        };
        if res == "win" {
            "win"
        } else if other == "win" {
            "loss"
        } else {
            "draw"
        }
    });
    let mut out = json!({"white":{"username":r["white_username"],"rating":r["white_rating"],"result":r["white_result"],"accuracy":r["accuracy_white"],"chesscom_accuracy":r["chesscom_accuracy_white"]},"black":{"username":r["black_username"],"rating":r["black_rating"],"result":r["black_result"],"accuracy":r["accuracy_black"],"chesscom_accuracy":r["chesscom_accuracy_black"]},"opening":{"eco":r["opening_eco"],"name":r["opening_name"],"family":r["opening_family"]},"user_color":r["user_color"],"outcome":outcome,"analyzed":!r["analyzed_at"].is_null(),"analysis_depth":r["depth"],"counts":decode(&r["counts_json"],Value::Null),"rated":r["rated"].as_i64().map(|v|v!=0),"job":job.map(|j|{let mut out=json!({});for key in ["status","error","progress","total"]{if let Some(v)=j.get(key){out[key]=v.clone();}}out})});
    for key in [
        "id",
        "url",
        "source",
        "end_time",
        "time_class",
        "time_control",
        "rules",
        "result",
        "termination",
        "plies",
    ] {
        out[key] = r[key].clone();
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn import_is_deduplicated_and_bad_games_are_reported() {
        let db = Arc::new(Database::open(":memory:").unwrap());
        let s = Services::new(db.clone()).unwrap();
        let input = json!({"pgn":"[Event \"Fixture\"]\n[White \"Alice\"]\n[Black \"Bob\"]\n\n1. e4 e5 2. Nf3 Nc6 *","analyze":false});
        let (_, first) = s.import_pgn(&input).unwrap();
        assert_eq!(first["imported"], 1);
        let (_, second) = s.import_pgn(&input).unwrap();
        assert_eq!(second["existing"], 1);
        let (_, game) = s
            .get_game(first["game_ids"][0].as_i64().unwrap(), &Query::new())
            .unwrap();
        assert_eq!(game["moves"].as_array().unwrap().len(), 4);
        assert!(!game["game"]["final_fen"].is_null());
        // Both imports are logged for Activity.
        let events = db
            .query("SELECT source, message FROM events ORDER BY id", &[])
            .unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["source"], "import");
        assert_eq!(events[0]["message"], "Imported 1 game(s) from PGN");
        assert_eq!(
            events[1]["message"],
            "PGN import: 1 game(s) already in Plyback"
        );
    }
    #[test]
    fn games_filter_by_how_they_ended() {
        let db = Arc::new(Database::open(":memory:").unwrap());
        let s = Services::new(Arc::clone(&db)).unwrap();
        for (id, white, black) in [
            (1, "win", "checkmated"),
            (2, "timeout", "win"),
            (3, "win", "timeout"),
            (4, "resigned", "win"),
            (5, "agreed", "agreed"),
        ] {
            db.execute(
                "INSERT INTO games(id,uuid,pgn,white_result,black_result,imported_at) VALUES (?,?,'',?,?,0)",
                &[json!(id), json!(id.to_string()), json!(white), json!(black)],
            )
            .unwrap();
        }
        let ids = |ending: &str| {
            let q: Query = [("ending".to_string(), ending.to_string())]
                .into_iter()
                .collect();
            let (_, v) = s.list_games(&q).unwrap();
            let mut ids: Vec<i64> = v["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|g| g["id"].as_i64().unwrap())
                .collect();
            ids.sort_unstable();
            ids
        };
        assert_eq!(ids("time"), [2, 3]);
        assert_eq!(ids("checkmate"), [1]);
        assert_eq!(ids("resignation"), [4]);
        // Player-side filters with and without an account (all connected accounts).
        db.execute(
            "INSERT INTO accounts(id,username,added_at) VALUES(1,'a',0),(2,'b',0)",
            &[],
        )
        .unwrap();
        db.execute(
            "INSERT INTO account_games(account_id,game_id,color) VALUES(1,1,'white'),(1,2,'white'),(2,4,'black')",
            &[],
        )
        .unwrap();
        let side = |pairs: &[(&str, &str)]| {
            let q: Query = pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            s.list_games(&q).map(|(_, v)| {
                let mut ids: Vec<i64> = v["items"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|g| g["id"].as_i64().unwrap())
                    .collect();
                ids.sort_unstable();
                ids
            })
        };
        assert_eq!(side(&[("outcome", "win")]).unwrap(), [1, 4]);
        assert_eq!(
            side(&[("outcome", "win"), ("account_id", "1")]).unwrap(),
            [1]
        );
        assert_eq!(side(&[("outcome", "loss")]).unwrap(), [2]);
        assert_eq!(side(&[("color", "black")]).unwrap(), [4]);
        assert_eq!(
            side(&[("color", "white"), ("account_id", "2")]).unwrap(),
            Vec::<i64>::new()
        );
        assert_eq!(side(&[("outcome", "draw")]).unwrap(), [5]);
        for bad in [("color", "purple"), ("outcome", "maybe")] {
            assert_eq!(
                side(&[bad])
                    .unwrap_err()
                    .downcast_ref::<crate::ApiError>()
                    .unwrap()
                    .0,
                422
            );
        }
        assert_eq!(ids(""), [1, 2, 3, 4, 5]);
        let q: Query = [("ending".to_string(), "boredom".to_string())]
            .into_iter()
            .collect();
        assert_eq!(
            s.list_games(&q)
                .unwrap_err()
                .downcast_ref::<crate::ApiError>()
                .unwrap()
                .0,
            422
        );
    }
    #[test]
    fn queue_reuses_completed_depth_and_bumps_queued_work() {
        let db = Arc::new(Database::open(":memory:").unwrap());
        let s = Services::new(Arc::clone(&db)).unwrap();
        let (_, v) = s
            .import_pgn(&json!({"pgn":"1. e4 e5 *","analyze":false}))
            .unwrap();
        let id = v["game_ids"][0].as_i64().unwrap();
        assert_eq!(s.enqueue(&[id, id], 10, Some(8), false).unwrap(), 1);
        assert_eq!(s.enqueue(&[id], 100, Some(12), false).unwrap(), 0);
        let job = db
            .one("SELECT priority,depth FROM jobs", &[])
            .unwrap()
            .unwrap();
        assert_eq!(job["priority"], 100);
        assert_eq!(job["depth"], 12);
        s.queue_action("clear").unwrap();
        assert_eq!(s.stats()["queued"], 0);
    }
}
