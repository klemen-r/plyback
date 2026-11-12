use super::*;
use crate::{api_error, now};
use chrono::{TimeZone, Utc};
use reqwest::blocking::Client;
use std::{io::Read, sync::OnceLock, time::Instant};
use url::Url;

/// The database stayed locked (busy review workers or a long request) for the sync's bounded
/// wait. Not a sync failure: the tick is skipped and the account is tried again on the next one.
#[derive(Debug)]
pub(super) struct SyncDbBusy;
/// How long an account waits after its sync found the database busy.
const SYNC_BUSY_RETRY: Duration = Duration::from_secs(45);
impl std::fmt::Display for SyncDbBusy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Sync database wait was cancelled or timed out")
    }
}
impl std::error::Error for SyncDbBusy {}

const OFFICIAL_BASE: &str = "https://api.chess.com/pub";
const MAX_RESPONSE: u64 = 32 * 1024 * 1024;
pub(super) struct ChessComClient {
    base: Url,
    client: OnceLock<Client>,
    serial: Mutex<Option<Instant>>,
}
struct ApiResponse {
    data: Value,
    not_modified: bool,
    etag: Option<String>,
    last_modified: Option<String>,
}

/// IDs, usernames and timestamps can all repeat after account deletion.
/// Every mutation checks the original row generation while holding the write
/// transaction, including errors and events generated after a network wait.
struct AccountSync {
    id: i64,
    generation: String,
}
impl AccountSync {
    fn from_row(account: &Value) -> Result<Self> {
        let id = account["id"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("Sync account has no ID"))?;
        let generation = account["sync_generation"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Sync account has no generation"))?
            .to_owned();
        Ok(Self { id, generation })
    }
    fn matches(&self, connection: &rusqlite::Connection) -> Result<bool> {
        Ok(connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=? AND sync_generation=?)",
            rusqlite::params![self.id, self.generation],
            |row| row.get(0),
        )?)
    }
    fn current(&self, service: &Services) -> Result<bool> {
        service
            .db
            .read_until(
                Instant::now() + Duration::from_secs(2),
                || service.stop.load(Ordering::SeqCst),
                |connection| self.matches(connection),
            )?
            .ok_or_else(|| SyncDbBusy.into())
    }
    fn transaction<T>(
        &self,
        service: &Services,
        mut f: impl FnMut(&rusqlite::Transaction<'_>) -> Result<T>,
    ) -> Result<Option<T>> {
        service
            .db
            .transaction_until(
                Instant::now() + Duration::from_secs(2),
                || service.stop.load(Ordering::SeqCst),
                |tx| {
                    if self.matches(tx)? {
                        f(tx).map(Some)
                    } else {
                        Ok(None)
                    }
                },
            )?
            .ok_or_else(|| SyncDbBusy.into())
    }
    fn event(&self, tx: &rusqlite::Transaction<'_>, level: &str, message: &str) -> Result<()> {
        crate::db::insert_event(tx, level, "sync", message, Some(self.id))
    }
}
impl ChessComClient {
    pub fn new() -> Result<Self> {
        let base = std::env::var("PLYBACK_CHESSCOM_API")
            .or_else(|_| std::env::var("CHESSCOM_API_BASE"))
            .unwrap_or_else(|_| OFFICIAL_BASE.to_string());
        let base = Url::parse(base.trim_end_matches('/'))?;
        if !matches!(base.scheme(), "http" | "https")
            || base.host_str().is_none()
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            anyhow::bail!("Invalid configured Chess.com API URL");
        }
        Ok(Self {
            base,
            client: OnceLock::new(),
            serial: Mutex::new(None),
        })
    }
    /// The configured API base without a trailing slash, so "{root}/player/..." never has "//"
    /// (a base without a path, such as "http://host", displays as "http://host/").
    fn root(&self) -> &str {
        self.base.as_str().trim_end_matches('/')
    }
    fn validate(&self, url: &Url) -> Result<()> {
        if url.scheme() != self.base.scheme()
            || url.host_str() != self.base.host_str()
            || url.port_or_known_default() != self.base.port_or_known_default()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !url
                .path()
                .starts_with(&format!("{}/", self.base.path().trim_end_matches('/')))
            || url.path().contains(['%', '\\'])
            || url.path().split('/').any(|p| p == "..")
        {
            return Err(api_error(
                502,
                "Refused a URL outside the configured Chess.com API",
            ));
        }
        Ok(())
    }
    pub fn profile(&self, username: &str, contact: &str, stop: &AtomicBool) -> Result<Value> {
        Ok(self
            .get(
                &format!("{}/player/{}", self.root(), username.to_ascii_lowercase()),
                None,
                None,
                contact,
                stop,
            )?
            .data)
    }
    fn get(
        &self,
        url: &str,
        etag: Option<&str>,
        last_modified: Option<&str>,
        contact: &str,
        stop: &AtomicBool,
    ) -> Result<ApiResponse> {
        let initial = Url::parse(url)?;
        self.validate(&initial)?;
        if self.client.get().is_none() {
            // Built lazily on a blocking API/service thread, never on a Tokio reactor.
            let built = Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .timeout(Duration::from_secs(30))
                .redirect(reqwest::redirect::Policy::none())
                .build()?;
            let _ = self.client.set(built);
        }
        let client = self.client.get().unwrap();
        let contact = contact
            .chars()
            .filter(|c| !c.is_control() && c.is_ascii())
            .take(120)
            .collect::<String>();
        let agent = format!(
            "Plyback/{} (self-hosted game review; +https://github.com/klemen-r/plyback){}",
            crate::APP_VERSION,
            if contact.is_empty() {
                String::new()
            } else {
                format!(" contact: {contact}")
            }
        );
        let mut last_error = "Chess.com unavailable".to_string();
        for attempt in 0..6 {
            if stop.load(Ordering::SeqCst) {
                return Err(api_error(503, "Request cancelled"));
            }
            let mut delay = 2u64.saturating_pow(attempt + 1).min(120);
            let result = (|| -> Result<ApiResponse> {
                let mut gate = loop {
                    if stop.load(Ordering::SeqCst) {
                        return Err(api_error(503, "Request cancelled"));
                    }
                    match self.serial.try_lock() {
                        Ok(gate) => break gate,
                        Err(std::sync::TryLockError::WouldBlock) => {
                            wait_cancel(Duration::from_millis(25), stop)?;
                        }
                        Err(std::sync::TryLockError::Poisoned(_)) => {
                            return Err(api_error(503, "Chess.com request lock failed"));
                        }
                    }
                };
                if stop.load(Ordering::SeqCst) {
                    return Err(api_error(503, "Request cancelled"));
                }
                if let Some(last) = *gate {
                    wait_cancel(
                        Duration::from_millis(350).saturating_sub(last.elapsed()),
                        stop,
                    )?;
                }
                let mut url = initial.clone();
                for redirects in 0..=5 {
                    if stop.load(Ordering::SeqCst) {
                        return Err(api_error(503, "Request cancelled"));
                    }
                    self.validate(&url)?;
                    let mut request = client
                        .get(url.clone())
                        .header("User-Agent", &agent)
                        .header("Accept", "application/json");
                    if let Some(etag) = etag {
                        request = request.header("If-None-Match", etag);
                    }
                    if let Some(last) = last_modified {
                        request = request.header("If-Modified-Since", last);
                    }
                    let response = request.send();
                    *gate = Some(Instant::now());
                    let response =
                        response.map_err(|_| api_error(502, "Could not reach Chess.com"))?;
                    if stop.load(Ordering::SeqCst) {
                        return Err(api_error(503, "Request cancelled"));
                    }
                    let status = response.status().as_u16();
                    if status == 304 {
                        return Ok(ApiResponse {
                            data: Value::Null,
                            not_modified: true,
                            etag: etag.map(str::to_string),
                            last_modified: last_modified.map(str::to_string),
                        });
                    }
                    if matches!(status, 301 | 302 | 303 | 307 | 308) {
                        if redirects == 5 {
                            return Err(api_error(502, "Too many Chess.com redirects"));
                        }
                        let location = response
                            .headers()
                            .get("location")
                            .and_then(|v| v.to_str().ok())
                            .ok_or_else(|| api_error(502, "Invalid Chess.com redirect"))?;
                        url = url.join(location)?;
                        self.validate(&url)?;
                        continue;
                    }
                    if matches!(status, 404 | 410) {
                        // Only the profile and archive list mean the player is gone; a missing
                        // monthly archive is just that archive.
                        return Err(api_error(
                            404,
                            if url.path().contains("/games/") && !url.path().ends_with("/archives")
                            {
                                format!("Chess.com archive not found: {}", url.path())
                            } else {
                                "Chess.com player not found".to_string()
                            },
                        ));
                    }
                    if status != 200 {
                        if let Some(wait) = response
                            .headers()
                            .get("retry-after")
                            .and_then(|s| s.to_str().ok())
                            .and_then(retry_after_seconds)
                        {
                            delay = delay.max(wait).min(120);
                        }
                        return Err(api_error(
                            if status == 429 || status >= 500 {
                                503
                            } else {
                                502
                            },
                            format!("Chess.com returned HTTP {status}"),
                        ));
                    }
                    if response.content_length().is_some_and(|n| n > MAX_RESPONSE) {
                        return Err(api_error(502, "Chess.com response is too large"));
                    }
                    let etag = response
                        .headers()
                        .get("etag")
                        .and_then(|s| s.to_str().ok())
                        .map(str::to_string);
                    let last_modified = response
                        .headers()
                        .get("last-modified")
                        .and_then(|s| s.to_str().ok())
                        .map(str::to_string);
                    let mut bytes = Vec::new();
                    response.take(MAX_RESPONSE + 1).read_to_end(&mut bytes)?;
                    if stop.load(Ordering::SeqCst) {
                        return Err(api_error(503, "Request cancelled"));
                    }
                    if bytes.len() as u64 > MAX_RESPONSE {
                        return Err(api_error(502, "Chess.com response is too large"));
                    }
                    let data = serde_json::from_slice(&bytes)
                        .map_err(|_| api_error(502, "Chess.com returned invalid JSON"))?;
                    return Ok(ApiResponse {
                        data,
                        not_modified: false,
                        etag,
                        last_modified,
                    });
                }
                unreachable!()
            })();
            match result {
                Ok(response) => return Ok(response),
                Err(error) => {
                    if let Some(crate::ApiError(status, _)) =
                        error.downcast_ref::<crate::ApiError>()
                        && (*status == 404
                            || (*status == 502 && !error.to_string().contains("Could not reach")))
                    {
                        return Err(error);
                    }
                    last_error = error.to_string();
                }
            }
            if attempt < 5 {
                wait_cancel(Duration::from_secs(delay), stop)?;
            }
        }
        Err(api_error(
            502,
            format!("Chess.com unavailable after 6 attempts: {last_error}"),
        ))
    }
    fn archive_url(&self, username: &str, raw: &str) -> Result<(String, String)> {
        let mut url = Url::parse(raw)?;
        if self.root() != OFFICIAL_BASE && raw.starts_with(&format!("{OFFICIAL_BASE}/")) {
            url = Url::parse(&format!("{}{}", self.root(), &raw[OFFICIAL_BASE.len()..]))?;
        }
        self.validate(&url)?;
        let prefix = format!(
            "{}/player/{}/games/",
            self.base.path().trim_end_matches('/'),
            username.to_ascii_lowercase()
        );
        let tail = url
            .path()
            .strip_prefix(&prefix)
            .ok_or_else(|| api_error(502, "Archive URL does not belong to this account"))?;
        if !valid_period(tail) {
            return Err(api_error(502, "Invalid archive month"));
        }
        Ok((tail.to_string(), url.to_string()))
    }
}
/// Seconds to wait from a Retry-After header: either delay-seconds or an HTTP date.
fn retry_after_seconds(value: &str) -> Option<u64> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(seconds);
    }
    let at = chrono::DateTime::parse_from_rfc2822(value).ok()?;
    Some((at.timestamp() - Utc::now().timestamp()).max(0) as u64)
}
fn wait_cancel(duration: Duration, stop: &AtomicBool) -> Result<()> {
    let until = Instant::now() + duration;
    while Instant::now() < until {
        if stop.load(Ordering::SeqCst) {
            return Err(api_error(503, "Request cancelled"));
        }
        std::thread::sleep(
            until
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(50)),
        );
    }
    Ok(())
}
pub(super) fn valid_period(period: &str) -> bool {
    let bytes = period.as_bytes();
    bytes.len() == 7
        && bytes[4] == b'/'
        && bytes[..4].iter().all(u8::is_ascii_digit)
        && bytes[5..].iter().all(u8::is_ascii_digit)
        && &period[..4] != "0000"
        && period[5..]
            .parse::<u32>()
            .is_ok_and(|m| (1..=12).contains(&m))
}
fn month_is_closed(period: &str) -> bool {
    let year = period[..4].parse::<i32>().unwrap_or(0);
    let month = period[5..].parse::<u32>().unwrap_or(0);
    let (year, month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    Utc.with_ymd_and_hms(year, month, 1, 0, 0, 0)
        .single()
        .is_some_and(|end| Utc::now().signed_duration_since(end).num_seconds() > 2 * 86400)
}
impl Services {
    fn sync_rows(&self, sql: &str, params: &[Value]) -> Result<Vec<Value>> {
        self.db
            .read_until(
                Instant::now() + Duration::from_secs(2),
                || self.stop.load(Ordering::SeqCst),
                |connection| crate::db::query_connection(connection, sql, params),
            )?
            .ok_or_else(|| SyncDbBusy.into())
    }
    fn sync_settings(&self) -> Result<Value> {
        settings::get_settings_until(&self.db, Instant::now() + Duration::from_secs(2), || {
            self.stop.load(Ordering::SeqCst)
        })?
        .ok_or_else(|| SyncDbBusy.into())
    }
    fn sync_error(&self, account: &Value, message: &str) -> Result<()> {
        let claim = AccountSync::from_row(account)?;
        claim.transaction(self, |tx| {
            tx.execute(
                "UPDATE accounts SET sync_status='error',sync_error=? WHERE id=?",
                rusqlite::params![message, claim.id],
            )?;
            claim.event(tx, "warning", &format!("Sync failed: {message}"))
        })?;
        self.sync_failed_at.lock().unwrap().insert(claim.id, now());
        Ok(())
    }
    pub(super) fn sync_loop(&self) {
        while !self.stop.load(Ordering::SeqCst) {
            match self.sync_tick() {
                Err(error) if error.is::<SyncDbBusy>() => {
                    tracing::debug!("Account sync tick skipped: database busy")
                }
                Err(error) => tracing::error!(%error, "Account sync tick failed"),
                Ok(()) => {}
            }
            if !self.stop.load(Ordering::SeqCst) {
                self.wait(Duration::from_secs(2));
            }
        }
    }
    fn sync_tick(&self) -> Result<()> {
        let settings = self.sync_settings()?;
        let accounts = self.sync_rows("SELECT * FROM accounts ORDER BY id", &[])?;
        let requested = std::mem::take(&mut *self.requested.lock().unwrap());
        let result = self.sync_due(&settings, &accounts, &requested);
        if result.is_err() {
            // A tick cut short keeps the manual requests it had not served yet.
            self.requested.lock().unwrap().extend(&requested);
        }
        result
    }
    fn sync_due(
        &self,
        settings: &Value,
        accounts: &[Value],
        requested: &HashSet<i64>,
    ) -> Result<()> {
        for account in accounts {
            if self.stop.load(Ordering::SeqCst) {
                break;
            }
            let id = account["id"].as_i64().unwrap();
            let active = account["last_new_game_at"]
                .as_i64()
                .is_some_and(|t| now() - t < 1800);
            let mut interval = settings[if active {
                "active_sync_interval_seconds"
            } else {
                "sync_interval_seconds"
            }]
            .as_i64()
            .unwrap_or(180);
            if account["sync_status"] == "error" {
                interval = interval.max(600);
            }
            let busy = self
                .sync_busy_until
                .lock()
                .unwrap()
                .get(&id)
                .is_some_and(|until| *until > Instant::now());
            // A failure leaves last_sync_at alone; the retry is paced from the failure.
            let last_attempt = account["last_sync_at"].as_i64().unwrap_or(0).max(
                self.sync_failed_at
                    .lock()
                    .unwrap()
                    .get(&id)
                    .copied()
                    .unwrap_or(0),
            );
            let due = requested.contains(&id)
                || (account["sync_enabled"] == 1 && now() - last_attempt >= interval);
            if busy {
                // Retried after a short pause; a manual request waits for it too.
                if requested.contains(&id) {
                    self.requested.lock().unwrap().insert(id);
                }
                continue;
            }
            if due {
                *self.current_sync.lock().unwrap() = Some(id);
                let mut result = self.sync_account(account);
                *self.current_sync.lock().unwrap() = None;
                if result.is_ok() {
                    self.sync_failed_at.lock().unwrap().remove(&id);
                }
                if let Err(error) = &result
                    && !error.is::<SyncDbBusy>()
                {
                    // Record the real failure. If recording it finds the database busy, retry
                    // the account soon; any other recording error ends the tick as before.
                    let message = error.to_string();
                    match self.sync_error(account, &message) {
                        Ok(()) => result = Ok(()),
                        Err(busy) if busy.is::<SyncDbBusy>() => result = Err(busy),
                        Err(other) => return Err(other),
                    }
                }
                match result {
                    Err(error) if error.is::<SyncDbBusy>() => {
                        // Not a failure: nothing was recorded, so the account stays due,
                        // but wait a little rather than refetching its archives every tick.
                        self.sync_busy_until
                            .lock()
                            .unwrap()
                            .insert(id, Instant::now() + SYNC_BUSY_RETRY);
                        if requested.contains(&id) {
                            self.requested.lock().unwrap().insert(id);
                        }
                    }
                    _ => {
                        self.sync_busy_until.lock().unwrap().remove(&id);
                    }
                }
            }
        }
        Ok(())
    }
    fn sync_account(&self, account: &Value) -> Result<()> {
        let claim = AccountSync::from_row(account)?;
        let id = claim.id;
        let username = account["username"].as_str().unwrap_or("");
        let initial = account["history_complete"].as_i64().unwrap_or(0) == 0;
        if claim
            .transaction(self, |tx| {
                Ok(tx.execute(
                    "UPDATE accounts SET sync_status='syncing',sync_error=NULL WHERE id=?",
                    [id],
                )?)
            })?
            .is_none()
        {
            return Ok(());
        }
        let settings = self.sync_settings()?;
        let contact = settings["contact"].as_str().unwrap_or("");
        if now() - account["profile_fetched_at"].as_i64().unwrap_or(0) > 21600 {
            let refresh = (|| -> Result<bool> {
                let profile = self.client.profile(username, contact, &self.stop)?;
                if !claim.current(self)? {
                    return Ok(false);
                }
                let stats = self
                    .client
                    .get(
                        &format!(
                            "{}/player/{}/stats",
                            self.client.root(),
                            username.to_ascii_lowercase()
                        ),
                        None,
                        None,
                        contact,
                        &self.stop,
                    )?
                    .data;
                Ok(claim.transaction(self, |tx| {
                    Ok(tx.execute("UPDATE accounts SET display_name=?,avatar=?,profile_json=?,stats_json=?,profile_fetched_at=? WHERE id=?", rusqlite::params![profile["username"].as_str().unwrap_or(username),profile["avatar"].as_str(),profile.to_string(),stats.to_string(),now(),id])?)
                })?.is_some())
            })();
            match refresh {
                Ok(false) => return Ok(()),
                Err(error) => {
                    if self.stop.load(Ordering::SeqCst)
                        || error
                            .downcast_ref::<crate::ApiError>()
                            .is_some_and(|e| e.0 == 404)
                    {
                        return Err(error);
                    }
                    tracing::warn!(%error,"Profile refresh deferred");
                }
                Ok(true) => {}
            }
        }
        if !claim.current(self)? {
            return Ok(());
        }
        let archives = self
            .client
            .get(
                &format!(
                    "{}/player/{}/games/archives",
                    self.client.root(),
                    username.to_ascii_lowercase()
                ),
                None,
                None,
                contact,
                &self.stop,
            )?
            .data;
        let urls = archives["archives"]
            .as_array()
            .ok_or_else(|| api_error(502, "Invalid Chess.com archive list"))?;
        if urls.len() > 2000 {
            return Err(api_error(502, "Chess.com archive list exceeds 2000 months"));
        }
        let mut periods = Vec::new();
        for raw in urls {
            let Some(raw) = raw.as_str() else { continue };
            let (period, url) = self.client.archive_url(username, raw)?;
            if account["import_since"]
                .as_str()
                .is_some_and(|since| period.as_str() < since)
            {
                continue;
            }
            periods.push((period, url));
        }
        periods.sort_by(|a, b| b.0.cmp(&a.0));
        periods.dedup_by(|a, b| a.0 == b.0);
        let mut new_count = 0;
        let mut fresh_count = 0;
        // When this account's archives were last read. A failed sync also moves last_sync_at
        // (for retry pacing) without reading any games, so it can't tell new games from history.
        let last_sync = self
            .sync_rows(
                "SELECT MAX(fetched_at) AS t FROM archives WHERE account_id=?",
                &[json!(id)],
            )?
            .into_iter()
            .next()
            .and_then(|row| row["t"].as_i64());
        let mut interrupted = false;
        for (period, url) in periods {
            if self.stop.load(Ordering::SeqCst) {
                interrupted = true;
                break;
            }
            if !claim.current(self)? {
                return Ok(());
            }
            let archive = self
                .sync_rows(
                    "SELECT ar.* FROM archives ar JOIN accounts a ON a.id=ar.account_id WHERE ar.account_id=? AND ar.period=? AND a.sync_generation=?",
                    &[json!(id), json!(period), json!(claim.generation)],
                )?
                .into_iter().next()
                .unwrap_or(Value::Null);
            if archive["complete"] == 1 {
                continue;
            }
            let response = self.client.get(
                &url,
                archive["etag"].as_str(),
                archive["last_modified"].as_str(),
                contact,
                &self.stop,
            )?;
            let complete = month_is_closed(&period);
            if response.not_modified {
                if claim.transaction(self, |tx| {
                    Ok(tx.execute("UPDATE archives SET fetched_at=?,complete=? WHERE account_id=? AND period=?", rusqlite::params![now(), complete, id, period])?)
                })?.is_none() { return Ok(()); }
                continue;
            }
            let games = response.data["games"]
                .as_array()
                .ok_or_else(|| api_error(502, "Invalid Chess.com game archive"))?;
            if games.len() > 20000 {
                return Err(api_error(502, "Monthly archive exceeds 20000 games"));
            }
            let mut rows = Vec::new();
            for game in games {
                if self.stop.load(Ordering::SeqCst) {
                    return Err(api_error(503, "Sync cancelled"));
                }
                match store::game_row_from_chesscom(game) {
                    Ok(Some(row)) => rows.push(row),
                    Ok(None) => {}
                    Err(error) => {
                        if claim
                            .transaction(self, |tx| {
                                claim.event(tx, "warning", &format!("Skipped invalid PGN: {error}"))
                            })?
                            .is_none()
                        {
                            return Ok(());
                        }
                    }
                }
            }
            let Some((history, fresh, analyze_history, auto_analyze)) = claim.transaction(self, |tx| {
                let mut results = Vec::with_capacity(rows.len());
                for batch in rows.chunks(128) {
                    if self.stop.load(Ordering::SeqCst) {
                        return Err(api_error(503, "Sync cancelled"));
                    }
                    let inserted = store::upsert_games(tx, batch)?;
                    results.extend(
                        inserted
                            .iter()
                            .zip(store::link_games(tx, id, username, batch, &inserted)?)
                            .map(|((game_id, _), linked)| (*game_id, linked)),
                    );
                }
                // Games newly linked to this account. During the first import, or when a changed
                // "import since" date backfills older archives, a game is history unless it ended
                // after the previous sync: that one was played since and is new.
                let (mut history, mut fresh) = (Vec::new(), Vec::new());
                for (row, (game_id, linked)) in rows.iter().zip(results) {
                    if !linked {
                        continue;
                    }
                    let ended = row["end_time"].as_i64().unwrap_or(0);
                    if initial && last_sync.is_none_or(|t| ended <= t) {
                        history.push(game_id);
                    } else {
                        fresh.push(game_id);
                    }
                }
                let (analyze_history, auto_analyze) = tx.query_row(
                    "SELECT analyze_history,auto_analyze FROM accounts WHERE id=?",
                    [id],
                    |row| Ok((row.get::<_, bool>(0)?, row.get::<_, bool>(1)?)),
                )?;
                // Save the pending jobs with the archive so they can survive a failed handoff.
                for (ids, priority, wanted) in [
                    (&history, PRIORITY_HISTORY, analyze_history),
                    (&fresh, PRIORITY_NEW, auto_analyze),
                ] {
                    for game_id in ids.iter().filter(|_| wanted) {
                        tx.execute(
                            "INSERT OR REPLACE INTO sync_pending(account_id,game_id,priority) VALUES(?,?,?)",
                            rusqlite::params![id, game_id, priority],
                        )?;
                    }
                }
                tx.execute("INSERT INTO archives(account_id,period,url,etag,last_modified,fetched_at,game_count,complete) VALUES(?,?,?,?,?,?,?,?) ON CONFLICT(account_id,period) DO UPDATE SET etag=excluded.etag,last_modified=excluded.last_modified,fetched_at=excluded.fetched_at,game_count=excluded.game_count,complete=excluded.complete",rusqlite::params![id,period,url,response.etag,response.last_modified,now(),rows.len()as i64,complete])?;
                Ok((history, fresh, analyze_history, auto_analyze))
            })?
            else {
                return Ok(());
            };
            new_count += history.len() + fresh.len();
            fresh_count += fresh.len();
            if analyze_history && !history.is_empty() {
                self.enqueue_for_account(id, &claim.generation, &history, PRIORITY_HISTORY)?;
            }
            if auto_analyze && !fresh.is_empty() {
                self.enqueue_for_account(id, &claim.generation, &fresh, PRIORITY_NEW)?;
            }
        }
        // Games are stored in the archive's transaction and queued afterwards, so a busy
        // database or a shutdown in between left them unqueued, and the next sync sees them
        // as already stored. `sync_pending` lists exactly those games; queueing removes the
        // row. Nothing else is caught up, so a cleared queue stays cleared and games imported
        // from a PGN without analysis stay unqueued.
        let pending = self.sync_rows(
            "SELECT p.game_id,p.priority FROM sync_pending p JOIN accounts a ON a.id=p.account_id WHERE p.account_id=? AND a.sync_generation=?",
            &[json!(id), json!(claim.generation)],
        )?;
        for priority in [PRIORITY_NEW, PRIORITY_HISTORY] {
            let missed = pending
                .iter()
                .filter(|row| row["priority"] == priority)
                .filter_map(|row| row["game_id"].as_i64())
                .collect::<Vec<_>>();
            if !missed.is_empty() {
                self.enqueue_for_account(id, &claim.generation, &missed, priority)?;
            }
        }
        if claim.transaction(self, |tx| {
            tx.execute("UPDATE accounts SET last_sync_at=?,sync_status='idle',sync_error=NULL,history_complete=CASE WHEN ? THEN history_complete ELSE 1 END,last_new_game_at=CASE WHEN ? THEN ? ELSE last_new_game_at END WHERE id=?",rusqlite::params![now(),interrupted,fresh_count>0,now(),id])?;
            if new_count > 0 { claim.event(tx, "info", &format!("Imported {new_count} game(s)"))?; }
            Ok(())
        })?.is_none() { return Ok(()); }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    /// The account was added when the newest base fixture game ended; the "extra" fixture
    /// game (1793525510) is played afterwards.
    const ADDED_AFTER_FIXTURE_GAMES: i64 = 1_793_524_537;
    #[test]
    fn archive_urls_must_stay_within_account_and_origin() {
        let c = ChessComClient {
            base: Url::parse(OFFICIAL_BASE).unwrap(),
            client: OnceLock::new(),
            serial: Mutex::new(None),
        };
        assert!(
            c.archive_url(
                "fixture",
                "https://api.chess.com/pub/player/fixture/games/2026/10"
            )
            .is_ok()
        );
        for url in [
            "https://evil.test/pub/player/fixture/games/2026/10",
            "https://api.chess.com.evil.test/pub/player/fixture/games/2026/10",
            "https://api.chess.com/pub/player/other/games/2026/10",
            "https://api.chess.com/pub/player/fixture/games/2026/99",
            "https://api.chess.com/pub/player/fixture/games/2026/%31%30",
            "https://user@api.chess.com/pub/player/fixture/games/2026/10",
        ] {
            assert!(c.archive_url("fixture", url).is_err(), "{url}");
        }
    }
    #[test]
    fn month_validation_is_ascii_and_bounded() {
        for good in ["2026/01", "2026/12"] {
            assert!(valid_period(good));
        }
        for bad in [
            "0000/01",
            "2026/00",
            "2026/13",
            "2026/1",
            "a026/10",
            "２０２６/10",
        ] {
            assert!(!valid_period(bad));
        }
    }

    struct FixtureServer {
        base: String,
        stop: Arc<AtomicBool>,
        extra: Arc<AtomicBool>,
        conditionals: Arc<AtomicU64>,
        handle: Option<std::thread::JoinHandle<()>>,
    }
    type PausedResponse = (
        String,
        std::sync::mpsc::SyncSender<()>,
        std::sync::mpsc::Receiver<()>,
    );
    impl FixtureServer {
        fn start() -> Self {
            Self::start_with_pause(None)
        }
        fn paused(
            suffix: &str,
        ) -> (
            Self,
            std::sync::mpsc::Receiver<()>,
            std::sync::mpsc::SyncSender<()>,
        ) {
            let (entered, blocked) = std::sync::mpsc::sync_channel(1);
            let (release, resume) = std::sync::mpsc::sync_channel(1);
            (
                Self::start_with_pause(Some((suffix.to_owned(), entered, resume))),
                blocked,
                release,
            )
        }
        fn start_with_pause(mut pause: Option<PausedResponse>) -> Self {
            use std::io::{BufRead, BufReader, Write};
            use std::net::TcpListener;
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = format!("http://{}/pub", listener.local_addr().unwrap());
            listener.set_nonblocking(true).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let extra = Arc::new(AtomicBool::new(false));
            let conditionals = Arc::new(AtomicU64::new(0));
            let (s, e, c) = (
                Arc::clone(&stop),
                Arc::clone(&extra),
                Arc::clone(&conditionals),
            );
            let handle = std::thread::spawn(move || {
                while !s.load(Ordering::SeqCst) {
                    let (mut socket, _) = match listener.accept() {
                        Ok(pair) => pair,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(10));
                            continue;
                        }
                        Err(error) => panic!("Fixture accept: {error}"),
                    };
                    // Winsock inherits the listener's nonblocking mode. This
                    // fixture uses blocking buffered I/O with bounded timeouts.
                    socket.set_nonblocking(false).unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    socket
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut reader = BufReader::new(socket.try_clone().unwrap());
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() {
                        continue;
                    }
                    let path = line.split_whitespace().nth(1).unwrap_or("").to_string();
                    let mut headers = String::new();
                    loop {
                        line.clear();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                            break;
                        }
                        headers.push_str(&line.to_ascii_lowercase());
                    }
                    if pause
                        .as_ref()
                        .is_some_and(|(suffix, _, _)| path.ends_with(suffix.as_str()))
                    {
                        let (_, entered, resume) = pause.take().unwrap();
                        let _ = entered.send(());
                        let _ = resume.recv_timeout(Duration::from_secs(5));
                    }
                    let mut body = if path.ends_with("/games/archives") {
                        serde_json::from_str(include_str!(
                            "../../../backend/tests/fixtures/chesscom/archives.json"
                        ))
                        .unwrap()
                    } else if path.ends_with("/stats") {
                        serde_json::from_str(include_str!(
                            "../../../backend/tests/fixtures/chesscom/stats.json"
                        ))
                        .unwrap()
                    } else if path.ends_with("/2026/10") {
                        serde_json::from_str(include_str!(
                            "../../../backend/tests/fixtures/chesscom/2026-10.json"
                        ))
                        .unwrap()
                    } else if path.ends_with("/2026/11") {
                        serde_json::from_str(include_str!(
                            "../../../backend/tests/fixtures/chesscom/2026-11.json"
                        ))
                        .unwrap()
                    } else {
                        serde_json::from_str::<Value>(include_str!(
                            "../../../backend/tests/fixtures/chesscom/profile.json"
                        ))
                        .unwrap()
                    };
                    let has_extra = e.load(Ordering::SeqCst);
                    if has_extra && path.ends_with("/2026/11") {
                        let more: Value = serde_json::from_str(include_str!(
                            "../../../backend/tests/fixtures/chesscom/2026-11-extra.json"
                        ))
                        .unwrap();
                        body["games"]
                            .as_array_mut()
                            .unwrap()
                            .extend(more["games"].as_array().unwrap().clone());
                    }
                    let etag = format!(
                        "\"fixture-{}\"",
                        u8::from(has_extra && path.ends_with("/2026/11"))
                    );
                    let conditional = headers.contains(&format!("if-none-match: {etag}"));
                    let data = body.to_string();
                    let response = if conditional {
                        c.fetch_add(1, Ordering::SeqCst);
                        "HTTP/1.1 304 Not Modified\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
                    } else {
                        format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nETag: {etag}\r\nConnection: close\r\n\r\n{data}",
                            data.len()
                        )
                    };
                    socket.write_all(response.as_bytes()).unwrap();
                }
            });
            Self {
                base,
                stop,
                extra,
                conditionals,
                handle: Some(handle),
            }
        }
    }
    impl Drop for FixtureServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(h) = self.handle.take() {
                h.join().unwrap();
            }
        }
    }

    #[test]
    fn delayed_profile_and_archive_cannot_modify_recreated_accounts() {
        for endpoint in ["/player/klemen_r", "/stats", "/2026/11"] {
            for replacement_name in ["klemen_r", "replacement_player"] {
                let (server, blocked, release) = FixtureServer::paused(endpoint);
                let db = Arc::new(Database::open(":memory:").unwrap());
                // Stats cases pause after the profile's early identity check,
                // exercising the write fence itself. Archive cases skip profile
                // refresh. No background workers are started.
                let fetched = if endpoint == "/2026/11" { now() } else { 0 };
                db.execute(
                    "INSERT INTO accounts(username,added_at,profile_fetched_at) VALUES('klemen_r',0,?)",
                    &[json!(fetched)],
                )
                .unwrap();
                let original = db.one("SELECT * FROM accounts", &[]).unwrap().unwrap();
                let mut service = Arc::try_unwrap(Services::new(Arc::clone(&db)).unwrap())
                    .ok()
                    .unwrap();
                service.client = ChessComClient {
                    base: Url::parse(&server.base).unwrap(),
                    client: OnceLock::new(),
                    serial: Mutex::new(None),
                };
                let service = Arc::new(service);
                let worker_service = Arc::clone(&service);
                let worker_account = original.clone();
                let worker =
                    std::thread::spawn(move || worker_service.sync_account(&worker_account));
                let reached = blocked.recv_timeout(Duration::from_secs(5)).is_ok();
                if !reached {
                    service.stop.store(true, Ordering::SeqCst);
                    let _ = release.send(());
                    let _ = worker.join();
                    panic!("Sync never reached blocked endpoint {endpoint}");
                }
                db.transaction(|tx| {
                    tx.execute("DELETE FROM accounts WHERE id=?", [original["id"].as_i64().unwrap()])?;
                    tx.execute("INSERT INTO accounts(username,display_name,profile_json,stats_json,added_at,auto_analyze,analyze_history) VALUES(?,'replacement','{\"new\":true}','{\"new\":true}',0,0,0)", [replacement_name])?;
                    Ok(())
                }).unwrap();
                let replacement = db.one("SELECT * FROM accounts", &[]).unwrap().unwrap();
                assert_eq!(replacement["id"], original["id"]);
                assert_ne!(replacement["sync_generation"], original["sync_generation"]);
                release.send(()).unwrap();
                worker.join().unwrap().unwrap();
                assert_eq!(
                    db.one("SELECT * FROM accounts", &[]).unwrap(),
                    Some(replacement.clone())
                );
                // A delayed error path must be fenced as strictly as success.
                service
                    .sync_error(&original, "late old-account failure")
                    .unwrap();
                assert_eq!(
                    db.one("SELECT * FROM accounts", &[]).unwrap(),
                    Some(replacement)
                );
                for table in ["games", "account_games", "archives", "jobs", "events"] {
                    assert_eq!(
                        db.scalar(&format!("SELECT COUNT(*) FROM {table}"), &[])
                            .unwrap(),
                        0,
                        "stale sync wrote {table}"
                    );
                }
            }
        }
    }

    #[test]
    fn account_generation_is_initialized_and_immutable() {
        let db = Database::open(":memory:").unwrap();
        db.execute(
            "INSERT INTO accounts(username,added_at) VALUES('fixture',0)",
            &[],
        )
        .unwrap();
        let generation = db
            .scalar("SELECT sync_generation FROM accounts", &[])
            .unwrap();
        assert_eq!(generation.as_str().unwrap().len(), 32);
        db.execute("UPDATE accounts SET display_name='changed'", &[])
            .unwrap();
        assert_eq!(
            db.scalar("SELECT sync_generation FROM accounts", &[])
                .unwrap(),
            generation
        );
        assert!(
            db.execute("UPDATE accounts SET sync_generation='replacement'", &[])
                .is_err()
        );
        assert_eq!(
            db.scalar("SELECT sync_generation FROM accounts", &[])
                .unwrap(),
            generation
        );
    }

    #[test]
    fn network_cancellation_does_not_wait_for_serial_lock() {
        let client = Arc::new(ChessComClient {
            base: Url::parse("http://127.0.0.1:9/pub").unwrap(),
            client: OnceLock::new(),
            serial: Mutex::new(None),
        });
        client
            .client
            .set(Client::builder().build().unwrap())
            .unwrap();
        let gate = client.serial.lock().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let worker_client = Arc::clone(&client);
        let (done, finished) = std::sync::mpsc::sync_channel(1);
        let worker = std::thread::spawn(move || {
            let result = worker_client.profile("fixture", "", &worker_stop);
            let _ = done.send(result);
        });
        std::thread::sleep(Duration::from_millis(50));
        stop.store(true, Ordering::SeqCst);
        let result = finished.recv_timeout(Duration::from_secs(1));
        drop(gate);
        worker.join().unwrap();
        let error = result
            .expect("Cancellation waited for another network request")
            .unwrap_err();
        assert_eq!(error.downcast_ref::<crate::ApiError>().unwrap().0, 503);
    }

    #[test]
    fn new_accounts_inherit_the_saved_auto_review_default_unless_overridden() {
        let server = FixtureServer::start();
        for (override_value, expected) in [(None, false), (Some(true), true)] {
            let db = Arc::new(Database::open(":memory:").unwrap());
            super::super::save_settings(&db, &json!({"auto_analyze_new": false})).unwrap();
            let mut service = Arc::try_unwrap(Services::new(db).unwrap()).ok().unwrap();
            service.client = ChessComClient {
                base: Url::parse(&server.base).unwrap(),
                client: OnceLock::new(),
                serial: Mutex::new(None),
            };
            let mut body = json!({"username": "klemen_r"});
            if let Some(value) = override_value {
                body["auto_analyze"] = json!(value);
            }
            let (status, account) = service
                .handle("POST", "/api/accounts", &crate::Query::new(), &body)
                .unwrap()
                .unwrap();
            assert_eq!(status, 201);
            assert_eq!(account["auto_analyze"], expected);
        }
    }

    #[test]
    fn fixture_sync_is_incremental_conditional_and_preserves_history_priority() {
        let server = FixtureServer::start();
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute(
            "INSERT INTO accounts(username,added_at,analyze_history) VALUES('klemen_r',?,0)",
            &[json!(ADDED_AFTER_FIXTURE_GAMES)],
        )
        .unwrap();
        let mut service = Arc::try_unwrap(Services::new(Arc::clone(&db)).unwrap())
            .ok()
            .unwrap();
        service.client = ChessComClient {
            base: Url::parse(&server.base).unwrap(),
            client: OnceLock::new(),
            serial: Mutex::new(None),
        };
        service
            .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
            .unwrap();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 11);
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM account_games", &[])
                .unwrap(),
            11
        );
        assert_eq!(db.scalar("SELECT COUNT(*) FROM jobs", &[]).unwrap(), 0);
        assert_eq!(
            db.scalar("SELECT history_complete FROM accounts", &[])
                .unwrap(),
            1
        );
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM games WHERE opening_family IS NOT NULL",
                &[]
            )
            .unwrap(),
            11
        );
        // Make the conditional-request assertion independent of the wall-clock month.
        db.execute("UPDATE archives SET complete=0", &[]).unwrap();
        service
            .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
            .unwrap();
        assert_eq!(server.conditionals.load(Ordering::SeqCst), 2);
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 11);
        server.extra.store(true, Ordering::SeqCst);
        db.execute("UPDATE archives SET complete=0", &[]).unwrap();
        service
            .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
            .unwrap();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 12);
        assert_eq!(db.scalar("SELECT COUNT(*) FROM jobs", &[]).unwrap(), 1);
        assert_eq!(
            db.scalar("SELECT priority FROM jobs", &[]).unwrap(),
            PRIORITY_NEW
        );
    }
    #[test]
    fn games_new_to_the_account_are_new_after_a_changed_import_date_or_another_import() {
        let server = FixtureServer::start();
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute(
            "INSERT INTO accounts(username,added_at,analyze_history,auto_analyze) VALUES('klemen_r',?,0,1)",
            &[json!(ADDED_AFTER_FIXTURE_GAMES)],
        )
        .unwrap();
        let mut service = Arc::try_unwrap(Services::new(Arc::clone(&db)).unwrap())
            .ok()
            .unwrap();
        service.client = ChessComClient {
            base: Url::parse(&server.base).unwrap(),
            client: OnceLock::new(),
            serial: Mutex::new(None),
        };
        let sync = || {
            service
                .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
                .unwrap()
        };
        sync();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM jobs", &[]).unwrap(), 0);
        // A changed "import since" date re-imports history (what PATCH /accounts does). One old
        // game is fetched again and stays history; the game played since the last sync is new.
        let last = db.scalar("SELECT MAX(end_time) FROM games", &[]).unwrap();
        db.execute(
            "DELETE FROM account_games WHERE game_id=(SELECT MIN(id) FROM games)",
            &[],
        )
        .unwrap();
        db.execute(
            "DELETE FROM games WHERE id=(SELECT MIN(id) FROM games)",
            &[],
        )
        .unwrap();
        // A failed attempt after that game ended moved last_sync_at past it without reading
        // any games: the game is still new.
        db.execute(
            "UPDATE accounts SET history_complete=0,last_sync_at=?",
            &[json!(now())],
        )
        .unwrap();
        db.execute("UPDATE archives SET complete=0,fetched_at=?", &[last])
            .unwrap();
        server.extra.store(true, Ordering::SeqCst);
        sync();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 12);
        assert_eq!(db.scalar("SELECT COUNT(*) FROM jobs", &[]).unwrap(), 1);
        assert_eq!(
            db.scalar("SELECT priority FROM jobs", &[]).unwrap(),
            PRIORITY_NEW
        );
        assert!(
            !db.scalar("SELECT last_new_game_at FROM accounts", &[])
                .unwrap()
                .is_null()
        );
        // A game already in the database (imported for another account or from a PGN file) but
        // not yet linked to this account is new to this account.
        let shared = db
            .scalar("SELECT MAX(game_id) FROM account_games WHERE game_id NOT IN (SELECT game_id FROM jobs)", &[])
            .unwrap();
        db.execute(
            "DELETE FROM account_games WHERE game_id=?",
            std::slice::from_ref(&shared),
        )
        .unwrap();
        // Another account fetches its own archive, so no conditional request applies.
        db.execute(
            "UPDATE archives SET complete=0,etag=NULL,last_modified=NULL",
            &[],
        )
        .unwrap();
        sync();
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM jobs WHERE game_id=? AND priority=?",
                &[shared, json!(PRIORITY_NEW)]
            )
            .unwrap(),
            1
        );
    }
    #[test]
    fn games_left_unqueued_by_a_failed_sync_are_queued_by_the_next_one() {
        let server = FixtureServer::start();
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute(
            "INSERT INTO accounts(username,added_at,analyze_history,auto_analyze) VALUES('klemen_r',?,0,1)",
            &[json!(ADDED_AFTER_FIXTURE_GAMES)],
        )
        .unwrap();
        let mut service = Arc::try_unwrap(Services::new(Arc::clone(&db)).unwrap())
            .ok()
            .unwrap();
        service.client = ChessComClient {
            base: Url::parse(&server.base).unwrap(),
            client: OnceLock::new(),
            serial: Mutex::new(None),
        };
        let sync = || {
            service
                .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
                .unwrap()
        };
        sync();
        let before = db.scalar("SELECT last_sync_at FROM accounts", &[]).unwrap();
        server.extra.store(true, Ordering::SeqCst);
        db.execute("UPDATE archives SET complete=0", &[]).unwrap();
        sync();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM jobs", &[]).unwrap(), 1);
        let pending = || {
            db.scalar("SELECT COUNT(*) FROM sync_pending", &[])
                .unwrap()
                .as_i64()
                .unwrap()
        };
        let owe = |game: &Value| {
            db.execute("DELETE FROM jobs", &[]).unwrap();
            db.execute(
                "INSERT INTO sync_pending(account_id,game_id,priority) VALUES(1,?,?)",
                &[game.clone(), json!(PRIORITY_NEW)],
            )
            .unwrap();
        };
        assert_eq!(pending(), 0);
        // As if queueing the new game had failed (busy database, shutdown): no job, the game is
        // still owed one, and the sync never finished. The archive is unchanged (304).
        let game = db.scalar("SELECT game_id FROM jobs", &[]).unwrap();
        owe(&game);
        db.execute("UPDATE accounts SET last_sync_at=?", &[before])
            .unwrap();
        db.execute("UPDATE archives SET complete=0", &[]).unwrap();
        sync();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM jobs", &[]).unwrap(), 1);
        assert_eq!(
            db.scalar("SELECT priority FROM jobs", &[]).unwrap(),
            PRIORITY_NEW
        );
        assert_eq!(pending(), 0);
        // A queue the user cleared stays cleared, also when the game was imported in the very
        // second the previous sync finished (imported_at == last_sync_at) or later.
        db.execute("DELETE FROM jobs", &[]).unwrap();
        db.execute("UPDATE archives SET complete=0", &[]).unwrap();
        db.execute(
            "UPDATE accounts SET last_sync_at=(SELECT MIN(imported_at) FROM games)",
            &[],
        )
        .unwrap();
        sync();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM jobs", &[]).unwrap(), 0);
        // After a recorded failure, the game still owed a job is queued anyway.
        owe(&game);
        service
            .sync_error(
                &db.one("SELECT * FROM accounts", &[]).unwrap().unwrap(),
                "archive list 502",
            )
            .unwrap();
        db.execute("UPDATE accounts SET last_sync_at=last_sync_at+3600", &[])
            .unwrap();
        sync();
        assert_eq!(db.scalar("SELECT game_id FROM jobs", &[]).unwrap(), game);
        assert_eq!(pending(), 0);
        // Clearing the queue also drops what a sync still meant to queue.
        owe(&game);
        service
            .handle("POST", "/api/queue/clear", &Query::new(), &Value::Null)
            .unwrap()
            .unwrap();
        assert_eq!(pending(), 0);
        // Auto-review switched off before the retry: the owed job is dropped, not queued.
        owe(&game);
        db.execute("UPDATE accounts SET auto_analyze=0", &[])
            .unwrap();
        sync();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM jobs", &[]).unwrap(), 0);
        assert_eq!(pending(), 0);
    }
    #[test]
    fn pgn_imports_without_analysis_are_not_queued_by_the_next_sync() {
        let server = FixtureServer::start();
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute(
            "INSERT INTO accounts(username,added_at,analyze_history,auto_analyze) VALUES('klemen_r',?,1,1)",
            &[json!(ADDED_AFTER_FIXTURE_GAMES)],
        )
        .unwrap();
        let mut service = Arc::try_unwrap(Services::new(Arc::clone(&db)).unwrap())
            .ok()
            .unwrap();
        service.client = ChessComClient {
            base: Url::parse(&server.base).unwrap(),
            client: OnceLock::new(),
            serial: Mutex::new(None),
        };
        let sync = || {
            service
                .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
                .unwrap()
        };
        sync();
        service
            .handle("POST", "/api/queue/clear", &Query::new(), &Value::Null)
            .unwrap()
            .unwrap();
        // The user's own game, played after the account was added, imported with analysis off.
        let pgn = "[White \"klemen_r\"]\n[Black \"someone\"]\n[UTCDate \"2027.01.02\"]\n[UTCTime \"10:00:00\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Nf3 1-0";
        let (_, imported) = service
            .handle(
                "POST",
                "/api/games/import",
                &Query::new(),
                &json!({"pgn":pgn,"analyze":false}),
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM account_games ag JOIN games g ON g.id=ag.game_id JOIN accounts a ON a.id=ag.account_id WHERE ag.game_id=? AND g.end_time>a.added_at",
                &[imported["game_ids"][0].clone()],
            )
            .unwrap(),
            1,
            "{imported}"
        );
        db.execute("UPDATE archives SET complete=0", &[]).unwrap();
        db.execute("UPDATE accounts SET last_sync_at=0", &[])
            .unwrap();
        sync();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM jobs", &[]).unwrap(), 0);
    }
    #[test]
    fn retry_after_accepts_seconds_and_http_dates() {
        assert_eq!(retry_after_seconds("30"), Some(30));
        assert_eq!(
            retry_after_seconds("Wed, 21 Oct 2015 07:28:00 GMT"),
            Some(0)
        );
        let soon = (Utc::now() + chrono::Duration::seconds(90)).to_rfc2822();
        assert!(retry_after_seconds(&soon).is_some_and(|s| (85..=90).contains(&s)));
        assert_eq!(retry_after_seconds("soon"), None);
    }
    #[test]
    fn an_api_base_without_a_path_builds_single_slash_urls() {
        let client = ChessComClient {
            base: Url::parse("http://127.0.0.1:8799").unwrap(),
            client: OnceLock::new(),
            serial: Mutex::new(None),
        };
        assert_eq!(client.root(), "http://127.0.0.1:8799");
        for raw in [
            "http://127.0.0.1:8799/player/fixture/games/2026/10",
            "https://api.chess.com/pub/player/fixture/games/2026/10",
        ] {
            let (period, url) = client.archive_url("fixture", raw).unwrap();
            assert_eq!(period, "2026/10");
            assert_eq!(url, "http://127.0.0.1:8799/player/fixture/games/2026/10");
        }
    }
    #[test]
    fn a_busy_database_skips_the_tick_instead_of_failing_the_sync() {
        let db = Arc::new(crate::db::Database::open(":memory:").unwrap());
        db.execute("INSERT INTO accounts(username,added_at,sync_enabled,last_sync_at) VALUES('fixture',0,1,0)", &[]).unwrap();
        let service = Services::new(db.clone()).unwrap();
        let holder = db.clone();
        let held = std::thread::spawn(move || {
            holder
                .transaction(|_| {
                    std::thread::sleep(Duration::from_secs(3));
                    Ok(())
                })
                .unwrap()
        });
        std::thread::sleep(Duration::from_millis(100));
        let error = service.sync_tick().unwrap_err();
        assert!(error.is::<SyncDbBusy>(), "{error}");
        held.join().unwrap();
        let account = db
            .one("SELECT sync_status, sync_error FROM accounts", &[])
            .unwrap()
            .unwrap();
        assert_ne!(account["sync_status"], "error");
        assert!(account["sync_error"].is_null());
        assert_eq!(db.scalar("SELECT COUNT(*) FROM events", &[]).unwrap(), 0);
    }
    #[test]
    fn an_account_that_found_the_database_busy_waits_before_retrying() {
        let db = Arc::new(crate::db::Database::open(":memory:").unwrap());
        db.execute("INSERT INTO accounts(username,added_at,sync_enabled,last_sync_at) VALUES('fixture',0,1,0)", &[]).unwrap();
        let service = Services::new(db.clone()).unwrap();
        service
            .sync_busy_until
            .lock()
            .unwrap()
            .insert(1, Instant::now() + SYNC_BUSY_RETRY);
        service.request_sync(1);
        // Due and requested, but still inside its busy pause: no sync (and no network).
        service.sync_tick().unwrap();
        assert!(service.requested.lock().unwrap().contains(&1));
        assert!(service.current_sync.lock().unwrap().is_none());
        let account = db
            .one("SELECT sync_status, last_sync_at FROM accounts", &[])
            .unwrap()
            .unwrap();
        assert_ne!(account["sync_status"], "syncing");
        assert_eq!(account["last_sync_at"], 0);
    }
    #[test]
    fn a_failed_sync_keeps_the_last_successful_time_and_waits_before_retrying() {
        let db = Arc::new(crate::db::Database::open(":memory:").unwrap());
        db.execute("INSERT INTO accounts(username,added_at,sync_enabled,last_sync_at) VALUES('fixture',0,1,1000)", &[]).unwrap();
        let service = Services::new(db.clone()).unwrap();
        let account = db.one("SELECT * FROM accounts", &[]).unwrap().unwrap();
        service.sync_error(&account, "archive list 502").unwrap();
        let account = db.one("SELECT * FROM accounts", &[]).unwrap().unwrap();
        assert_eq!(account["sync_status"], "error");
        assert_eq!(account["last_sync_at"], 1000);
        // last_sync_at alone says the account is long overdue, but the failure was just now:
        // no sync starts (and nothing reaches the network).
        service.sync_tick().unwrap();
        assert!(service.current_sync.lock().unwrap().is_none());
        assert_eq!(
            db.scalar("SELECT sync_status FROM accounts", &[]).unwrap(),
            "error"
        );
        assert!(service.sync_failed_at.lock().unwrap().contains_key(&1));
    }
}
