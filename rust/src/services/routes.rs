use super::*;
use crate::{
    ANALYSIS_VERSION, api_error,
    db::{query_connection, sql_values},
    now,
    params::{RangeError, bool_value, depth, id, object, query_bool, query_integer},
    pgn,
};
use rusqlite::params_from_iter;
use store::decode;

const INSERT_ACCOUNT: &str = "INSERT INTO accounts(platform,username,display_name,avatar,profile_json,added_at,auto_analyze,analyze_history,import_since,profile_fetched_at,stats_json) VALUES(?,?,?,?,?,?,?,?,?,?,?)";
const ACCOUNT_ORPHAN_GAMES: &str = "SELECT game_id FROM account_games WHERE account_id=? AND game_id IN(SELECT id FROM games WHERE source!='pgn') AND game_id NOT IN(SELECT game_id FROM account_games WHERE account_id!=?)";
const ACCOUNT_OPENINGS: &str = "SELECT g.opening_family AS family,COUNT(*) AS n FROM games g JOIN account_games ag ON ag.game_id=g.id WHERE ag.account_id=? AND g.opening_family IS NOT NULL GROUP BY 1 ORDER BY n DESC";
const ALL_OPENINGS: &str = "SELECT opening_family AS family,COUNT(*) AS n FROM games WHERE opening_family IS NOT NULL GROUP BY 1 ORDER BY n DESC";
const QUEUED_JOBS: &str = "SELECT j.id,j.game_id,j.priority,j.depth,j.status,j.attempts,j.created_at,j.claim_token,g.white_username,g.black_username,g.end_time,g.time_class FROM jobs j JOIN games g ON g.id=j.game_id WHERE j.status IN('queued','running') ORDER BY j.status='running' DESC,j.priority DESC,j.sort_key DESC LIMIT 50";
const FAILED_JOBS: &str = "SELECT j.id,j.game_id,j.error,j.attempts,j.finished_at,g.white_username,g.black_username FROM jobs j JOIN games g ON g.id=j.game_id WHERE j.status='failed' ORDER BY j.finished_at DESC LIMIT 50";
const GAME_COLUMNS: &str = "g.id,g.url,g.source,g.end_time,g.time_class,g.time_control,g.rated,g.rules,g.result,g.termination,g.plies,g.white_username,g.white_rating,g.white_result,g.black_username,g.black_rating,g.black_result,g.chesscom_accuracy_white,g.chesscom_accuracy_black,g.opening_eco,g.opening_name,g.opening_family";
const GAME_ANALYSIS_COLUMNS: &str =
    "ga.accuracy_white,ga.accuracy_black,ga.counts_json,ga.analyzed_at,ga.depth";
const GAME_JOBS: &str = "SELECT id,game_id,status,error,claim_token FROM jobs WHERE game_id IN";
const PENDING_JOB_STATUS: &str = "AND status IN('queued','running','failed') ORDER BY id";
fn pgn_import_platform(text: &str) -> Result<&'static str> {
    use pgn_reader::{RawTag, Reader, Visitor};
    use std::ops::ControlFlow;

    fn from_site(raw: &str) -> Option<&'static str> {
        let raw = raw.trim();
        let url = url::Url::parse(raw)
            .or_else(|_| url::Url::parse(&format!("https://{raw}")))
            .ok()?;
        if !matches!(url.scheme(), "https" | "http")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return None;
        }
        match url.host_str()? {
            "chess.com" | "www.chess.com" | "api.chess.com" => Some("chesscom"),
            "lichess.org" | "www.lichess.org" => Some("lichess"),
            _ => None,
        }
    }
    struct PlatformHeaders;
    impl Visitor for PlatformHeaders {
        type Tags = (Option<String>, Option<String>);
        type Movetext = ();
        type Output = &'static str;
        fn begin_tags(&mut self) -> ControlFlow<Self::Output, Self::Tags> {
            ControlFlow::Continue((None, None))
        }
        fn tag(
            &mut self,
            tags: &mut Self::Tags,
            name: &[u8],
            value: RawTag<'_>,
        ) -> ControlFlow<Self::Output> {
            match name {
                b"Site" => tags.0 = Some(value.decode_utf8_lossy().into_owned()),
                b"Link" => tags.1 = Some(value.decode_utf8_lossy().into_owned()),
                _ => {}
            }
            ControlFlow::Continue(())
        }
        fn begin_movetext(
            &mut self,
            (site, link): Self::Tags,
        ) -> ControlFlow<Self::Output, Self::Movetext> {
            // A game-specific link takes precedence over the broader site tag.
            ControlFlow::Break(
                link.as_deref()
                    .and_then(from_site)
                    .or_else(|| site.as_deref().and_then(from_site))
                    .unwrap_or("chesscom"),
            )
        }
        fn end_game(&mut self, _: Self::Movetext) -> Self::Output {
            "chesscom"
        }
    }
    Reader::build(text.as_bytes())
        .set_supported_tag_line_length(16_384)
        .finish()
        .read_game(&mut PlatformHeaders)
        .map_err(|_| api_error(400, "Malformed PGN platform headers"))
        .map(|platform| platform.unwrap_or("chesscom"))
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
            ("GET", ["api", "accounts", account]) => {
                Some(id(account).and_then(|id| Ok((200, self.account_public(&self.account(id)?)?))))
            }
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
    fn account_public(&self, account: &Value) -> Result<Value> {
        let mut public = store::account_public(&self.db, account)?;
        // Lichess keeps transient sync state in memory so cooldowns and polling do not
        // write account rows or invalidate data caches.
        if account["platform"] == "lichess"
            && account["id"]
                .as_i64()
                .is_some_and(|id| *self.current_sync.lock().unwrap() == Some(id))
        {
            public["sync_status"] = json!("syncing");
        }
        Ok(public)
    }
    fn list_accounts(&self) -> ApiResult {
        Ok((
            200,
            Value::Array(
                self.db
                    .query("SELECT * FROM accounts ORDER BY added_at", &[])?
                    .iter()
                    .map(|a| self.account_public(a))
                    .collect::<Result<Vec<_>>>()?,
            ),
        ))
    }
    fn add_account(&self, body: &Value) -> ApiResult {
        object(body)?;
        let platform = match body.get("platform") {
            None => "chesscom",
            Some(value) => value
                .as_str()
                .filter(|p| matches!(*p, "chesscom" | "lichess"))
                .ok_or_else(|| api_error(422, "platform must be chesscom or lichess"))?,
        };
        let platform_name = if platform == "lichess" {
            "Lichess"
        } else {
            "Chess.com"
        };
        let max_username = if platform == "lichess" { 30 } else { 40 };
        let username = body["username"]
            .as_str()
            .ok_or_else(|| api_error(422, "Username is required"))?
            .trim()
            .trim_start_matches('@');
        if !(2..=max_username).contains(&username.len())
            || !username
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(api_error(
                400,
                format!("That doesn't look like a valid {platform_name} username"),
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
        let settings = get_settings(&self.db)?;
        let auto = bool_value(
            body,
            "auto_analyze",
            settings["auto_analyze_new"].as_bool().unwrap_or(true),
        )?;
        if self
            .db
            .one(
                "SELECT id FROM accounts WHERE platform=? AND username=? COLLATE NOCASE",
                &[json!(platform), json!(username)],
            )?
            .is_some()
        {
            return Err(api_error(409, format!("{username} is already connected")));
        }
        let profile = if platform == "lichess" {
            self.lichess_client.profile(
                username,
                settings["contact"].as_str().unwrap_or(""),
                &self.stop,
            )
        } else {
            self.client.profile(
                username,
                settings["contact"].as_str().unwrap_or(""),
                &self.stop,
            )
        }
        .map_err(|error| {
            if error
                .downcast_ref::<crate::ApiError>()
                .is_some_and(|e| e.0 == 404)
            {
                api_error(404, format!("No {platform_name} player named '{username}'"))
            } else if error
                .downcast_ref::<crate::ApiError>()
                .is_some_and(|e| e.0 == 503)
            {
                error
            } else {
                api_error(
                    502,
                    format!("{platform_name} is not reachable right now: {error}"),
                )
            }
        })?;
        let canonical = profile["username"]
            .as_str()
            .filter(|s| {
                (2..=max_username).contains(&s.len())
                    && s.bytes()
                        .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
            })
            .unwrap_or(username);
        // Reuse the ratings returned with the initial Lichess profile.
        let lichess_profile = platform == "lichess";
        let id = self.db.transaction(|tx| {
            let result = tx.execute(
                INSERT_ACCOUNT,
                rusqlite::params![
                    platform,
                    canonical,
                    canonical,
                    profile["avatar"].as_str(),
                    profile.to_string(),
                    now(),
                    auto,
                    history,
                    since,
                    lichess_profile.then(now),
                    lichess_profile.then(|| json!({"perfs": profile["perfs"]}).to_string())
                ],
            );
            match result {
                Ok(_) => Ok(tx.last_insert_rowid()),
                Err(rusqlite::Error::SqliteFailure(e, _))
                    if e.code == rusqlite::ErrorCode::ConstraintViolation =>
                {
                    Err(api_error(409, format!("{username} is already connected")))
                }
                Err(error) => Err(error.into()),
            }
        })?;
        self.db.log_event(
            "info",
            "accounts",
            &format!("Connected {username}"),
            Some(id),
        );
        self.request_sync(id);
        Ok((201, self.account_public(&self.account(id)?)?))
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
                    tx.execute("DELETE FROM lichess_pending_games WHERE account_id=?", [id])?;
                    tx.execute(
                        "UPDATE accounts SET history_complete=0,lichess_cursor_ms=NULL,lichess_scan_ms=NULL,lichess_pending_ms=NULL,lichess_finished_ms=NULL WHERE id=?",
                        [id],
                    )?;
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
        Ok((200, self.account_public(&self.account(id)?)?))
    }
    fn delete_account(&self, id: i64) -> ApiResult {
        self.account(id)?;
        let orphans = self
            .db
            .query(ACCOUNT_ORPHAN_GAMES, &[json!(id), json!(id)])?;
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
        let removed = self.db.transaction(|tx| {
            // Re-evaluate ownership inside the same transaction as deletion.
            let orphans = query_connection(tx, ACCOUNT_ORPHAN_GAMES, &[json!(id), json!(id)])?;
            tx.execute("DELETE FROM accounts WHERE id=?", [id])?;
            for row in &orphans {
                tx.execute(
                    "DELETE FROM games WHERE id=?",
                    [row["game_id"].as_i64().unwrap()],
                )?;
            }
            Ok(orphans.len())
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
        let (texts, malformed_tail) = pgn::split_games(text)?;
        if texts.is_empty() {
            return Err(api_error(400, "No games found in the PGN"));
        }
        let mut rows = Vec::new();
        let mut platforms = Vec::new();
        let mut errors = Vec::new();
        for (index, text) in texts.iter().enumerate() {
            match store::game_row_from_pgn(text) {
                Ok(row) if row["plies"].as_u64().unwrap_or(0) > 0 => {
                    platforms.push(pgn_import_platform(text)?);
                    rows.push(row);
                }
                Ok(_) => errors.push(json!({"index":index,"error":if text.trim_start().starts_with('[') {
                    "Game has no moves"
                } else {
                    "This doesn't look like PGN: no tags such as [White \"…\"] and no moves such as 1. e4"
                }})),
                Err(error) => errors.push(json!({"index":index,"error":error.to_string()})),
            }
        }
        // The unreadable rest may hold any number of games, so it is not one skipped game:
        // say after how many complete games reading stopped.
        let unread_after = malformed_tail.then_some(texts.len());
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
            for account in query_connection(tx, "SELECT id,username,platform FROM accounts", &[])? {
                for ((row, platform), game_id) in rows.iter().zip(&platforms).zip(&result) {
                    if account["platform"] == *platform {
                        store::link_games(
                            tx,
                            account["id"].as_i64().unwrap(),
                            account["username"].as_str().unwrap(),
                            std::slice::from_ref(row),
                            std::slice::from_ref(game_id),
                        )?;
                    }
                }
            }
            // Imports from the Import page appear in Activity next to sync imports.
            let new = result.iter().filter(|(_, new)| *new).count();
            let existing = result.len() - new;
            let message = match (new, existing) {
                (n, 0) => format!("Imported {n} game(s) from PGN"),
                (0, e) => format!("PGN import: {e} game(s) already in Plyback"),
                (n, e) => format!("Imported {n} game(s) from PGN; {e} already in Plyback"),
            };
            crate::db::insert_event(tx, "info", "import", &message, None)?;
            // Queued in the same transaction: a failed queue write also undoes the import, so
            // an "Analyse immediately" import never leaves games stored without their jobs.
            let ids = result.iter().map(|(id, _)| *id).collect::<Vec<_>>();
            let queued = if analyze {
                self.enqueue_import(tx, &ids)?
            } else {
                0
            };
            Ok((result, ids, queued))
        })?;
        let (results, ids, queued) = results;
        if queued > 0 {
            self.notify();
        }
        Ok((
            200,
            json!({"imported":results.iter().filter(|(_,new)|*new).count(),"existing":results.iter().filter(|(_,new)|!*new).count(),"game_ids":ids,"errors":errors,"unread_after":unread_after}),
        ))
    }
    fn list_games(&self, q: &Query) -> ApiResult {
        let page = query_integer(q, "page", 1, 1, 1_000_000_000, RangeError::Bounds)?;
        let size = query_integer(q, "page_size", 50, 1, 200, RangeError::Bounds)?;
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
                params.push(json!(query_integer(
                    q,
                    key,
                    0,
                    0,
                    253402300799,
                    RangeError::Bounds
                )?));
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
        let columns = format!("{GAME_COLUMNS},{color} AS user_color,{GAME_ANALYSIS_COLUMNS}");
        let rows = self.db.query(
            &format!("SELECT {columns} {base} ORDER BY {order},g.id DESC LIMIT ? OFFSET ?"),
            &params,
        )?;
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
            self.db.query(ACCOUNT_OPENINGS, &[json!(id(account)?)])?
        } else {
            self.db.query(ALL_OPENINGS, &[])?
        };
        Ok((200, json!(rows)))
    }
    fn job_map(&self, ids: &[i64]) -> Result<HashMap<i64, Value>> {
        if ids.is_empty() {
            return Ok(HashMap::new());
        }
        let rows = self.db.query(
            &format!(
                "{GAME_JOBS}({}) {PENDING_JOB_STATUS}",
                vec!["?"; ids.len()].join(",")
            ),
            &ids.iter().map(|id| json!(id)).collect::<Vec<_>>(),
        )?;
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
                &[json!(crate::params::id(account)?), json!(id)],
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
        let mut queued = self.db.query(QUEUED_JOBS, &[])?;
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
        stats["failed_jobs"] = json!(self.db.query(FAILED_JOBS, &[])?);
        Ok((200, stats))
    }
    fn queue_action(&self, action: &str) -> ApiResult {
        match action {
            "pause" => self.set_paused(true)?,
            "resume" => self.set_paused(false)?,
            "retry-failed" => return Ok((200, json!({"retried":self.retry_failed()?}))),
            "clear" => {
                let cleared = self.db.transaction(|tx| {
                    tx.execute("DELETE FROM sync_pending", [])?;
                    tx.execute(
                        "UPDATE jobs SET followup_priority=NULL,followup_depth=NULL WHERE status='running' AND followup_depth IS NOT NULL",
                        [],
                    )?;
                    Ok(tx.execute("DELETE FROM jobs WHERE status='queued'", [])?)
                })?;
                return Ok((200, json!({"cleared":cleared})));
            }
            _ => return Err(api_error(404, "Unknown action")),
        }
        Ok((200, json!({"paused":self.paused.load(Ordering::SeqCst)})))
    }
}
fn game_summary(r: &Value, job: Option<&Value>) -> Value {
    let outcome = r["user_color"].as_str().and_then(|color| {
        let (res, other) = if color == "white" {
            (&r["white_result"], &r["black_result"])
        } else {
            (&r["black_result"], &r["white_result"])
        };
        if res == "win" {
            Some("win")
        } else if other == "win" {
            Some("loss")
        } else if res.is_null() && other.is_null() {
            // Stored without a result (PGN `*`): neither a draw nor in the draw filter.
            None
        } else {
            Some("draw")
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
    fn lichess_pgn_promotion_account_deletion_preserves_import_and_review() {
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute("INSERT INTO accounts(id,username,platform,added_at) VALUES(1,'fixture','lichess',0),(2,'other','lichess',0)",&[]).unwrap();
        let service = Services::new(db.clone()).unwrap();
        let row = store::game_row_from_pgn("[Site \"https://lichess.org/pgnfirst\"]\n[White \"fixture\"]\n[Black \"opponent\"]\n\n1. e4 e5 *").unwrap();
        let id = db
            .transaction(|tx| {
                let ids = store::upsert_games(tx, std::slice::from_ref(&row))?;
                store::link_games(tx, 1, "fixture", std::slice::from_ref(&row), &ids)?;
                let mut synced = row.clone();
                synced["source"] = json!("lichess");
                synced["rated"] = json!(1);
                store::upsert_games(tx, &[synced])?;
                Ok(ids[0].0)
            })
            .unwrap();
        db.execute("INSERT INTO game_analysis(game_id,depth,multipv,version,analyzed_at) VALUES(?,18,2,?,0)", &[json!(id),json!(ANALYSIS_VERSION)]).unwrap();
        db.execute("INSERT INTO jobs(game_id,status,depth,created_at,claim_token) VALUES(?,'running',20,0,'keep')", &[json!(id)]).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        service
            .progress
            .lock()
            .unwrap()
            .insert("keep".into(), (json!({"game_id":id}), cancel.clone()));
        // Platform-only orphans are removed; shared platform games survive.
        db.execute("INSERT INTO games(id,uuid,source,pgn,imported_at) VALUES(2,'lichess:platform','lichess','',0),(3,'lichess:shared','lichess','',0)",&[]).unwrap();
        db.execute(
            "INSERT INTO account_games VALUES(1,2,'white'),(1,3,'white'),(2,3,'black')",
            &[],
        )
        .unwrap();
        let games = db
            .query("SELECT * FROM games WHERE id!=2 ORDER BY id", &[])
            .unwrap();
        let analysis = db.query("SELECT * FROM game_analysis", &[]).unwrap();
        let jobs = db.query("SELECT * FROM jobs", &[]).unwrap();
        assert_eq!(service.delete_account(1).unwrap().1["games_removed"], 1);
        assert_eq!(
            db.query("SELECT * FROM games ORDER BY id", &[]).unwrap(),
            games
        );
        assert_eq!(
            db.query("SELECT * FROM game_analysis", &[]).unwrap(),
            analysis
        );
        assert_eq!(db.query("SELECT * FROM jobs", &[]).unwrap(), jobs);
        assert!(!cancel.load(Ordering::SeqCst));
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM account_games WHERE game_id=?",
                &[json!(id)]
            )
            .unwrap(),
            0
        );
    }
    #[test]
    fn accounts_api_reports_current_lichess_sync_without_writing() {
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute(
            "INSERT INTO accounts(id,platform,username,added_at,sync_status,sync_error) VALUES(1,'lichess','active',0,'error','previous failure'),(2,'lichess','other',1,'idle',NULL),(3,'chesscom','legacy',2,'syncing',NULL)",
            &[],
        ).unwrap();
        let service = Services::new(db.clone()).unwrap();
        let original_accounts = db.query("SELECT * FROM accounts ORDER BY id", &[]).unwrap();
        let revision = db
            .scalar("SELECT revision FROM data_revision", &[])
            .unwrap();
        let writes = db.scalar("SELECT total_changes()", &[]).unwrap();
        let get = |path: &str| {
            let (status, value) = service
                .handle("GET", path, &Query::new(), &Value::Null)
                .unwrap()
                .unwrap();
            assert_eq!(status, 200);
            value
        };

        // Merely requesting a sync (including cooldowns) keeps the durable status.
        service.request_sync(1);
        assert_eq!(get("/api/accounts/1")["sync_status"], "error");
        *service.current_sync.lock().unwrap() = Some(1);
        assert_eq!(get("/api/accounts/1")["sync_status"], "syncing");
        let accounts = get("/api/accounts");
        assert_eq!(accounts[0]["sync_status"], "syncing");
        assert_eq!(accounts[1]["sync_status"], "idle");
        assert_eq!(accounts[2]["sync_status"], "syncing");

        // Once the in-memory work ends, expose the stored status again.
        *service.current_sync.lock().unwrap() = None;
        assert_eq!(get("/api/accounts/1")["sync_status"], "error");
        assert_eq!(get("/api/accounts/1")["sync_error"], "previous failure");
        assert_eq!(
            db.query("SELECT * FROM accounts ORDER BY id", &[]).unwrap(),
            original_accounts
        );
        assert_eq!(
            db.scalar("SELECT revision FROM data_revision", &[])
                .unwrap(),
            revision
        );
        assert_eq!(db.scalar("SELECT total_changes()", &[]).unwrap(), writes);
    }

    #[test]
    fn changing_import_range_atomically_resets_only_that_accounts_pending_games() {
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute(
            "INSERT INTO accounts(id,platform,username,added_at,import_since,history_complete,lichess_cursor_ms,lichess_scan_ms,lichess_pending_ms,lichess_finished_ms) VALUES(1,'lichess','first',0,'2026/01',1,100,200,300,400),(2,'lichess','second',0,'2026/01',1,500,600,700,800)",
            &[],
        ).unwrap();
        db.execute(
            "INSERT INTO archives(account_id,period,url,complete) VALUES(1,'2026/01','fixture:first',1),(2,'2026/01','fixture:second',1)",
            &[],
        ).unwrap();
        db.execute(
            "INSERT INTO lichess_pending_games(account_id,game_id,created_at_ms) VALUES(1,'firstgame',100),(2,'othergame',500)",
            &[],
        ).unwrap();
        let service = Services::new(db.clone()).unwrap();
        let original_accounts = db.query("SELECT * FROM accounts ORDER BY id", &[]).unwrap();
        let original_archives = db
            .query("SELECT * FROM archives ORDER BY account_id", &[])
            .unwrap();
        let original_pending = db
            .query(
                "SELECT * FROM lichess_pending_games ORDER BY account_id",
                &[],
            )
            .unwrap();

        // A failed pending reset must roll back the range and archive changes too.
        db.execute("CREATE TRIGGER fail_pending_reset BEFORE DELETE ON lichess_pending_games BEGIN SELECT RAISE(ABORT,'fixture reset failure'); END", &[]).unwrap();
        assert!(
            service
                .patch_account(1, &json!({"import_since":"2025/01"}))
                .is_err()
        );
        assert_eq!(
            db.query("SELECT * FROM accounts ORDER BY id", &[]).unwrap(),
            original_accounts
        );
        assert_eq!(
            db.query("SELECT * FROM archives ORDER BY account_id", &[])
                .unwrap(),
            original_archives
        );
        assert_eq!(
            db.query(
                "SELECT * FROM lichess_pending_games ORDER BY account_id",
                &[]
            )
            .unwrap(),
            original_pending
        );
        db.execute("DROP TRIGGER fail_pending_reset", &[]).unwrap();

        service
            .patch_account(1, &json!({"auto_analyze":false}))
            .unwrap();
        assert_eq!(
            db.query(
                "SELECT * FROM lichess_pending_games ORDER BY account_id",
                &[]
            )
            .unwrap(),
            original_pending
        );
        service
            .patch_account(1, &json!({"import_since":"2025/01"}))
            .unwrap();
        let first = db
            .one("SELECT * FROM accounts WHERE id=1", &[])
            .unwrap()
            .unwrap();
        assert_eq!(first["import_since"], "2025/01");
        assert_eq!(first["history_complete"], 0);
        for field in [
            "lichess_cursor_ms",
            "lichess_scan_ms",
            "lichess_pending_ms",
            "lichess_finished_ms",
        ] {
            assert!(first[field].is_null(), "{field}");
        }
        assert_eq!(
            db.one("SELECT * FROM accounts WHERE id=2", &[])
                .unwrap()
                .unwrap(),
            original_accounts[1]
        );
        let archives = db
            .query("SELECT * FROM archives ORDER BY account_id", &[])
            .unwrap();
        assert_eq!(archives[0]["complete"], 0);
        assert_eq!(archives[1], original_archives[1]);
        assert_eq!(
            db.query(
                "SELECT * FROM lichess_pending_games ORDER BY account_id",
                &[]
            )
            .unwrap(),
            vec![original_pending[1].clone()]
        );
    }

    #[test]
    fn account_platform_and_lichess_username_validation_precede_network_requests() {
        let service = Services::new(Arc::new(Database::open(":memory:").unwrap())).unwrap();
        for body in [
            json!({"username":"fixture","platform":"unknown"}),
            json!({"username":"fixture","platform":null}),
            json!({"username":"fixture","platform":1}),
        ] {
            let error = service.add_account(&body).unwrap_err();
            assert_eq!(error.downcast_ref::<crate::ApiError>().unwrap().0, 422);
        }
        for username in [
            "a",
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "has space",
            "ümlaut",
            "../fixture",
            "fixture?x",
        ] {
            let error = service
                .add_account(&json!({"username":username,"platform":"lichess"}))
                .unwrap_err();
            assert_eq!(
                error.downcast_ref::<crate::ApiError>().unwrap().0,
                400,
                "{username}"
            );
            assert!(error.to_string().contains("Lichess"));
        }
    }
    #[test]
    fn adding_lichess_verifies_profile_allows_other_platform_and_explains_missing_user() {
        use std::io::{BufRead, BufReader, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = url::Url::parse(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
        let server = std::thread::spawn(move || {
            for expected in ["/api/user/fixture", "/api/user/missing"] {
                let (mut socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut request = String::new();
                let mut reader = BufReader::new(socket.try_clone().unwrap());
                loop {
                    let mut line = String::new();
                    reader.read_line(&mut line).unwrap();
                    if line == "\r\n" || line.is_empty() {
                        break;
                    }
                    request.push_str(&line);
                }
                assert!(
                    request.starts_with(&format!("GET {expected} HTTP/1.1\r\n")),
                    "{request}"
                );
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("user-agent: plyback/")
                );
                let (status, body) = if expected.ends_with("missing") {
                    ("404 Not Found", "{\"error\":\"Not found\"}")
                } else {
                    (
                        "200 OK",
                        "{\"id\":\"fixture\",\"username\":\"Fixture\",\"perfs\":{\"blitz\":{\"games\":12,\"rating\":1800}}}",
                    )
                };
                write!(socket, "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute(
            "INSERT INTO accounts(username,added_at) VALUES('fixture',0)",
            &[],
        )
        .unwrap();
        let mut service = Services::new(db).unwrap();
        Arc::get_mut(&mut service).unwrap().lichess_client = sync::LichessClient::fixture(base);
        let (status, account) = service
            .add_account(&json!({"username":"fixture","platform":"lichess"}))
            .unwrap();
        assert_eq!(status, 201);
        assert_eq!(account["username"], "Fixture");
        assert_eq!(account["platform"], "lichess");
        assert_eq!(account["ratings"]["blitz"]["rating"], 1800);
        for body in [
            json!({"username":"FIXTURE","platform":"lichess"}),
            json!({"username":"Fixture"}),
        ] {
            assert_eq!(
                service
                    .add_account(&body)
                    .unwrap_err()
                    .downcast_ref::<crate::ApiError>()
                    .unwrap()
                    .0,
                409
            );
        }
        let error = service
            .add_account(&json!({"username":"missing","platform":"lichess"}))
            .unwrap_err();
        assert_eq!(error.downcast_ref::<crate::ApiError>().unwrap().0, 404);
        assert!(
            error
                .to_string()
                .contains("No Lichess player named 'missing'")
        );
        assert_eq!(
            service.list_accounts().unwrap().1.as_array().unwrap().len(),
            2
        );
        server.join().unwrap();
    }
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

    fn platform_import_game(event: &str, headers: &str) -> String {
        format!("[Event \"{event}\"]\n[White \"FOO\"]\n[Black \"Rival\"]\n{headers}\n1. e4 e5 *")
    }

    #[test]
    fn pgn_import_links_each_game_only_to_the_platform_in_its_headers() {
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute("INSERT INTO accounts(id,username,platform,added_at) VALUES(1,'foo','chesscom',0),(2,'foo','lichess',0)", &[]).unwrap();
        let service = Services::new(db.clone()).unwrap();
        let cases = [
            (
                "chesscom",
                "[Site \"Chess.com\"]\n[Link \"https://www.chess.com/game/live/123\"]",
                1,
            ),
            ("lichess", "[Site \"https://lichess.org/AbCd1234\"]", 2),
            (
                "lichess-link",
                "[Site \"Online match\"]\n[Link \"https://lichess.org/ZyXw1234\"]",
                2,
            ),
            ("unknown", "[Site \"Local tournament\"]", 1),
            ("no-site", "", 1),
            (
                "spoofed-site",
                "[Site \"https://lichess.org.evil.test/game\"]",
                1,
            ),
            (
                "spoofed-link",
                "[Link \"https://evil.test/?site=lichess.org\"]",
                1,
            ),
            (
                "link-before-site",
                "[Site \"Chess.com\"]\n[Link \"https://lichess.org/QrSt1234\"]",
                2,
            ),
            (
                "chesscom-link-before-site",
                "[Site \"https://lichess.org\"]\n[Link \"https://www.chess.com/game/live/456\"]",
                1,
            ),
        ];
        let text = cases
            .iter()
            .map(|(event, headers, _)| platform_import_game(event, headers))
            .collect::<Vec<_>>()
            .join("\n\n");
        let (_, imported) = service
            .import_pgn(&json!({"pgn":text,"analyze":false}))
            .unwrap();
        assert_eq!(imported["imported"], cases.len());
        for ((event, _, account), id) in cases.iter().zip(imported["game_ids"].as_array().unwrap())
        {
            let links = db
                .query(
                    "SELECT account_id FROM account_games WHERE game_id=?",
                    std::slice::from_ref(id),
                )
                .unwrap();
            assert_eq!(links, vec![json!({"account_id":account})], "{event}");
        }
    }

    #[test]
    fn reimported_lichess_pgn_links_an_existing_game_without_cross_platform_membership() {
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute(
            "INSERT INTO accounts(id,username,platform,added_at) VALUES(1,'foo','chesscom',0)",
            &[],
        )
        .unwrap();
        let service = Services::new(db.clone()).unwrap();
        let body = json!({"pgn":platform_import_game("existing-game", "[Site \"https://lichess.org/AbCd1234\"]"),"analyze":false});
        let (_, first) = service.import_pgn(&body).unwrap();
        assert_eq!(first["imported"], 1);
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM account_games", &[])
                .unwrap(),
            0
        );
        db.execute(
            "INSERT INTO accounts(id,username,platform,added_at) VALUES(2,'foo','lichess',0)",
            &[],
        )
        .unwrap();
        for _ in 0..2 {
            let (_, again) = service.import_pgn(&body).unwrap();
            assert_eq!(again["existing"], 1);
            assert_eq!(again["imported"], 0);
            assert_eq!(again["game_ids"], first["game_ids"]);
            assert_eq!(
                db.query("SELECT account_id FROM account_games", &[])
                    .unwrap(),
                vec![json!({"account_id":2})]
            );
        }
    }

    #[test]
    fn pgn_platform_headers_use_exact_web_hosts_and_ignore_movetext_comments() {
        for site in [
            "https://lichess.org/a1B2c3D4",
            "lichess.org",
            "https://www.lichess.org/game",
        ] {
            assert_eq!(
                pgn_import_platform(&platform_import_game("site", &format!("[Site \"{site}\"]")))
                    .unwrap(),
                "lichess"
            );
        }
        for site in [
            "https://lichess.org.evil.test",
            "https://evil.test/lichess.org",
            "https://lichess.org@evil.test",
            "javascript://lichess.org",
            "mailto:foo@lichess.org",
        ] {
            assert_eq!(
                pgn_import_platform(&platform_import_game("site", &format!("[Site \"{site}\"]")))
                    .unwrap(),
                "chesscom",
                "{site}"
            );
        }
        assert_eq!(
            pgn_import_platform(
                "[Site \"Local\"]\n\n1. e4 { [Site \"https://lichess.org/game\"] } e5 *"
            )
            .unwrap(),
            "chesscom"
        );
    }

    #[test]
    fn pgn_import_preserves_complete_games_before_a_malformed_tail() {
        let db = Arc::new(Database::open(":memory:").unwrap());
        let services = Services::new(Arc::clone(&db)).unwrap();
        let good = "[Event \"Good\"]\n\n1. e4 e5 *";
        let broken = "[Event \"Broken\"]\n\n1. d4 {unterminated";
        let later = "[Event \"Later\"]\n\n1. c4 *";
        // The unclosed comment swallows the later game too: reading stops after one game,
        // which is reported as such, not as one skipped game.
        let (status, result) = services
            .handle(
                "POST",
                "/api/games/import",
                &Query::new(),
                &json!({"pgn": format!("{good}\n\n{broken}\n\n{later}"), "analyze": false}),
            )
            .unwrap()
            .unwrap();
        assert_eq!(status, 200);
        assert_eq!(result["imported"], 1);
        assert_eq!(result["errors"], json!([]));
        assert_eq!(result["unread_after"], 1);
        let (_, whole) = services
            .handle(
                "POST",
                "/api/games/import",
                &Query::new(),
                &json!({"pgn": later, "analyze": false}),
            )
            .unwrap()
            .unwrap();
        assert!(whole["unread_after"].is_null());
        db.execute(
            "DELETE FROM games WHERE id=?",
            &[whole["game_ids"][0].clone()],
        )
        .unwrap();
        assert_eq!(
            db.scalar("SELECT pgn FROM games", &[]).unwrap(),
            format!("{good}\n")
        );
        let error = services
            .import_pgn(&json!({"pgn": broken, "analyze": false}))
            .unwrap_err();
        assert_eq!(error.downcast_ref::<crate::ApiError>().unwrap().0, 400);
    }

    #[test]
    fn pgn_import_and_its_jobs_are_saved_together() {
        let db = Arc::new(Database::open(":memory:").unwrap());
        let s = Services::new(Arc::clone(&db)).unwrap();
        let reject_jobs = "CREATE TRIGGER reject_job BEFORE INSERT ON jobs BEGIN SELECT RAISE(ABORT, 'test job write failure'); END";
        db.execute(
            "INSERT INTO accounts(username,added_at) VALUES('me',0)",
            &[],
        )
        .unwrap();
        db.execute(reject_jobs, &[]).unwrap();
        let body =
            json!({"pgn": "[White \"Me\"]\n[Black \"Friend\"]\n\n1. e4 e5 *", "analyze": true});
        assert!(s.import_pgn(&body).is_err());
        for table in ["games", "account_games", "events", "jobs"] {
            assert_eq!(
                db.scalar(&format!("SELECT COUNT(*) FROM {table}"), &[])
                    .unwrap(),
                0,
                "{table}"
            );
        }
        db.execute("DROP TRIGGER reject_job", &[]).unwrap();
        let (status, imported) = s.import_pgn(&body).unwrap();
        assert_eq!(status, 200);
        assert_eq!(
            db.scalar("SELECT game_id FROM jobs WHERE status='queued'", &[])
                .unwrap(),
            imported["game_ids"][0]
        );
        // A failed import leaves no deeper-analysis request on a game that is being reviewed.
        db.execute(
            "UPDATE jobs SET status='running',depth=6 WHERE game_id=?",
            &[imported["game_ids"][0].clone()],
        )
        .unwrap();
        db.execute(reject_jobs, &[]).unwrap();
        let more = json!({"pgn": "[White \"Me\"]\n[Black \"Friend\"]\n\n1. e4 e5 *\n\n1. d4 d5 *", "analyze": true});
        assert!(s.import_pgn(&more).is_err());
        assert!(
            db.scalar("SELECT followup_depth FROM jobs", &[])
                .unwrap()
                .is_null()
        );
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 1);
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
    fn a_game_without_a_result_is_not_a_draw() {
        let db = Arc::new(Database::open(":memory:").unwrap());
        let s = Services::new(Arc::clone(&db)).unwrap();
        db.execute(
            "INSERT INTO accounts(id,username,added_at) VALUES (1,'me',0)",
            &[],
        )
        .unwrap();
        for (id, white, black) in [
            (1, json!("agreed"), json!("agreed")),
            (2, json!(null), json!(null)),
        ] {
            db.execute(
                "INSERT INTO games(id,uuid,pgn,white_result,black_result,imported_at) VALUES (?,?,'',?,?,0)",
                &[json!(id), json!(id.to_string()), white, black],
            )
            .unwrap();
            db.execute(
                "INSERT INTO account_games(account_id,game_id,color) VALUES (1,?,'white')",
                &[json!(id)],
            )
            .unwrap();
        }
        let list = |outcome: &str| {
            let q: Query = [("account_id", "1"), ("outcome", outcome)]
                .into_iter()
                .filter(|(_, v)| !v.is_empty())
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect();
            let (_, v) = s.list_games(&q).unwrap();
            v["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|g| (g["id"].as_i64().unwrap(), g["outcome"].clone()))
                .collect::<std::collections::BTreeMap<_, _>>()
        };
        let all = list("");
        assert_eq!(all[&1], "draw");
        assert!(all[&2].is_null(), "{}", all[&2]);
        assert_eq!(list("draw").keys().copied().collect::<Vec<_>>(), [1]);
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
