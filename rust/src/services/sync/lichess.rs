//! Official Lichess export API. Streams are spooled before publication so an
//! interrupted export publishes only complete NDJSON records. Discovery advances
//! by creation time; ongoing games are persisted and rechecked separately by ID.
use super::*;
use std::io::{BufRead, BufReader, Seek, Write};
mod mapping;
use mapping::game_row;

const BASE: &str = "https://lichess.org";
const MAX_LINE: usize = 4 * 1024 * 1024;
const EXPORT_SLICE: Duration = Duration::from_secs(15);
const PROFILE_LOCK_WAIT: Duration = Duration::from_secs(5);
const PENDING_POLL_SECONDS: i64 = 60;
const PENDING_BATCH_LIMIT: usize = 300;
const PENDING_MISS_LIMIT: i64 = 3;
const NO_PROGRESS_ERROR_THRESHOLD: u32 = 3;

#[derive(Clone, PartialEq, Eq)]
struct RetryScope {
    generation: String,
    import_since: Option<String>,
    cursor: Option<i64>,
}
impl RetryScope {
    fn from_account(account: &Value) -> Self {
        Self {
            generation: account["sync_generation"]
                .as_str()
                .unwrap_or_default()
                .to_owned(),
            import_since: account["import_since"].as_str().map(str::to_owned),
            cursor: account["lichess_cursor_ms"].as_i64(),
        }
    }
}
struct NoProgressRetry {
    scope: RetryScope,
    failures: u32,
    until: Instant,
}

#[derive(Clone, Copy)]
enum RequestKind<'a> {
    Profile,
    Discovery,
    Pending(&'a str),
}
impl RequestKind<'_> {
    fn ndjson(self) -> bool {
        !matches!(self, Self::Profile)
    }
}
struct ExportRequest<'a> {
    url: Url,
    kind: RequestKind<'a>,
    username: &'a str,
    contact: &'a str,
    deadline: Instant,
}

pub(in crate::services) struct LichessClient {
    base: Url,
    // Held through response consumption AND retries: profile checks must not
    // overlap exports or bypass a rate-limit wait.
    serial: Mutex<Option<Instant>>,
    retries: Mutex<HashMap<i64, NoProgressRetry>>,
    export_slice: Duration,
    profile_lock_wait: Duration,
    #[cfg(test)]
    backoffs: AtomicU64,
}

impl LichessClient {
    pub fn new() -> Result<Self> {
        let base = std::env::var("PLYBACK_LICHESS_API").unwrap_or_else(|_| BASE.into());
        let base = Url::parse(base.trim_end_matches('/'))?;
        let official = base.as_str() == format!("{BASE}/");
        let fixture = base.scheme() == "http"
            && matches!(base.host_str(), Some("127.0.0.1" | "[::1]" | "localhost"));
        if !(official || fixture)
            || base.path() != "/"
            || !base.username().is_empty()
            || base.password().is_some()
            || base.query().is_some()
            || base.fragment().is_some()
        {
            anyhow::bail!("Lichess API must use https://lichess.org (or HTTP loopback fixtures)");
        }
        Ok(Self {
            base,
            serial: Mutex::new(None),
            retries: Mutex::new(HashMap::new()),
            export_slice: EXPORT_SLICE,
            profile_lock_wait: PROFILE_LOCK_WAIT,
            #[cfg(test)]
            backoffs: AtomicU64::new(0),
        })
    }

    #[cfg(test)]
    pub(in crate::services) fn fixture(base: Url) -> Self {
        Self {
            base,
            serial: Mutex::new(None),
            retries: Mutex::new(HashMap::new()),
            export_slice: EXPORT_SLICE,
            profile_lock_wait: PROFILE_LOCK_WAIT,
            backoffs: AtomicU64::new(0),
        }
    }

    fn retry_blocked(&self, id: i64, scope: &RetryScope) -> bool {
        let mut retries = self.retries.lock().unwrap();
        match retries.get(&id) {
            Some(retry) if retry.scope == *scope => retry.until > Instant::now(),
            Some(_) => {
                retries.remove(&id);
                false
            }
            None => false,
        }
    }

    fn defer_no_progress(&self, id: i64, scope: &RetryScope) -> u32 {
        let mut retries = self.retries.lock().unwrap();
        let retry = retries.entry(id).or_insert_with(|| NoProgressRetry {
            scope: scope.clone(),
            failures: 0,
            until: Instant::now(),
        });
        if retry.scope != *scope {
            retry.scope = scope.clone();
            retry.failures = 0;
        }
        retry.failures = retry.failures.saturating_add(1);
        let seconds = (30u64 << retry.failures.saturating_sub(1).min(5)).min(600);
        retry.until = Instant::now() + Duration::from_secs(seconds);
        retry.failures
    }

    fn clear_retry(&self, id: i64, scope: &RetryScope) {
        let mut retries = self.retries.lock().unwrap();
        if retries.get(&id).is_some_and(|retry| retry.scope == *scope) {
            retries.remove(&id);
        }
    }

    fn cooling_down(&self) -> bool {
        self.serial
            .try_lock()
            .ok()
            .is_some_and(|gate| gate.is_some_and(|until| until > Instant::now()))
    }

    fn endpoint(&self, username: &str, since: Option<i64>) -> Result<Url> {
        if !(2..=30).contains(&username.len())
            || !username
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
        {
            return Err(api_error(400, "Invalid Lichess username"));
        }
        let path = if since.is_some() {
            "api/games/user"
        } else {
            "api/user"
        };
        let mut url = self
            .base
            .join(&format!("{path}/{}", username.to_ascii_lowercase()))?;
        if let Some(since) = since {
            if since > 0 {
                url.query_pairs_mut()
                    .append_pair("since", &since.to_string());
            }
            url.query_pairs_mut().extend_pairs([
                ("pgnInJson", "true"),
                ("clocks", "true"),
                ("opening", "true"),
                ("evals", "false"),
                ("accuracy", "false"),
                ("sort", "dateAsc"),
                ("ongoing", "true"),
            ]);
        }
        Ok(url)
    }

    fn validate(&self, url: &Url, initial: &Url) -> Result<()> {
        if url.origin() != self.base.origin()
            || url.path() != initial.path()
            || url.query() != initial.query()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.fragment().is_some()
            || url.path().contains(['%', '\\'])
        {
            return Err(api_error(
                502,
                "Refused a Lichess URL outside this account and origin",
            ));
        }
        Ok(())
    }

    pub fn profile(&self, username: &str, contact: &str, stop: &AtomicBool) -> Result<Value> {
        self.profile_until(
            username,
            contact,
            stop,
            Instant::now() + Duration::from_secs(30),
        )
        .map_err(|error| {
            if error.is::<RateLimited>() {
                api_error(
                    503,
                    "Lichess is rate limited; its request slot is unavailable, please retry later",
                )
            } else if error.is::<RequestDeadline>() {
                api_error(503, "Lichess profile validation timed out; please retry")
            } else if error.is::<InterruptedStream>() {
                api_error(
                    502,
                    "Lichess profile transport was interrupted; please retry",
                )
            } else {
                error
            }
        })
    }

    fn profile_until(
        &self,
        username: &str,
        contact: &str,
        stop: &AtomicBool,
        deadline: Instant,
    ) -> Result<Value> {
        let url = self.endpoint(username, None)?;
        let mut bytes = Vec::new();
        self.request(
            &url,
            RequestKind::Profile,
            contact,
            stop,
            deadline,
            |chunk| {
                if bytes.len() + chunk.len() > MAX_LINE {
                    return Err(api_error(502, "Lichess profile is too large"));
                }
                bytes.extend_from_slice(chunk);
                Ok(())
            },
        )?;
        let profile: Value = serde_json::from_slice(&bytes)
            .map_err(|_| api_error(502, "Lichess returned invalid profile JSON"))?;
        if !profile["username"]
            .as_str()
            .is_some_and(|name| name.eq_ignore_ascii_case(username))
        {
            return Err(api_error(
                502,
                "Lichess returned an invalid profile identity",
            ));
        }
        Ok(profile)
    }

    #[cfg(test)]
    fn export(
        &self,
        username: &str,
        since: i64,
        contact: &str,
        stop: &AtomicBool,
        cursor: CreationCursor,
    ) -> Result<ExportBatch> {
        self.export_until(
            username,
            since,
            contact,
            stop,
            Instant::now() + self.export_slice,
            cursor,
        )
    }

    fn export_until(
        &self,
        username: &str,
        since: i64,
        contact: &str,
        stop: &AtomicBool,
        deadline: Instant,
        cursor: CreationCursor,
    ) -> Result<ExportBatch> {
        let url = self.endpoint(username, Some(since))?;
        self.collect_export(
            ExportRequest {
                url,
                kind: RequestKind::Discovery,
                username,
                contact,
                deadline,
            },
            stop,
            cursor,
        )
    }

    fn pending(
        &self,
        username: &str,
        ids: &[String],
        contact: &str,
        stop: &AtomicBool,
        deadline: Instant,
    ) -> Result<ExportBatch> {
        if ids.is_empty()
            || ids.len() > PENDING_BATCH_LIMIT
            || ids.iter().any(|id| !valid_game_id(id))
        {
            return Err(api_error(502, "Invalid pending Lichess game IDs"));
        }
        let body = ids.join(",");
        let mut url = self.base.join("api/games/export/_ids")?;
        url.query_pairs_mut().extend_pairs([
            ("pgnInJson", "true"),
            ("clocks", "true"),
            ("opening", "true"),
            ("evals", "false"),
            ("accuracy", "false"),
        ]);
        self.collect_export(
            ExportRequest {
                url,
                kind: RequestKind::Pending(&body),
                username,
                contact,
                deadline,
            },
            stop,
            CreationCursor::default(),
        )
    }

    fn collect_export(
        &self,
        request: ExportRequest<'_>,
        stop: &AtomicBool,
        mut cursor: CreationCursor,
    ) -> Result<ExportBatch> {
        let mut file = tempfile::tempfile()?;
        let mut pending = Vec::new();
        let ordered = matches!(request.kind, RequestKind::Discovery);
        let warning_cursor = ordered.then_some(cursor.last_seen).flatten();
        let mut records = 0;
        let result = self.request(
            &request.url,
            request.kind,
            request.contact,
            stop,
            request.deadline,
            |chunk| {
                for part in chunk.split_inclusive(|c| *c == b'\n') {
                    check_request_deadline(request.deadline, stop)?;
                    if pending.len() + part.len() > MAX_LINE {
                        return Err(api_error(502, "Lichess NDJSON line is too large"));
                    }
                    pending.extend_from_slice(part);
                    if part.last() == Some(&b'\n') {
                        spool_export_line(
                            &pending,
                            &mut file,
                            &mut cursor,
                            request.username,
                            ordered,
                            match request.kind {
                                RequestKind::Pending(ids) => Some(ids),
                                _ => None,
                            },
                            warning_cursor,
                        )?;
                        records += usize::from(!pending.iter().all(u8::is_ascii_whitespace));
                        pending.clear();
                    }
                }
                Ok(())
            },
        );
        let complete = match result {
            Ok(()) => true,
            Err(error) if error.is::<RequestDeadline>() => false,
            Err(error) if error.is::<InterruptedStream>() => false,
            Err(error) => return Err(error),
        };
        // Only a clean EOF may complete an unterminated final NDJSON record.
        if complete {
            spool_export_line(
                &pending,
                &mut file,
                &mut cursor,
                request.username,
                ordered,
                match request.kind {
                    RequestKind::Pending(ids) => Some(ids),
                    _ => None,
                },
                warning_cursor,
            )?;
        }
        file.rewind()?;
        Ok(ExportBatch {
            file,
            cursor,
            complete,
            records,
        })
    }

    fn request(
        &self,
        initial: &Url,
        kind: RequestKind<'_>,
        contact: &str,
        stop: &AtomicBool,
        deadline: Instant,
        mut consume: impl FnMut(&[u8]) -> Result<()>,
    ) -> Result<()> {
        self.validate(initial, initial)?;
        let ndjson = kind.ndjson();
        let lock_deadline = if ndjson {
            deadline
        } else {
            deadline.min(Instant::now() + self.profile_lock_wait)
        };
        let mut gate = loop {
            check_request_deadline(deadline, stop)?;
            if Instant::now() >= lock_deadline {
                return Err(api_error(
                    503,
                    "Lichess is busy syncing games; timed out waiting for its request slot, please retry",
                ));
            }
            if stop.load(Ordering::SeqCst) {
                return Err(api_error(503, "Request cancelled"));
            }
            match self.serial.try_lock() {
                Ok(gate) => break gate,
                Err(std::sync::TryLockError::WouldBlock) => {
                    wait_cancel(Duration::from_millis(25), stop)?
                }
                Err(_) => return Err(api_error(503, "Lichess request lock failed")),
            }
        };
        if let Some(until) = *gate {
            if until > Instant::now() {
                return Err(RateLimited.into());
            }
            wait_request(
                until.saturating_duration_since(Instant::now()),
                deadline,
                stop,
            )?;
        }
        let contact: String = contact
            .chars()
            .filter(|c| c.is_ascii() && !c.is_control())
            .take(120)
            .collect();
        let agent = format!(
            "Plyback/{} (self-hosted game review; +https://github.com/klemen-r/plyback){}",
            crate::APP_VERSION,
            if contact.is_empty() {
                String::new()
            } else {
                format!(" contact: {contact}")
            }
        );
        // One background slice includes lock acquisition, retries and streaming.
        // Each network wait also has an inactivity bound and polls cancellation.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        runtime.block_on(async {
            let client = reqwest::Client::builder()
                .connect_timeout(Duration::from_secs(10))
                .redirect(reqwest::redirect::Policy::none())
                .build()?;
            let mut url = initial.clone();
            let mut redirects = 0;
            for attempt in 0..6 {
                check_request_deadline(deadline, stop)?;
                let mut request = match kind {
                    RequestKind::Pending(body) => client.post(url.clone()).header("Content-Type","text/plain").body(body.to_owned()),
                    _ => client.get(url.clone()),
                };
                request = request
                        .header("User-Agent", &agent)
                        .header(
                            "Accept",
                            if ndjson {
                                "application/x-ndjson"
                            } else {
                                "application/json"
                            },
                        );
                let mut response = network_wait(
                    request.send(),
                    stop,
                    deadline,
                )
                .await?
                .map_err(|_| api_error(502, "Could not reach Lichess"))?;
                let status = response.status().as_u16();
                if matches!(status, 301 | 302 | 303 | 307 | 308) {
                    redirects += 1;
                    if redirects > 5 {
                        return Err(api_error(502, "Too many Lichess redirects"));
                    }
                    let location = response
                        .headers()
                        .get("location")
                        .and_then(|v| v.to_str().ok())
                        .ok_or_else(|| api_error(502, "Invalid Lichess redirect"))?;
                    url = url.join(location)?;
                    self.validate(&url, initial)?;
                    continue;
                }
                if status == 404 {
                    return Err(api_error(
                        404,
                        format!(
                            "No Lichess player named '{}'",
                            initial.path().rsplit('/').next().unwrap_or("")
                        ),
                    ));
                }
                if status == 429 || status >= 500 {
                    let delay = retry_delay(
                        status,
                        response
                            .headers()
                            .get("retry-after")
                            .and_then(|v| v.to_str().ok()),
                        attempt,
                    );
                    *gate = Some(Instant::now() + delay);
                    #[cfg(test)]
                    self.backoffs.fetch_add(1, Ordering::SeqCst);
                    drop(response);
                    // Preserve the cooldown even if this caller is cancelled;
                    // another profile/export cannot bypass a received 429.
                    if status == 429 {
                        return Err(RateLimited.into());
                    }
                    if delay >= deadline.saturating_duration_since(Instant::now()) {
                        return Err(api_error(
                            502,
                            format!("Lichess returned HTTP {status}; retries exceed this request's deadline"),
                        ));
                    }
                    wait_request(delay, deadline, stop)?;
                    if attempt == 5 {
                        return Err(api_error(
                            503,
                            format!("Lichess returned HTTP {status} after 6 attempts"),
                        ));
                    }
                    // Keep the global request gate while waiting.
                    continue;
                }
                if status != 200 {
                    return Err(api_error(502, format!("Lichess returned HTTP {status}")));
                }
                loop {
                    let chunk = match network_wait(response.chunk(), stop, deadline).await {
                        Ok(Ok(chunk)) => chunk,
                        Ok(Err(_)) => return Err(InterruptedStream.into()),
                        Err(error) if error.is::<RequestDeadline>() || stop.load(Ordering::SeqCst) => return Err(error),
                        Err(_) => return Err(InterruptedStream.into()),
                    };
                    let Some(chunk) = chunk else { break; };
                    check_request_deadline(deadline, stop)?;
                    consume(&chunk)?;
                }
                if stop.load(Ordering::SeqCst) {
                    return Err(api_error(503, "Request cancelled"));
                }
                return Ok(());
            }
            Err(api_error(502, "Lichess request did not complete"))
        })
    }
}

#[derive(Debug)]
struct RateLimited;
impl std::fmt::Display for RateLimited {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Lichess is rate limited; retry after its shared cooldown")
    }
}
impl std::error::Error for RateLimited {}

#[derive(Debug)]
struct InterruptedStream;
impl std::fmt::Display for InterruptedStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Lichess export transport was interrupted")
    }
}
impl std::error::Error for InterruptedStream {}

#[derive(Debug)]
struct RequestDeadline;
impl std::fmt::Display for RequestDeadline {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Lichess export time slice finished")
    }
}
impl std::error::Error for RequestDeadline {}

fn check_request_deadline(deadline: Instant, stop: &AtomicBool) -> Result<()> {
    if stop.load(Ordering::SeqCst) {
        return Err(api_error(503, "Request cancelled"));
    }
    if Instant::now() >= deadline {
        return Err(RequestDeadline.into());
    }
    Ok(())
}
fn wait_request(delay: Duration, deadline: Instant, stop: &AtomicBool) -> Result<()> {
    check_request_deadline(deadline, stop)?;
    wait_cancel(
        delay.min(deadline.saturating_duration_since(Instant::now())),
        stop,
    )?;
    check_request_deadline(deadline, stop)
}
async fn network_wait<F: std::future::Future>(
    future: F,
    stop: &AtomicBool,
    deadline: Instant,
) -> Result<F::Output> {
    check_request_deadline(deadline, stop)?;
    let limit = Duration::from_secs(30).min(deadline.saturating_duration_since(Instant::now()));
    let bounded = tokio::time::timeout(limit, future);
    tokio::pin!(bounded);
    loop {
        check_request_deadline(deadline, stop)?;
        tokio::select! {
            result = &mut bounded => return match result {
                Ok(output) => Ok(output),
                Err(_) if Instant::now() >= deadline => Err(RequestDeadline.into()),
                Err(_) => Err(api_error(502, "Lichess network wait timed out")),
            },
            _ = tokio::time::sleep(Duration::from_millis(20)) => {}
        }
    }
}

fn retry_delay(status: u16, retry_after: Option<&str>, attempt: u32) -> Duration {
    let header = retry_after
        .and_then(|s| {
            s.parse::<u64>().ok().or_else(|| {
                chrono::DateTime::parse_from_rfc2822(s)
                    .ok()
                    .map(|d| (d.timestamp() - now()).max(0) as u64)
            })
        })
        .unwrap_or(0);
    Duration::from_secs(
        header
            .max(if status == 429 {
                60
            } else {
                2u64.pow(attempt + 1)
            })
            .min(7 * 86400),
    )
}

struct ExportBatch {
    file: std::fs::File,
    cursor: CreationCursor,
    complete: bool,
    records: usize,
}

#[derive(Default)]
struct CreationCursor {
    last_seen: Option<i64>,
}
impl CreationCursor {
    fn next(&self) -> Option<i64> {
        self.last_seen
    }
    fn observe(&mut self, game: &Value) -> Result<()> {
        let created = game["createdAt"]
            .as_i64()
            .filter(|ms| (0..=253402300799000).contains(ms))
            .ok_or_else(|| api_error(502, "Invalid Lichess creation timestamp"))?;
        if self.last_seen.is_some_and(|previous| previous > created) {
            return Err(api_error(
                502,
                "Lichess export is not ordered by creation time",
            ));
        }
        self.last_seen = Some(created);
        Ok(())
    }
}

fn spool_export_line(
    bytes: &[u8],
    file: &mut std::fs::File,
    cursor: &mut CreationCursor,
    username: &str,
    ordered: bool,
    requested_ids: Option<&str>,
    warning_cursor: Option<i64>,
) -> Result<()> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(());
    }
    let game: Value = serde_json::from_slice(bytes)
        .map_err(|_| api_error(502, "Lichess returned invalid NDJSON"))?;
    if !game.is_object() {
        return Err(api_error(502, "Invalid Lichess game object"));
    }
    let belongs = ["white", "black"].iter().any(|color| {
        let player = &game["players"][*color];
        player["user"]["name"]
            .as_str()
            .or_else(|| player["user"]["id"].as_str())
            .or_else(|| player["name"].as_str())
            .is_some_and(|name| name.eq_ignore_ascii_case(username))
    });
    if !belongs
        || requested_ids
            .is_some_and(|ids| !ids.split(',').any(|id| game["id"].as_str() == Some(id)))
    {
        return Ok(());
    }
    // Mapping must not hold the creation checkpoint behind one bad game.
    if ordered {
        cursor.observe(&game)?;
    }
    if let Some(id) = game["id"].as_str().filter(|id| valid_game_id(id)) {
        let ongoing = matches!(game["status"].as_str(), Some("created" | "started"));
        let supported = matches!(
            game["variant"].as_str(),
            Some("standard" | "fromPosition" | "chess960")
        ) && game["rules"].as_str().is_none_or(|rules| rules == "chess");
        if !ongoing || supported {
            let created = game["createdAt"]
                .as_i64()
                .filter(|ms| (0..=253402300799000).contains(ms))
                .ok_or_else(|| api_error(502, "Invalid pending Lichess creation timestamp"))?;
            serde_json::to_writer(
                &mut *file,
                &json!({"_sync_pending":{"id":id,"created":created,"ongoing":ongoing}}),
            )?;
            file.write_all(b"\n")?;
        }
    }
    let row = match game_row(&game) {
        Ok(row) => row,
        // Discovery is inclusive. A boundary game already had its warning in
        // the slice that advanced the persisted cursor, including after restart.
        Err(_) if warning_cursor.is_some() && warning_cursor == game["createdAt"].as_i64() => None,
        Err(error) => {
            let id = game["id"]
                .as_str()
                .unwrap_or("unknown")
                .chars()
                .filter(|c| !c.is_control())
                .take(64)
                .collect::<String>();
            Some(
                json!({"_sync_warning":format!("Skipped Lichess game '{id}': {error}"),"_sync_warning_game":{"id":game["id"],"created":game["createdAt"]}}),
            )
        }
    };
    if let Some(row) = row {
        serde_json::to_writer(&mut *file, &row)?;
        file.write_all(b"\n")?;
    }
    Ok(())
}

fn valid_game_id(id: &str) -> bool {
    (1..=32).contains(&id.len()) && id.bytes().all(|c| c.is_ascii_alphanumeric())
}

#[cfg(test)]
fn spool_line(bytes: &[u8], file: &mut std::fs::File) -> Result<()> {
    if bytes.iter().all(u8::is_ascii_whitespace) {
        return Ok(());
    }
    let game: Value = serde_json::from_slice(bytes)
        .map_err(|_| api_error(502, "Lichess returned invalid NDJSON"))?;
    if let Some(row) = game_row(&game)? {
        serde_json::to_writer(&mut *file, &row)?;
        file.write_all(b"\n")?;
    }
    Ok(())
}

impl Services {
    pub(super) fn sync_lichess_account(&self, account: &Value) -> Result<()> {
        let claim = AccountSync::from_row(account)?;
        // A previous slice may have stored games but failed to hand them to the queue.
        // Recover them even when HTTP work is deferred by a cooldown or backoff.
        self.enqueue_lichess_pending(&claim)?;
        let retry_scope = RetryScope::from_account(account);
        // A cooldown is shared by profiles and exports. Do not claim a fresh
        // sync, refresh a timestamp, or invalidate insights while no HTTP work
        // can be done. Retain a manual request without waking the loop.
        if self.lichess_client.cooling_down()
            || self.lichess_client.retry_blocked(claim.id, &retry_scope)
        {
            self.requested.lock().unwrap().insert(claim.id);
            return Ok(());
        }
        let username = account["username"].as_str().unwrap_or("");
        let initial = account["history_complete"] != 1;
        let settings = self.sync_settings()?;
        let contact = settings["contact"].as_str().unwrap_or("");
        let deadline = Instant::now() + self.lichess_client.export_slice;
        let profile = if now() - account["profile_fetched_at"].as_i64().unwrap_or(0) > 21600 {
            match self
                .lichess_client
                .profile_until(username, contact, &self.stop, deadline)
            {
                Ok(profile) => Some(profile),
                Err(error) if error.is::<RateLimited>() => {
                    self.requested.lock().unwrap().insert(claim.id);
                    return Ok(());
                }
                Err(error) if error.is::<RequestDeadline>() => {
                    self.defer_lichess_no_progress(&claim, &retry_scope)?;
                    return Ok(());
                }
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        let pending_ids = self.sync_rows(
            "SELECT game_id FROM lichess_pending_games WHERE account_id=? AND checked_at=(SELECT MIN(checked_at) FROM lichess_pending_games WHERE account_id=?) AND checked_at<=? ORDER BY game_id LIMIT ?",
            &[json!(claim.id), json!(claim.id), json!(now()-PENDING_POLL_SECONDS), json!(PENDING_BATCH_LIMIT)],
        )?.into_iter().filter_map(|row|row["game_id"].as_str().map(str::to_owned)).collect::<Vec<_>>();
        let mut batches = Vec::new();
        if !pending_ids.is_empty() {
            // Reserve half the remaining slice for discovery even when a
            // correspondence export is slow. ID responses are not date-ordered.
            let pending_deadline =
                Instant::now() + deadline.saturating_duration_since(Instant::now()) / 2;
            match self.lichess_client.pending(
                username,
                &pending_ids,
                contact,
                &self.stop,
                pending_deadline,
            ) {
                Ok(batch) => batches.push((true, batch)),
                Err(error) if error.is::<RateLimited>() => {
                    self.requested.lock().unwrap().insert(claim.id);
                    return Ok(());
                }
                Err(error) => return Err(error),
            }
        }
        let since = account["lichess_cursor_ms"].as_i64().unwrap_or_else(|| {
            account["import_since"]
                .as_str()
                .and_then(|period| {
                    chrono::NaiveDate::parse_from_str(&format!("{period}/01"), "%Y/%m/%d").ok()
                })
                .and_then(|date| date.and_hms_opt(0, 0, 0))
                .map(|date| date.and_utc().timestamp_millis())
                .unwrap_or(0)
        });
        let mut rate_limited = false;
        // A failed discovery export must not throw away the finished games already
        // downloaded by ID: import them, then report the failure.
        let mut export_error = None;
        match self.lichess_client.export_until(
            username,
            since,
            contact,
            &self.stop,
            deadline,
            CreationCursor {
                last_seen: (since > 0).then_some(since),
            },
        ) {
            Ok(batch) => batches.push((false, batch)),
            Err(error) if error.is::<RateLimited>() => {
                rate_limited = true;
            }
            Err(error) if batches.is_empty() => return Err(error),
            Err(error) => export_error = Some(error),
        }
        let discovery = batches
            .iter()
            .find(|(by_ids, _)| !*by_ids)
            .map(|(_, batch)| batch);
        let next_cursor = discovery.and_then(|batch| batch.cursor.next());
        let history_complete = !initial || discovery.is_some_and(|batch| batch.complete);
        let incomplete = batches.iter().any(|(_, batch)| !batch.complete);
        if rate_limited {
            self.requested.lock().unwrap().insert(claim.id);
        }
        if !batches
            .iter()
            .any(|(_, batch)| batch.complete || batch.records > 0)
        {
            if !rate_limited {
                self.defer_lichess_no_progress(&claim, &retry_scope)?;
            }
            return Ok(());
        }
        // Games newly linked to this account, and those of them that are new rather than
        // history: during an initial import a game is history unless it ended after the
        // previous sync (the same rule as the Chess.com sync).
        let mut linked = Vec::new();
        let mut fresh = Vec::new();
        let last_sync = account["last_sync_at"].as_i64();
        let mut progress = false;
        let Some(true) = claim.transaction(self, |tx| {
            linked.clear();
            fresh.clear();
            progress = next_cursor.is_some() && next_cursor != account["lichess_cursor_ms"].as_i64();
            // A range reset during the download invalidates both the discovery
            // checkpoint and pending-ID results from the old account scope.
            let still_current: bool = tx.query_row(
                "SELECT import_since IS ? AND lichess_cursor_ms IS ? FROM accounts WHERE id=?",
                rusqlite::params![account["import_since"].as_str(),account["lichess_cursor_ms"].as_i64(),claim.id],
                |row|row.get(0),
            )?;
            if !still_current { return Ok(false); }
            let (analyze_history, auto_analyze) = tx.query_row(
                "SELECT analyze_history,auto_analyze FROM accounts WHERE id=?",
                [claim.id],
                |row| Ok((row.get::<_, bool>(0)?, row.get::<_, bool>(1)?)),
            )?;
            for (by_ids,batch) in &batches {
                let mut observed = HashSet::new();
                let mut reader = BufReader::new(batch.file.try_clone()?);
                reader.rewind()?;
                for line in reader.lines() {
                    if self.stop.load(Ordering::SeqCst) { return Err(api_error(503,"Sync cancelled")); }
                    let row:Value = serde_json::from_str(&line?)?;
                    if let Some(pending) = row.get("_sync_pending") {
                        let game_id = pending["id"].as_str().unwrap();
                        observed.insert(game_id.to_owned());
                        let changed = if pending["ongoing"] == true {
                            if *by_ids {
                                tx.execute(
                                    "INSERT INTO lichess_pending_games(account_id,game_id,created_at_ms,checked_at) VALUES(?,?,?,?) ON CONFLICT(account_id,game_id) DO UPDATE SET checked_at=excluded.checked_at,misses=0 WHERE lichess_pending_games.checked_at IS NOT excluded.checked_at OR lichess_pending_games.misses<>0",
                                    rusqlite::params![claim.id,game_id,pending["created"].as_i64(),now()],
                                )?
                            } else {
                                tx.execute("INSERT OR IGNORE INTO lichess_pending_games(account_id,game_id,created_at_ms) VALUES(?,?,?)",rusqlite::params![claim.id,game_id,pending["created"].as_i64()])?
                            }
                        } else {
                            tx.execute("DELETE FROM lichess_pending_games WHERE account_id=? AND game_id=?",rusqlite::params![claim.id,game_id])?
                        };
                        progress |= changed>0;
                        continue;
                    }
                    if let Some(warning) = row["_sync_warning"].as_str() {
                        if rate_limited {
                            // Keep an unmappable pending ID until its diagnostic
                            // can be published on a non-rate-limited pass.
                            let game=&row["_sync_warning_game"];
                            progress |= tx.execute("INSERT INTO lichess_pending_games(account_id,game_id,created_at_ms,checked_at) VALUES(?,?,?,?) ON CONFLICT(account_id,game_id) DO UPDATE SET checked_at=excluded.checked_at,misses=0 WHERE lichess_pending_games.checked_at IS NOT excluded.checked_at OR lichess_pending_games.misses<>0",rusqlite::params![claim.id,game["id"].as_str(),game["created"].as_i64(),now()])?>0;
                        } else { claim.event(tx,"warning",warning)?; }
                        continue;
                    }
                    let rows = [row];
                    let inserted = store::upsert_games(tx,&rows)?;
                    let newly = store::link_games(tx,claim.id,username,&rows,&inserted)?;
                    for (((id,is_new),new),row) in inserted.iter().zip(newly).zip(&rows) {
                        if !new { continue; }
                        linked.push(*id);
                        let ended = row["end_time"].as_i64().unwrap_or(0);
                        let is_fresh = !(initial && last_sync.is_none_or(|t| ended <= t));
                        if is_fresh { fresh.push(*id); }
                        let (priority, wanted) = if is_fresh {
                            (PRIORITY_NEW, auto_analyze)
                        } else {
                            (PRIORITY_HISTORY, analyze_history)
                        };
                        if wanted {
                            // Linking a user import promotes metadata only. Its review (or
                            // deliberate lack of one) remains exactly as the user left it.
                            if !is_new && tx.query_row("SELECT source='pgn' FROM games WHERE id=?",[id],|r|r.get::<_,bool>(0))? {
                                continue;
                            }
                            // The marker and game commit together; enqueue_for_account
                            // consumes it atomically with the job, as for Chess.com.
                            tx.execute(
                                "INSERT OR REPLACE INTO sync_pending(account_id,game_id,priority) VALUES(?,?,?)",
                                rusqlite::params![claim.id,id,priority],
                            )?;
                        }
                    }
                }
                if *by_ids && batch.complete {
                    // Only a clean, complete response can establish that a
                    // requested ID was omitted. Interrupted tails are not misses.
                    for id in &pending_ids {
                        if !observed.contains(id) {
                            progress |= tx.execute("UPDATE lichess_pending_games SET checked_at=?,misses=misses+1 WHERE account_id=? AND game_id=?",rusqlite::params![now(),claim.id,id])?>0;
                        }
                    }
                }
            }
            // Lichess stopped returning these games (deleted, hidden or filtered). Say so
            // before dropping them: the creation cursor has already moved past them.
            let dropped: Vec<String> = {
                let mut statement = tx.prepare("SELECT game_id FROM lichess_pending_games WHERE account_id=? AND misses>=? ORDER BY game_id")?;
                let rows = statement.query_map(rusqlite::params![claim.id,PENDING_MISS_LIMIT],|row|row.get(0))?;
                rows.collect::<rusqlite::Result<_>>()?
            };
            if !dropped.is_empty() {
                tx.execute("DELETE FROM lichess_pending_games WHERE account_id=? AND misses>=?",rusqlite::params![claim.id,PENDING_MISS_LIMIT])?;
                let shown = dropped.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
                let more = dropped.len().saturating_sub(5);
                claim.event(tx,"warning",&format!("Stopped waiting for {} unfinished Lichess game(s) that Lichess no longer returns: {shown}{}",dropped.len(),if more>0 {format!(" and {more} more")} else {String::new()}))?;
            }
            // Partial exports that made no committed progress must not fabricate
            // a successful sync or invalidate insights every retry. A 429 after
            // pending results likewise must not change any account metadata.
            if !rate_limited && (!incomplete || progress) {
            if let Some(profile) = &profile {
                tx.execute("UPDATE accounts SET display_name=?,profile_json=?,stats_json=?,profile_fetched_at=? WHERE id=?",rusqlite::params![profile["username"].as_str().unwrap_or(username),profile.to_string(),json!({"perfs":profile["perfs"]}).to_string(),now(),claim.id])?;
            }
            tx.execute("UPDATE accounts SET last_sync_at=?,sync_status='idle',sync_error=NULL,history_complete=?,lichess_cursor_ms=COALESCE(?,lichess_cursor_ms),lichess_scan_ms=NULL,lichess_pending_ms=NULL,lichess_finished_ms=NULL,last_new_game_at=CASE WHEN ? THEN ? ELSE last_new_game_at END WHERE id=? AND (last_sync_at IS NOT ? OR sync_status IS NOT 'idle' OR sync_error IS NOT NULL OR history_complete IS NOT ? OR (? IS NOT NULL AND lichess_cursor_ms IS NOT ?) OR ?)",rusqlite::params![now(),history_complete,next_cursor,!fresh.is_empty(),now(),claim.id,now(),history_complete,next_cursor,next_cursor,!linked.is_empty()])?;
            }
            if !rate_limited && !linked.is_empty() { claim.event(tx,"info",&format!("Imported {} game(s)",linked.len()))?; }
            Ok(true)
        })? else {return Ok(());};
        // Ask for the next slice before queueing this one's reviews: if the database is busy
        // while queueing, the import still continues, and the next slice queues what's owed.
        if export_error.is_none() && incomplete && !rate_limited && progress {
            self.lichess_client.clear_retry(claim.id, &retry_scope);
            self.request_sync(claim.id);
        }
        self.enqueue_lichess_pending(&claim)?;
        if let Some(error) = export_error {
            return Err(error);
        }
        if incomplete && !rate_limited {
            if !progress {
                self.defer_lichess_no_progress(&claim, &retry_scope)?;
            }
        } else if !batches.is_empty() {
            self.lichess_client.clear_retry(claim.id, &retry_scope);
        }
        Ok(())
    }

    fn enqueue_lichess_pending(&self, claim: &AccountSync) -> Result<()> {
        // Only jobs explicitly owed by a sync are recovered: a cleared queue and a
        // PGN imported without analysis stay unqueued. The shared queue code checks
        // the current generation/preferences and claims each marker with the job.
        let pending = self.sync_rows(
            "SELECT p.game_id,p.priority FROM sync_pending p JOIN accounts a ON a.id=p.account_id WHERE p.account_id=? AND a.sync_generation=?",
            &[json!(claim.id), json!(claim.generation)],
        )?;
        for priority in [PRIORITY_NEW, PRIORITY_HISTORY] {
            let ids = pending
                .iter()
                .filter(|row| row["priority"] == priority)
                .filter_map(|row| row["game_id"].as_i64())
                .collect::<Vec<_>>();
            if !ids.is_empty() {
                self.enqueue_for_account(claim.id, &claim.generation, &ids, priority)?;
            }
        }
        Ok(())
    }

    fn defer_lichess_no_progress(&self, claim: &AccountSync, scope: &RetryScope) -> Result<()> {
        let failures = self.lichess_client.defer_no_progress(claim.id, scope);
        // Preserve manual requests too, but do not notify the loop or bypass the
        // retry deadline. Its regular tick checks the in-memory deadline.
        self.requested.lock().unwrap().insert(claim.id);
        if failures == NO_PROGRESS_ERROR_THRESHOLD {
            claim.transaction(self, |tx| {
                let current: bool = tx.query_row(
                    "SELECT import_since IS ? AND lichess_cursor_ms IS ? FROM accounts WHERE id=?",
                    rusqlite::params![scope.import_since,scope.cursor,claim.id], |row|row.get(0),
                )?;
                if current {
                    let message = "Lichess export repeatedly made no progress; retrying with exponential backoff (up to 10 minutes)";
                    tx.execute("UPDATE accounts SET sync_status='error',sync_error=? WHERE id=?",rusqlite::params![message,claim.id])?;
                    claim.event(tx,"warning",message)?;
                }
                Ok(())
            })?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    fn fixture_game(id: &str, end: i64) -> Value {
        json!({"id":id,"variant":"standard","speed":"blitz","rated":true,"createdAt":end-60000,"lastMoveAt":end,"status":"mate","winner":"white","players":{"white":{"user":{"name":"fixture"},"rating":1500,"analysis":{"accuracy":99}},"black":{"user":{"name":"opponent"},"rating":1450}},"clock":{"initial":300,"increment":2},"clocks":[29975,29800,30100,29750],"analysis":[{"eval":999}],"pgn":"[White \"fixture\"]\n[Black \"opponent\"]\n[Result \"1-0\"]\n[WhiteAccuracy \"99\"]\n\n1. e4 { [%eval 9.99] } e5 2. Nf3 Nc6 1-0"})
    }

    type Response = (u16, String, String, bool);
    struct FixtureServer {
        base: Url,
        requests: Arc<Mutex<Vec<String>>>,
        stop: Arc<AtomicBool>,
        handle: Option<std::thread::JoinHandle<()>>,
    }
    impl FixtureServer {
        fn start(handler: impl Fn(&str) -> Response + Send + 'static) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
            listener.set_nonblocking(true).unwrap();
            let requests = Arc::new(Mutex::new(Vec::new()));
            let stop = Arc::new(AtomicBool::new(false));
            let (thread_requests, thread_stop) = (requests.clone(), stop.clone());
            let handle = std::thread::spawn(move || {
                while !thread_stop.load(Ordering::SeqCst) {
                    let (mut socket, _) = match listener.accept() {
                        Ok(pair) => pair,
                        Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                            continue;
                        }
                        Err(e) => panic!("fixture accept: {e}"),
                    };
                    socket.set_nonblocking(false).unwrap();
                    socket
                        .set_read_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    socket
                        .set_write_timeout(Some(Duration::from_secs(2)))
                        .unwrap();
                    let mut reader = BufReader::new(socket.try_clone().unwrap());
                    let mut request = String::new();
                    loop {
                        let mut line = String::new();
                        if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                            break;
                        }
                        request.push_str(&line);
                    }
                    let content_length = request
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                    assert!(content_length <= 16384);
                    let mut body = vec![0; content_length];
                    std::io::Read::read_exact(&mut reader, &mut body).unwrap();
                    request.push_str("\r\n");
                    request.push_str(std::str::from_utf8(&body).unwrap());
                    thread_requests.lock().unwrap().push(request.clone());
                    let (status, headers, body, stall) = handler(&request);
                    let response = format!(
                        "HTTP/1.1 {status} Fixture\r\nContent-Length: {}\r\nConnection: close\r\n{headers}\r\n",
                        body.len() + usize::from(stall || headers.contains("X-Fixture-Truncate"))
                    );
                    if socket.write_all(response.as_bytes()).is_err() {
                        continue;
                    }
                    // Small chunks exercise delimiters split across network reads.
                    for chunk in body.as_bytes().chunks(17) {
                        if socket.write_all(chunk).is_err() {
                            break;
                        }
                    }
                    if stall {
                        socket.set_nonblocking(true).unwrap();
                        while !thread_stop.load(Ordering::SeqCst) {
                            match socket.peek(&mut [0u8; 1]) {
                                Ok(0) => break,
                                Err(error) if error.kind() != std::io::ErrorKind::WouldBlock => {
                                    break;
                                }
                                _ => std::thread::sleep(Duration::from_millis(5)),
                            }
                        }
                    }
                }
            });
            Self {
                base,
                requests,
                stop,
                handle: Some(handle),
            }
        }
        fn wait_requests(&self, count: usize) {
            let until = Instant::now() + Duration::from_secs(3);
            while self.requests.lock().unwrap().len() < count {
                assert!(Instant::now() < until, "fixture request not received");
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
    impl Drop for FixtureServer {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
            if let Some(handle) = self.handle.take() {
                handle.join().unwrap();
            }
        }
    }

    #[test]
    fn lichess_mapping_preserves_clocks_and_uses_only_our_analysis() {
        let row = game_row(&fixture_game("AbCd1234", 1770000000123))
            .unwrap()
            .unwrap();
        assert_eq!(row["uuid"], "lichess:AbCd1234");
        assert_eq!(row["url"], "https://lichess.org/AbCd1234");
        assert_eq!(row["white_rating"], 1500);
        assert_eq!(row["black_result"], "checkmated");
        assert_eq!(row["result"], "1-0");
        assert_eq!(row["termination"], "mate");
        assert_eq!(row["rated"], 1);
        assert_eq!(row["end_time"], 1770000000);
        assert_eq!(row["time_control"], "300+2");
        assert!(row["chesscom_accuracy_white"].is_null());
        let pgn = row["pgn"].as_str().unwrap();
        assert!(!pgn.contains("eval"));
        assert!(!pgn.contains("Accuracy"));
        let parsed = crate::pgn::parse(pgn).unwrap();
        assert_eq!(parsed.moves[0].clock, Some(299.75));
        assert_eq!(parsed.moves[3].clock, Some(297.5));
        assert_eq!(parsed.headers["TimeControl"], "300+2");
    }

    #[test]
    fn lichess_speeds_variants_and_statuses_are_mapped() {
        for (speed, time_class) in [
            ("ultraBullet", "bullet"),
            ("bullet", "bullet"),
            ("blitz", "blitz"),
            ("rapid", "rapid"),
            ("classical", "rapid"),
            ("correspondence", "daily"),
        ] {
            let mut game = fixture_game("speed", 1770000000000);
            game["speed"] = json!(speed);
            assert_eq!(game_row(&game).unwrap().unwrap()["time_class"], time_class);
        }
        for (status, result) in [
            ("outoftime", "timeout"),
            ("resign", "resigned"),
            ("mate", "checkmated"),
        ] {
            let mut game = fixture_game("status", 1770000000000);
            game["status"] = json!(status);
            assert_eq!(game_row(&game).unwrap().unwrap()["black_result"], result);
        }
        for variant in ["atomic", "antichess", "crazyhouse"] {
            let mut game = fixture_game("variant", 1770000000000);
            game["variant"] = json!(variant);
            assert!(game_row(&game).unwrap().is_none());
        }
        // Chess960 is imported and reviewed, as from Chess.com.
        {
            let mut game = fixture_game("c960", 1770000000000);
            game["variant"] = json!("chess960");
            game["pgn"] = json!(
                "[White \"fixture\"]
[Black \"opponent\"]
[Result \"1-0\"]
[Variant \"Chess960\"]
[FEN \"rkrbbqnn/pppppppp/8/8/8/8/PPPPPPPP/RKRBBQNN w CAca - 0 1\"]
[SetUp \"1\"]

1. e4 e5 2. Ng3 Ng6 1-0"
            );
            let row = game_row(&game).unwrap().expect("Chess960 game imported");
            assert_eq!(row["rules"], "chess960");
        }
        let mut game = fixture_game("position", 1770000000000);
        game["variant"] = json!("fromPosition");
        assert!(game_row(&game).unwrap().is_some());
        game["rules"] = json!("atomic");
        assert!(game_row(&game).unwrap().is_none());
        game["rules"] = json!("chess");
        game["status"] = json!("started");
        assert!(game_row(&game).unwrap().is_none());
    }

    #[test]
    fn lichess_from_position_keeps_fen_move_numbers_and_correspondence_clock() {
        let mut game = fixture_game("position", 1770000000000);
        game["variant"] = json!("fromPosition");
        game["speed"] = json!("correspondence");
        game["clock"] = Value::Null;
        game["daysPerTurn"] = json!(3);
        game["pgn"] = json!(
            "[Variant \"From Position\"]\n[SetUp \"1\"]\n[FEN \"rnbqkbnr/pppp1ppp/8/4p3/4P3/5N2/PPPP1PPP/RNBQKB1R b KQkq - 1 7\"]\n[Result \"1/2-1/2\"]\n\n7... Nc6 8. Bc4 Bc5 1/2-1/2"
        );
        game["winner"] = Value::Null;
        game["status"] = json!("draw");
        let row = game_row(&game).unwrap().unwrap();
        let pgn = row["pgn"].as_str().unwrap();
        assert!(pgn.contains("7... Nc6"));
        assert!(pgn.contains("8. Bc4"));
        assert_eq!(row["time_control"], "1/259200");
        assert_eq!(row["result"], "1/2-1/2");
        assert_eq!(row["white_result"], "agreed");
        let parsed = crate::pgn::parse(pgn).unwrap();
        assert_eq!(parsed.moves.len(), 3);
        assert_eq!(parsed.moves[0].color, "b");
        assert_eq!(parsed.headers["TimeControl"], "1/259200");
    }

    #[test]
    fn lichess_urls_must_stay_within_account_query_and_origin() {
        let client = LichessClient::fixture(Url::parse(BASE).unwrap());
        let initial = client.endpoint("Fixture", Some(1234)).unwrap();
        let full_history = client.endpoint("Fixture", Some(0)).unwrap();
        assert_eq!(full_history.path(), "/api/games/user/fixture");
        assert!(!full_history.query_pairs().any(|(key, _)| key == "since"));
        assert!(
            full_history
                .query_pairs()
                .any(|(key, value)| key == "pgnInJson" && value == "true")
        );
        assert!(client.validate(&initial, &initial).is_ok());
        for raw in [
            "https://evil.test/api/games/user/fixture",
            "https://lichess.org.evil.test/api/games/user/fixture",
            "https://lichess.org/api/games/user/other",
            "https://lichess.org/api/user/fixture",
            "https://user@lichess.org/api/games/user/fixture",
            "https://lichess.org/api/games/user/%66ixture",
            "http://lichess.org/api/games/user/fixture",
        ] {
            let mut url = Url::parse(raw).unwrap();
            url.set_query(initial.query());
            assert!(client.validate(&url, &initial).is_err(), "{raw}");
        }
        let mut altered = initial.clone();
        altered.set_query(Some("since=999"));
        assert!(client.validate(&altered, &initial).is_err());
        for name in [
            "a",
            "../fixture",
            "fixture%2fother",
            "two words",
            "fixture?since=0",
        ] {
            assert!(client.endpoint(name, Some(0)).is_err());
        }
    }

    #[test]
    fn lichess_ndjson_accepts_blank_and_final_lines_and_rejects_invalid_json() {
        let mut file = tempfile::tempfile().unwrap();
        spool_line(b"\r\n", &mut file).unwrap();
        spool_line(
            fixture_game("ndjson", 1770000000000).to_string().as_bytes(),
            &mut file,
        )
        .unwrap();
        file.rewind().unwrap();
        assert_eq!(BufReader::new(&mut file).lines().count(), 1);
        assert!(spool_line(b"{broken}", &mut file).is_err());
        assert!(spool_line(b"null", &mut file).is_err());
    }

    #[test]
    fn lichess_fixture_sync_is_incremental_and_queues_existing_engine() {
        let extra = Arc::new(AtomicBool::new(false));
        let server_extra = extra.clone();
        let reload = Arc::new(AtomicBool::new(false));
        let server_reload = reload.clone();
        let server = FixtureServer::start(move |request| {
            if request.starts_with("GET /api/user/") {
                return (
                    200,
                    String::new(),
                    json!({"username":"fixture","perfs":{"blitz":{"rating":1500}}}).to_string(),
                    false,
                );
            }
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("accept: application/x-ndjson")
            );
            assert!(request.contains("pgnInJson=true"));
            assert!(request.contains("evals=false"));
            assert!(request.contains("Plyback/"));
            if server_reload.load(Ordering::SeqCst) {
                return (
                    200,
                    String::new(),
                    format!(
                        "{}\n{}\n",
                        fixture_game("older", 1760000000123),
                        fixture_game("second", 1770000060456)
                    ),
                    false,
                );
            }
            let game = if server_extra.load(Ordering::SeqCst) {
                fixture_game("second", 1770000060456)
            } else {
                fixture_game("first", 1770000000123)
            };
            (200, String::new(), format!("\n{}\n\r\n", game), false)
        });
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute("INSERT INTO accounts(username,platform,added_at,analyze_history) VALUES('fixture','lichess',0,0)",&[]).unwrap();
        let mut service = Services::new(db.clone()).unwrap();
        Arc::get_mut(&mut service).unwrap().lichess_client =
            LichessClient::fixture(server.base.clone());
        service
            .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
            .unwrap();
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM games WHERE source='lichess'", &[])
                .unwrap(),
            1
        );
        assert_eq!(db.scalar("SELECT COUNT(*) FROM jobs", &[]).unwrap(), 0);
        assert_eq!(
            db.scalar("SELECT history_complete FROM accounts", &[])
                .unwrap(),
            1
        );
        extra.store(true, Ordering::SeqCst);
        service
            .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
            .unwrap();
        let requests = server.requests.lock().unwrap();
        assert!(requests.last().unwrap().contains("since=1769999940123"));
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 2);
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM account_games", &[])
                .unwrap(),
            2
        );
        assert_eq!(
            db.scalar("SELECT priority FROM jobs", &[]).unwrap(),
            PRIORITY_NEW
        );
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM game_analysis", &[])
                .unwrap(),
            0
        );
        assert!(
            db.scalar("SELECT chesscom_accuracy_white FROM games LIMIT 1", &[])
                .unwrap()
                .is_null()
        );
        drop(requests);
        service
            .handle(
                "PATCH",
                "/api/accounts/1",
                &Query::new(),
                &json!({"import_since":null}),
            )
            .unwrap()
            .unwrap();
        reload.store(true, Ordering::SeqCst);
        service
            .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
            .unwrap();
        assert!(
            !server
                .requests
                .lock()
                .unwrap()
                .last()
                .unwrap()
                .contains("since=")
        );
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 3);
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM account_games", &[])
                .unwrap(),
            3
        );
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM games WHERE uuid='lichess:older'", &[])
                .unwrap(),
            1
        );
    }

    #[test]
    fn lichess_games_new_to_the_account_are_new() {
        let extra = Arc::new(AtomicBool::new(false));
        let server_extra = extra.clone();
        let server = FixtureServer::start(move |request| {
            if request.starts_with("GET /api/user/") {
                return (
                    200,
                    String::new(),
                    json!({"username":"fixture","perfs":{}}).to_string(),
                    false,
                );
            }
            let game = if server_extra.load(Ordering::SeqCst) {
                fixture_game("second", 1770000060456)
            } else {
                fixture_game("first", 1770000000123)
            };
            (
                200,
                String::new(),
                format!(
                    "{game}
"
                ),
                false,
            )
        });
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute("INSERT INTO accounts(username,platform,added_at,analyze_history,auto_analyze) VALUES('fixture','lichess',0,0,1)",&[]).unwrap();
        let mut service = Services::new(db.clone()).unwrap();
        Arc::get_mut(&mut service).unwrap().lichess_client =
            LichessClient::fixture(server.base.clone());
        let sync = || {
            service
                .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
                .unwrap()
        };
        // The first import is history (not analysed with "analyse history" off).
        sync();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM jobs", &[]).unwrap(), 0);
        // A game another tracked account (or a PGN import) stored first is still new to
        // this account when it links it.
        db.execute("DELETE FROM account_games", &[]).unwrap();
        sync();
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM account_games", &[])
                .unwrap(),
            1
        );
        assert_eq!(
            db.scalar("SELECT priority FROM jobs", &[]).unwrap(),
            PRIORITY_NEW
        );
        assert!(
            !db.scalar("SELECT last_new_game_at FROM accounts", &[])
                .unwrap()
                .is_null()
        );
        // During a re-import of history (a changed "import since" date), a game that ended
        // after the previous sync is new.
        db.execute("DELETE FROM jobs", &[]).unwrap();
        db.execute(
            "UPDATE accounts SET history_complete=0,last_sync_at=?,last_new_game_at=NULL",
            &[json!(1770000030)],
        )
        .unwrap();
        extra.store(true, Ordering::SeqCst);
        sync();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM jobs", &[]).unwrap(), 1);
        assert_eq!(
            db.scalar("SELECT priority FROM jobs", &[]).unwrap(),
            PRIORITY_NEW
        );
    }
    #[test]
    fn a_pgn_downloaded_from_lichess_is_the_synced_game() {
        let server = FixtureServer::start(move |request| {
            if request.starts_with("GET /api/user/") {
                return (
                    200,
                    String::new(),
                    json!({"username":"fixture","perfs":{}}).to_string(),
                    false,
                );
            }
            let game = fixture_game("Abcd1234", 1770000000123);
            (
                200,
                String::new(),
                format!(
                    "{game}
"
                ),
                false,
            )
        });
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute("INSERT INTO accounts(username,platform,added_at,analyze_history,auto_analyze) VALUES('fixture','lichess',0,0,0)",&[]).unwrap();
        let mut service = Services::new(db.clone()).unwrap();
        Arc::get_mut(&mut service).unwrap().lichess_client =
            LichessClient::fixture(server.base.clone());
        service
            .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
            .unwrap();
        let pgn = "[Site \"https://lichess.org/Abcd1234\"]
[White \"fixture\"]
[Black \"opponent\"]
[Result \"1-0\"]

1. e4 e5 2. Nf3 Nc6 1-0";
        let row = store::game_row_from_pgn(pgn).unwrap();
        let inserted = db
            .transaction(|tx| store::upsert_games(tx, std::slice::from_ref(&row)))
            .unwrap();
        assert!(
            !inserted[0].1,
            "the imported PGN is the game the sync stored"
        );
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 1);
    }
    #[test]
    fn creation_cursor_revisits_ongoing_games_after_restart_and_keeps_equal_time_games() {
        const OLD: i64 = 1770000000123;
        const NEW: i64 = 1770000100456;
        let phase = Arc::new(AtomicU64::new(0));
        let server_phase = phase.clone();
        let server = FixtureServer::start(move |request| {
            let path = request
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap();
            let url = Url::parse(&format!("http://fixture{path}")).unwrap();
            let query: HashMap<_, _> = url
                .query_pairs()
                .map(|(k, v)| (k.into_owned(), v.into_owned()))
                .collect();
            let since = query
                .get("since")
                .and_then(|s| s.parse::<i64>().ok())
                .unwrap_or(0);
            let phase = server_phase.load(Ordering::SeqCst);
            let mut old = fixture_game("longdaily", OLD + 9_000_000);
            old["createdAt"] = json!(OLD);
            old["speed"] = json!("correspondence");
            if phase == 0 {
                old["status"] = json!("started");
                old["winner"] = Value::Null;
                old["pgn"] = Value::Null;
            }
            let mut recent = fixture_game("recent", NEW + 50_000);
            recent["createdAt"] = json!(NEW);
            let mut same_time = fixture_game("equaltime", NEW + 90_000);
            same_time["createdAt"] = json!(NEW);
            let mut games = vec![old, recent];
            if phase == 2 {
                games.push(same_time);
            }
            if request.starts_with("POST /api/games/export/_ids") {
                let ids = request.split_once("\r\n\r\n").unwrap().1;
                let body = games
                    .into_iter()
                    .rev()
                    .filter(|g| ids.split(',').any(|id| g["id"].as_str() == Some(id)))
                    .map(|g| format!("{g}\n"))
                    .collect();
                return (200, String::new(), body, false);
            }
            let body = games
                .into_iter()
                .filter(|g| {
                    g["createdAt"].as_i64().unwrap() >= since
                        && (g["status"] != "started"
                            || query.get("ongoing").is_some_and(|s| s == "true"))
                })
                .map(|g| format!("{g}\n"))
                .collect::<String>();
            (200, String::new(), body, false)
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cursor.db");
        let db = Arc::new(Database::open(&path).unwrap());
        db.execute("INSERT INTO accounts(username,platform,added_at,profile_fetched_at,analyze_history,auto_analyze) VALUES('fixture','lichess',0,?,0,0)", &[json!(now())]).unwrap();
        let mut service = Services::new(db.clone()).unwrap();
        Arc::get_mut(&mut service).unwrap().lichess_client =
            LichessClient::fixture(server.base.clone());
        service
            .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
            .unwrap();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 1);
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM games WHERE uuid='lichess:longdaily'",
                &[]
            )
            .unwrap(),
            0
        );
        assert_eq!(
            db.scalar("SELECT lichess_cursor_ms FROM accounts", &[])
                .unwrap(),
            NEW
        );
        drop(service);
        drop(db);
        phase.store(1, Ordering::SeqCst);
        let db = Arc::new(Database::open(&path).unwrap());
        let mut service = Services::new(db.clone()).unwrap();
        Arc::get_mut(&mut service).unwrap().lichess_client =
            LichessClient::fixture(server.base.clone());
        service
            .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
            .unwrap();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 2);
        assert_eq!(
            db.scalar("SELECT lichess_cursor_ms FROM accounts", &[])
                .unwrap(),
            NEW
        );
        phase.store(2, Ordering::SeqCst);
        service
            .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
            .unwrap();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 3);
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM account_games", &[])
                .unwrap(),
            3
        );
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM games WHERE uuid='lichess:equaltime'",
                &[]
            )
            .unwrap(),
            1
        );
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games WHERE typeof(start_time)!='integer' OR typeof(end_time)!='integer'",&[]).unwrap(),0);
    }

    #[test]
    fn lichess_failed_queue_handoffs_recover_exact_jobs_and_honor_clear_and_preferences() {
        for (initial, priority, preference) in [
            (true, PRIORITY_HISTORY, "analyze_history"),
            (false, PRIORITY_NEW, "auto_analyze"),
        ] {
            for action in ["retry", "clear", "disable"] {
                let server = FixtureServer::start(|_| {
                    (
                        200,
                        String::new(),
                        format!("{}\n", fixture_game("owed", 1770000000123)),
                        false,
                    )
                });
                let db = Arc::new(Database::open(":memory:").unwrap());
                db.execute(
                    "INSERT INTO accounts(username,platform,added_at,profile_fetched_at,history_complete,auto_analyze,analyze_history) VALUES('fixture','lichess',0,?,?,1,1)",
                    &[json!(now()), json!(i64::from(!initial))],
                ).unwrap();
                db.execute("CREATE TRIGGER reject_sync_job BEFORE INSERT ON jobs BEGIN SELECT RAISE(ABORT,'fixture queue failure'); END", &[]).unwrap();
                let service = local_service(db.clone(), &server.base);
                let error = service
                    .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
                    .unwrap_err();
                assert!(error.to_string().contains("fixture queue failure"));
                let game = db.scalar("SELECT id FROM games", &[]).unwrap();
                assert_eq!(
                    db.scalar("SELECT COUNT(*) FROM account_games", &[])
                        .unwrap(),
                    1
                );
                assert_eq!(db.scalar("SELECT COUNT(*) FROM jobs", &[]).unwrap(), 0);
                assert_eq!(
                    db.scalar("SELECT game_id FROM sync_pending", &[]).unwrap(),
                    game
                );
                assert_eq!(
                    db.scalar("SELECT priority FROM sync_pending", &[]).unwrap(),
                    priority
                );
                db.execute("DROP TRIGGER reject_sync_job", &[]).unwrap();
                // Recreate the service: recovery depends on persisted markers, not memory
                // or the timestamp a failed sync may have recorded.
                drop(service);
                db.execute("UPDATE accounts SET last_sync_at=last_sync_at+3600", &[])
                    .unwrap();
                let service = local_service(db.clone(), &server.base);
                match action {
                    "clear" => {
                        service
                            .handle("POST", "/api/queue/clear", &Query::new(), &Value::Null)
                            .unwrap()
                            .unwrap();
                    }
                    "disable" => {
                        db.execute(&format!("UPDATE accounts SET {preference}=0"), &[])
                            .unwrap();
                    }
                    _ => {}
                }
                // A network cooldown cannot postpone an already-persisted handoff.
                *service.lichess_client.serial.lock().unwrap() =
                    Some(Instant::now() + Duration::from_secs(60));
                let before = db.one("SELECT * FROM accounts", &[]).unwrap().unwrap();
                let requests = server.requests.lock().unwrap().len();
                service.sync_account(&before).unwrap();
                assert_eq!(server.requests.lock().unwrap().len(), requests);
                assert_eq!(
                    db.one("SELECT * FROM accounts", &[]).unwrap().unwrap(),
                    before
                );
                assert_eq!(
                    db.scalar("SELECT COUNT(*) FROM sync_pending", &[]).unwrap(),
                    0
                );
                if action == "retry" {
                    assert_eq!(db.scalar("SELECT game_id FROM jobs", &[]).unwrap(), game);
                    assert_eq!(
                        db.scalar("SELECT priority FROM jobs", &[]).unwrap(),
                        priority
                    );
                    service
                        .handle("POST", "/api/queue/clear", &Query::new(), &Value::Null)
                        .unwrap()
                        .unwrap();
                } else {
                    assert_eq!(db.scalar("SELECT COUNT(*) FROM jobs", &[]).unwrap(), 0);
                }
                // Re-exporting the same game must not resurrect any cleared or declined job.
                *service.lichess_client.serial.lock().unwrap() = None;
                service
                    .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
                    .unwrap();
                assert_eq!(db.scalar("SELECT COUNT(*) FROM jobs", &[]).unwrap(), 0);
                assert_eq!(
                    db.scalar("SELECT COUNT(*) FROM sync_pending", &[]).unwrap(),
                    0
                );
            }
        }
    }

    #[test]
    fn lichess_pgn_promotion_does_not_create_or_change_jobs() {
        for enabled in [false, true] {
            for status in ["absent", "done", "queued", "running", "failed"] {
                let server = FixtureServer::start(|_| {
                    (
                        200,
                        String::new(),
                        format!("{}\n", fixture_game("pgnfirst", 1770000000123)),
                        false,
                    )
                });
                let db = Arc::new(Database::open(":memory:").unwrap());
                let imported = store::game_row_from_pgn("[Site \"https://lichess.org/pgnfirst\"]\n[White \"fixture\"]\n[Black \"opponent\"]\n[Result \"1-0\"]\n\n1. e4 $1 {my notes} (1. d4) e5 2. Nf3 Nc6 1-0").unwrap();
                let id = db
                    .transaction(|tx| {
                        Ok(store::upsert_games(tx, std::slice::from_ref(&imported))?[0].0)
                    })
                    .unwrap();
                db.execute("INSERT INTO accounts(id,username,platform,added_at,profile_fetched_at,auto_analyze,analyze_history) VALUES(1,'fixture','lichess',0,?,?,?)", &[json!(now()),json!(enabled),json!(enabled)]).unwrap();
                let service = local_service(db.clone(), &server.base);
                if status != "absent" {
                    db.execute("INSERT INTO jobs(game_id,status,priority,depth,created_at,claim_token,followup_depth,followup_priority,progress,total,partial_json) VALUES(?,?,10,20,0,'keep-claim',22,50,2,4,'{}')", &[json!(id),json!(status)]).unwrap();
                }
                let jobs = db.query("SELECT * FROM jobs", &[]).unwrap();
                service
                    .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
                    .unwrap();
                assert_eq!(
                    db.query("SELECT * FROM jobs", &[]).unwrap(),
                    jobs,
                    "{enabled}/{status}"
                );
                assert_eq!(
                    db.scalar("SELECT COUNT(*) FROM sync_pending", &[]).unwrap(),
                    0
                );
                assert_eq!(
                    db.scalar("SELECT COUNT(*) FROM account_games", &[])
                        .unwrap(),
                    1
                );
                assert_eq!(
                    db.scalar("SELECT pgn FROM games", &[]).unwrap(),
                    imported["pgn"]
                );
            }
        }
    }

    #[test]
    fn lichess_pgn_promotion_rejects_different_or_unparseable_mainlines() {
        for text in [
            "1. d4 d5 1-0",
            "1. e4 e5 2. Nf3 Nc6 3. Bb5 1-0",
            "[FEN \"rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 5 1\"]\n\n1. e4 e5 2. Nf3 Nc6 1-0",
            "broken stored PGN",
        ] {
            let server = FixtureServer::start(|_| {
                (
                    200,
                    String::new(),
                    format!("{}\n", fixture_game("pgnfirst", 1770000000123)),
                    false,
                )
            });
            let db = Arc::new(Database::open(":memory:").unwrap());
            let mut row = store::game_row_from_pgn("[Site \"https://lichess.org/pgnfirst\"]\n[White \"fixture\"]\n[Black \"opponent\"]\n\n1. e4 e5 *").unwrap();
            row["pgn"] = json!(text);
            db.transaction(|tx| store::upsert_games(tx, &[row]))
                .unwrap();
            db.execute("INSERT INTO accounts(id,username,platform,added_at,profile_fetched_at,auto_analyze,analyze_history) VALUES(1,'fixture','lichess',0,?,1,1)",&[json!(now())]).unwrap();
            let service = local_service(db.clone(), &server.base);
            let before = db.query("SELECT * FROM games", &[]).unwrap();
            service
                .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
                .unwrap();
            assert_eq!(
                db.query("SELECT * FROM games", &[]).unwrap(),
                before,
                "{text}"
            );
            assert_eq!(
                db.scalar("SELECT COUNT(*) FROM account_games", &[])
                    .unwrap(),
                0,
                "{text}"
            );
            assert_eq!(db.scalar("SELECT COUNT(*) FROM jobs", &[]).unwrap(), 0);
            assert_eq!(db.scalar("SELECT COUNT(*) FROM events WHERE level='warning' AND message LIKE '%main line%'", &[]).unwrap(),1);
        }
    }

    #[test]
    fn lichess_pgn_import_without_analysis_is_not_caught_up_by_sync() {
        let server = FixtureServer::start(|_| {
            (
                200,
                String::new(),
                format!("{}\n", fixture_game("imported", 1770000000123)),
                false,
            )
        });
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute("INSERT INTO accounts(username,platform,added_at,profile_fetched_at,auto_analyze,analyze_history) VALUES('fixture','lichess',0,?,1,1)", &[json!(now())]).unwrap();
        let service = local_service(db.clone(), &server.base);
        let pgn = "[Site \"https://lichess.org/imported\"]\n[White \"fixture\"]\n[Black \"opponent\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Nf3 Nc6 1-0";
        let (_, imported) = service
            .handle(
                "POST",
                "/api/games/import",
                &Query::new(),
                &json!({"pgn":pgn,"analyze":false}),
            )
            .unwrap()
            .unwrap();
        assert_eq!(imported["imported"], 1);
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM account_games", &[])
                .unwrap(),
            1
        );
        service
            .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
            .unwrap();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 1);
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM sync_pending", &[]).unwrap(),
            0
        );
        assert_eq!(db.scalar("SELECT COUNT(*) FROM jobs", &[]).unwrap(), 0);
    }

    #[test]
    fn lichess_import_uses_preferences_current_when_games_commit() {
        for enabled in [false, true] {
            let db = Arc::new(Database::open(":memory:").unwrap());
            db.execute("INSERT INTO accounts(username,platform,added_at,profile_fetched_at,history_complete,auto_analyze,analyze_history) VALUES('fixture','lichess',0,?,1,?,0)", &[json!(now()), json!(!enabled)]).unwrap();
            let server_db = db.clone();
            let server = FixtureServer::start(move |_| {
                server_db
                    .execute("UPDATE accounts SET auto_analyze=?", &[json!(enabled)])
                    .unwrap();
                (
                    200,
                    String::new(),
                    format!("{}\n", fixture_game("preference", 1770000000123)),
                    false,
                )
            });
            let service = local_service(db.clone(), &server.base);
            service
                .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
                .unwrap();
            assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 1);
            assert_eq!(
                db.scalar("SELECT COUNT(*) FROM sync_pending", &[]).unwrap(),
                0
            );
            assert_eq!(
                db.scalar("SELECT COUNT(*) FROM jobs", &[]).unwrap(),
                i64::from(enabled)
            );
        }
    }

    fn local_service(db: Arc<Database>, base: &Url) -> Arc<Services> {
        let mut service = Services::new(db).unwrap();
        let client = LichessClient::fixture(base.clone());
        Arc::get_mut(&mut service).unwrap().lichess_client = client;
        service
    }

    #[test]
    fn export_deadline_resumes_past_an_ongoing_pin_and_discards_partial_records() {
        const OLD: i64 = 1770000000123;
        const STEP: i64 = 60_000;
        let server = FixtureServer::start(move |request| {
            let path = request
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap();
            let url = Url::parse(&format!("http://fixture{path}")).unwrap();
            let since = url
                .query_pairs()
                .find(|(k, _)| k == "since")
                .and_then(|(_, v)| v.parse::<i64>().ok())
                .unwrap_or(0);
            let record = |id: &str, created: i64| {
                let mut game = fixture_game(id, created + 30_000);
                game["createdAt"] = json!(created);
                game
            };
            let (body, stall) = if since == 0 {
                let mut ongoing = record("oldpending", OLD);
                ongoing["status"] = json!("started");
                ongoing["winner"] = Value::Null;
                ongoing["pgn"] = Value::Null;
                (
                    format!(
                        "{ongoing}\n{}\n{{\"id\":\"partial",
                        record("one", OLD + STEP)
                    ),
                    true,
                )
            } else if since == OLD + STEP {
                (
                    format!(
                        "{}\n{}\n{{\"id\":\"partial",
                        record("one", OLD + STEP),
                        record("two", OLD + 2 * STEP)
                    ),
                    true,
                )
            } else {
                if since != OLD + 2 * STEP {
                    return (
                        400,
                        String::new(),
                        "scan did not advance beyond ongoing cursor".into(),
                        false,
                    );
                }
                (
                    format!(
                        "{}\n{}\n",
                        record("two", OLD + 2 * STEP),
                        record("three", OLD + 3 * STEP)
                    ),
                    false,
                )
            };
            (200, String::new(), body, stall)
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("resume.db");
        let db = Arc::new(Database::open(&path).unwrap());
        // Both review settings on: every game of a first import, in any slice, is history.
        db.execute("INSERT INTO accounts(username,platform,added_at,profile_fetched_at,auto_analyze,analyze_history) VALUES('fixture','lichess',0,?,1,1)",&[json!(now())]).unwrap();
        let mut service = local_service(db.clone(), &server.base);
        Arc::get_mut(&mut service)
            .unwrap()
            .lichess_client
            .export_slice = Duration::from_millis(500);
        for expected in [1, 2] {
            service.sync_tick().unwrap();
            assert_eq!(
                db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(),
                expected
            );
            assert_eq!(
                db.scalar("SELECT lichess_cursor_ms FROM accounts", &[])
                    .unwrap(),
                OLD + expected * STEP
            );
            assert!(
                db.scalar("SELECT lichess_scan_ms FROM accounts", &[])
                    .unwrap()
                    .is_null()
            );
            assert_eq!(
                db.scalar("SELECT history_complete FROM accounts", &[])
                    .unwrap(),
                0
            );
            assert!(
                service.requested.lock().unwrap().contains(&1),
                "partial scan must resume next tick"
            );
        }
        drop(service);
        drop(db);
        let db = Arc::new(Database::open(&path).unwrap());
        let service = local_service(db.clone(), &server.base);
        service.request_sync(1);
        service.sync_tick().unwrap();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 3);
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM jobs WHERE priority=?",
                &[json!(PRIORITY_HISTORY)]
            )
            .unwrap(),
            3,
            "a later slice of the first import is still history"
        );
        assert!(
            db.scalar("SELECT last_new_game_at FROM accounts", &[])
                .unwrap()
                .is_null()
        );
        assert_eq!(
            db.scalar("SELECT lichess_cursor_ms FROM accounts", &[])
                .unwrap(),
            OLD + 3 * STEP
        );
        assert!(
            db.scalar("SELECT lichess_scan_ms FROM accounts", &[])
                .unwrap()
                .is_null()
        );
        assert!(
            db.scalar("SELECT lichess_pending_ms FROM accounts", &[])
                .unwrap()
                .is_null()
        );
        assert!(
            db.scalar("SELECT lichess_finished_ms FROM accounts", &[])
                .unwrap()
                .is_null()
        );
        assert_eq!(
            db.scalar("SELECT history_complete FROM accounts", &[])
                .unwrap(),
            1
        );
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM games WHERE uuid IN('lichess:oldpending','lichess:partial')",
                &[]
            )
            .unwrap(),
            0
        );
    }

    #[test]
    fn stalled_lichess_export_yields_to_chesscom_in_the_same_tick() {
        for rate_limited in [false, true] {
            let server = FixtureServer::start(move |request| {
                if request.starts_with("GET /pub/") {
                    return (
                        200,
                        String::new(),
                        json!({"archives":[]}).to_string(),
                        false,
                    );
                }
                if rate_limited {
                    (429, "Retry-After: 60\r\n".into(), String::new(), false)
                } else {
                    (
                        200,
                        String::new(),
                        format!("{}\n", fixture_game("prefix", 1770000000123)),
                        true,
                    )
                }
            });
            let db = Arc::new(Database::open(":memory:").unwrap());
            db.execute("INSERT INTO accounts(username,platform,added_at,profile_fetched_at,auto_analyze,analyze_history) VALUES('fixture','lichess',0,?,0,0),('chessfixture','chesscom',0,?,0,0)",&[json!(now()),json!(now())]).unwrap();
            let mut service = local_service(db.clone(), &server.base);
            let inner = Arc::get_mut(&mut service).unwrap();
            inner.lichess_client.export_slice = Duration::from_millis(100);
            inner.client = ChessComClient {
                base: server.base.join("pub").unwrap(),
                client: OnceLock::new(),
                serial: Mutex::new(None),
            };
            let watchdog_service = service.clone();
            let (done_tx, done_rx) = std::sync::mpsc::channel();
            let watchdog = std::thread::spawn(move || {
                if done_rx.recv_timeout(Duration::from_secs(5)).is_err() {
                    watchdog_service.request_stop();
                }
            });
            let outcome = service.sync_tick();
            let _ = done_tx.send(());
            watchdog.join().unwrap();
            outcome.unwrap();
            assert!(
                db.scalar(
                    "SELECT last_sync_at FROM accounts WHERE platform='chesscom'",
                    &[]
                )
                .unwrap()
                .is_i64()
            );
            assert!(
                server
                    .requests
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|r| r.starts_with("GET /pub/"))
            );
            assert_eq!(
                db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(),
                i64::from(!rate_limited)
            );
            if rate_limited {
                let count = server.requests.lock().unwrap().len();
                service.sync_tick().unwrap();
                assert_eq!(
                    server.requests.lock().unwrap().len(),
                    count,
                    "cooldown must not issue another HTTP request"
                );
                assert!(
                    service
                        .lichess_client
                        .serial
                        .lock()
                        .unwrap()
                        .unwrap()
                        .saturating_duration_since(Instant::now())
                        > Duration::from_secs(58)
                );
            }
        }
    }

    #[test]
    fn adding_account_times_out_on_the_lichess_request_slot() {
        let server = FixtureServer::start(|_| {
            (
                200,
                String::new(),
                json!({"username":"newfixture"}).to_string(),
                false,
            )
        });
        let db = Arc::new(Database::open(":memory:").unwrap());
        let mut service = local_service(db.clone(), &server.base);
        Arc::get_mut(&mut service)
            .unwrap()
            .lichess_client
            .profile_lock_wait = Duration::from_millis(50);
        let holder_service = service.clone();
        let (held_tx, held_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        let holder = std::thread::spawn(move || {
            let _gate = holder_service.lichess_client.serial.lock().unwrap();
            held_tx.send(()).unwrap();
            let _ = release_rx.recv_timeout(Duration::from_secs(2));
        });
        held_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        let result = service
            .handle(
                "POST",
                "/api/accounts",
                &Query::new(),
                &json!({"username":"newfixture","platform":"lichess"}),
            )
            .unwrap();
        let _ = release_tx.send(());
        holder.join().unwrap();
        let error = result.unwrap_err();
        assert_eq!(error.downcast_ref::<crate::ApiError>().unwrap().0, 503);
        assert!(
            error
                .to_string()
                .contains("timed out waiting for its request slot")
        );
        assert_eq!(db.scalar("SELECT COUNT(*) FROM accounts", &[]).unwrap(), 0);
        assert!(server.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn adding_account_does_not_wait_through_a_persisted_rate_limit() {
        let server = FixtureServer::start(|_| panic!("a request must not bypass the cooldown"));
        let db = Arc::new(Database::open(":memory:").unwrap());
        let mut service = local_service(db.clone(), &server.base);
        Arc::get_mut(&mut service)
            .unwrap()
            .lichess_client
            .profile_lock_wait = Duration::from_millis(50);
        *service.lichess_client.serial.lock().unwrap() =
            Some(Instant::now() + Duration::from_secs(60));
        let watchdog_service = service.clone();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let watchdog = std::thread::spawn(move || {
            if done_rx.recv_timeout(Duration::from_secs(2)).is_err() {
                watchdog_service.request_stop();
            }
        });
        let result = service
            .handle(
                "POST",
                "/api/accounts",
                &Query::new(),
                &json!({"username":"newfixture","platform":"lichess"}),
            )
            .unwrap();
        let _ = done_tx.send(());
        watchdog.join().unwrap();
        let error = result.unwrap_err();
        assert_eq!(error.downcast_ref::<crate::ApiError>().unwrap().0, 503);
        assert!(error.to_string().contains("rate limited"));
        assert!(error.to_string().contains("request slot"));
        assert_eq!(db.scalar("SELECT COUNT(*) FROM accounts", &[]).unwrap(), 0);
    }

    #[test]
    fn changed_history_range_cannot_publish_an_old_export_checkpoint() {
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute("INSERT INTO accounts(username,platform,added_at,profile_fetched_at,auto_analyze,analyze_history) VALUES('fixture','lichess',0,?,0,0)",&[json!(now())]).unwrap();
        let server_db = db.clone();
        let server = FixtureServer::start(move |_| {
            // The account's scope changes while its HTTP request is outstanding.
            server_db.execute("UPDATE accounts SET import_since='2025/01',history_complete=0,lichess_cursor_ms=NULL,lichess_scan_ms=NULL,lichess_pending_ms=NULL,lichess_finished_ms=NULL",&[]).unwrap();
            (
                200,
                String::new(),
                format!("{}\n", fixture_game("stalescope", 1770000000123)),
                false,
            )
        });
        let service = local_service(db.clone(), &server.base);
        let old_account = db.one("SELECT * FROM accounts", &[]).unwrap().unwrap();
        service.sync_account(&old_account).unwrap();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 0);
        assert_eq!(
            db.scalar("SELECT import_since FROM accounts", &[]).unwrap(),
            "2025/01"
        );
        assert_eq!(
            db.scalar("SELECT history_complete FROM accounts", &[])
                .unwrap(),
            0
        );
        assert!(
            db.scalar("SELECT lichess_cursor_ms FROM accounts", &[])
                .unwrap()
                .is_null()
        );
        assert!(
            db.scalar("SELECT last_sync_at FROM accounts", &[])
                .unwrap()
                .is_null()
        );
    }

    #[test]
    fn unmappable_games_warn_without_blocking_this_or_later_syncs() {
        const BASE_TIME: i64 = 1770000000123;
        let mut too_long = fixture_game("badplies", BASE_TIME + 60_000);
        too_long["pgn"] = json!(format!(
            "{} 1-0",
            "Nf3 Nf6 Ng1 Ng8 ".repeat(crate::pgn::MAX_PLIES / 4 + 1)
        ));
        let mut bad_fen = fixture_game("badfen", BASE_TIME + 61_000);
        bad_fen["pgn"] = json!("[SetUp \"1\"]\n[FEN \"8/8/8/8/8/8/8/8 w - - 0 1\"]\n\n1-0");
        let mut bad_speed = fixture_game("badspeed", BASE_TIME + 62_000);
        bad_speed["speed"] = json!("unknown");
        let good = fixture_game("validafter", BASE_TIME + 63_000);
        let mut bad_newest = fixture_game("badnewest", BASE_TIME + 64_000);
        bad_newest["speed"] = json!("unknown");
        let games = [too_long, bad_fen, bad_speed, good, bad_newest];
        let server = FixtureServer::start(move |request| {
            let path = request
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap();
            let url = Url::parse(&format!("http://fixture{path}")).unwrap();
            let since = url
                .query_pairs()
                .find(|(key, _)| key == "since")
                .and_then(|(_, v)| v.parse::<i64>().ok())
                .unwrap_or(0);
            (
                200,
                String::new(),
                games
                    .iter()
                    .filter(|g| g["createdAt"].as_i64().unwrap() >= since)
                    .map(|g| format!("{g}\n"))
                    .collect(),
                false,
            )
        });
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute("INSERT INTO accounts(username,platform,added_at,profile_fetched_at,auto_analyze,analyze_history) VALUES('fixture','lichess',0,?,0,0)",&[json!(now())]).unwrap();
        let service = local_service(db.clone(), &server.base);
        service
            .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
            .unwrap();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 1);
        assert_eq!(
            db.scalar("SELECT uuid FROM games", &[]).unwrap(),
            "lichess:validafter"
        );
        assert_eq!(
            db.scalar("SELECT lichess_cursor_ms FROM accounts", &[])
                .unwrap(),
            BASE_TIME + 4_000
        );
        let warnings = db
            .query(
                "SELECT message,account_id FROM events WHERE level='warning' ORDER BY id",
                &[],
            )
            .unwrap();
        assert_eq!(warnings.len(), 4);
        for (warning, id) in warnings
            .iter()
            .zip(["badplies", "badfen", "badspeed", "badnewest"])
        {
            assert!(warning["message"].as_str().unwrap().contains(id));
            assert_eq!(warning["account_id"], 1);
        }
        drop(service);
        let service = local_service(db.clone(), &server.base);
        for _ in 0..3 {
            service
                .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
                .unwrap();
        }
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 1);
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM events WHERE level='warning'", &[])
                .unwrap(),
            4,
            "the newest unmappable game must not repeat its warning at the inclusive boundary"
        );
    }

    #[test]
    fn lichess_429_keeps_its_minute_cooldown_without_holding_the_request_slot() {
        assert_eq!(retry_delay(429, Some("1"), 0), Duration::from_secs(60));
        assert_eq!(retry_delay(429, Some("120"), 0), Duration::from_secs(120));
        let server =
            FixtureServer::start(|_| (429, "Retry-After: 1\r\n".into(), String::new(), false));
        let client = LichessClient::fixture(server.base.clone());
        let stop = AtomicBool::new(false);
        let error = client
            .export("fixture", 0, "", &stop, CreationCursor::default())
            .err()
            .unwrap();
        assert!(error.is::<RateLimited>());
        assert_eq!(client.backoffs.load(Ordering::SeqCst), 1);
        assert_eq!(server.requests.lock().unwrap().len(), 1);
        assert!(
            client
                .serial
                .lock()
                .unwrap()
                .unwrap()
                .saturating_duration_since(Instant::now())
                > Duration::from_secs(58)
        );
        let error = client.profile("fixture", "", &stop).unwrap_err();
        assert_eq!(error.downcast_ref::<crate::ApiError>().unwrap().0, 503);
        assert!(error.to_string().contains("rate limited"));
        assert_eq!(server.requests.lock().unwrap().len(), 1);
        stop.store(true, Ordering::SeqCst);
        let error = client
            .export("fixture", 0, "", &stop, CreationCursor::default())
            .err()
            .unwrap();
        assert!(error.to_string().contains("cancelled"));
    }

    #[test]
    fn server_error_is_not_reported_as_a_successful_partial_export() {
        let server = FixtureServer::start(|_| (503, String::new(), String::new(), false));
        let mut client = LichessClient::fixture(server.base.clone());
        client.export_slice = Duration::from_millis(100);
        let error = client
            .export(
                "fixture",
                0,
                "",
                &AtomicBool::new(false),
                CreationCursor::default(),
            )
            .err()
            .unwrap();
        assert_eq!(error.downcast_ref::<crate::ApiError>().unwrap().0, 502);
        assert!(error.to_string().contains("HTTP 503"));
        assert_eq!(server.requests.lock().unwrap().len(), 1);
        assert!(client.serial.lock().unwrap().unwrap() > Instant::now());
    }

    #[test]
    fn lichess_cancelled_stream_and_serial_wait_finish_promptly_without_publishing() {
        let server = FixtureServer::start(|_| {
            (
                200,
                String::new(),
                format!("{}\n", fixture_game("cancelled", 1770000000123)),
                true,
            )
        });
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute("INSERT INTO accounts(username,platform,added_at,profile_fetched_at) VALUES('fixture','lichess',0,?)",&[json!(now())]).unwrap();
        let mut service = Services::new(db.clone()).unwrap();
        Arc::get_mut(&mut service).unwrap().lichess_client =
            LichessClient::fixture(server.base.clone());
        let worker_service = service.clone();
        let account = db.one("SELECT * FROM accounts", &[]).unwrap().unwrap();
        let worker = std::thread::spawn(move || worker_service.sync_account(&account));
        server.wait_requests(1);
        let waiter_service = service.clone();
        let waiter = std::thread::spawn(move || {
            waiter_service
                .lichess_client
                .profile("fixture", "", &waiter_service.stop)
        });
        std::thread::sleep(Duration::from_millis(100));
        let started = Instant::now();
        service.stop.store(true, Ordering::SeqCst);
        assert!(
            worker
                .join()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("cancelled")
        );
        assert!(
            waiter
                .join()
                .unwrap()
                .unwrap_err()
                .to_string()
                .contains("cancelled")
        );
        assert!(started.elapsed() < Duration::from_secs(1));
        assert_eq!(server.requests.lock().unwrap().len(), 1);
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 0);
        assert_eq!(db.scalar("SELECT COUNT(*) FROM jobs", &[]).unwrap(), 0);
        assert_eq!(
            db.scalar("SELECT history_complete FROM accounts", &[])
                .unwrap(),
            0
        );
    }

    #[test]
    fn lichess_truncated_or_malformed_export_does_not_advance_cursor() {
        let server = FixtureServer::start(|_| {
            (
                200,
                String::new(),
                format!(
                    "{}\n{{invalid}}",
                    fixture_game("unpublished", 1770000000123)
                ),
                false,
            )
        });
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute("INSERT INTO accounts(username,platform,added_at,profile_fetched_at) VALUES('fixture','lichess',0,?)",&[json!(now())]).unwrap();
        let mut service = Services::new(db.clone()).unwrap();
        Arc::get_mut(&mut service).unwrap().lichess_client =
            LichessClient::fixture(server.base.clone());
        assert!(
            service
                .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
                .is_err()
        );
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 0);
        assert_eq!(
            db.scalar("SELECT history_complete FROM accounts", &[])
                .unwrap(),
            0
        );
    }

    #[test]
    fn ongoing_ids_do_not_pin_discovery_and_are_rechecked_after_restart() {
        const OLD: i64 = 1770000000000;
        const LATEST: i64 = OLD + 100_000;
        let finished = Arc::new(AtomicBool::new(false));
        let server_finished = finished.clone();
        let server = FixtureServer::start(move |request| {
            let path = request
                .lines()
                .next()
                .unwrap()
                .split_whitespace()
                .nth(1)
                .unwrap();
            let url = Url::parse(&format!("http://fixture{path}")).unwrap();
            let mut games = Vec::new();
            for (id, created) in [("dailyone", OLD), ("dailytwo", OLD + 1)] {
                let mut game = fixture_game(id, created + 60_000);
                if !server_finished.load(Ordering::SeqCst) {
                    game["status"] = json!("started");
                    game["winner"] = Value::Null;
                    game["pgn"] = Value::Null;
                }
                games.push(game);
            }
            if request.starts_with("POST ") {
                assert_eq!(url.path(), "/api/games/export/_ids");
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("content-type: text/plain")
                );
                assert!(
                    request
                        .to_ascii_lowercase()
                        .contains("accept: application/x-ndjson")
                );
                assert!(request.contains("evals=false") && request.contains("accuracy=false"));
                assert_eq!(
                    request.split_once("\r\n\r\n").unwrap().1,
                    "dailyone,dailytwo"
                );
                // The IDs API is reverse chronological and an unrequested game
                // must not be accepted, even when the username matches.
                games.reverse();
                games.push(fixture_game("unsolicited", LATEST + 60_000));
            } else {
                assert!(
                    url.query_pairs()
                        .any(|(k, v)| k == "ongoing" && v == "true")
                );
                let since = url
                    .query_pairs()
                    .find(|(k, _)| k == "since")
                    .and_then(|(_, v)| v.parse::<i64>().ok())
                    .unwrap_or(0);
                for n in 1..=100 {
                    games.push(fixture_game(&format!("recent{n}"), OLD + n * 1000 + 60_000));
                }
                games.retain(|g| g["createdAt"].as_i64().unwrap() >= since);
                if since > 0 {
                    assert_eq!(since, LATEST, "discovery must advance past ongoing games");
                    assert_eq!(games.len(), 1);
                }
            }
            (
                200,
                String::new(),
                games.into_iter().map(|g| format!("{g}\n")).collect(),
                false,
            )
        });
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pending.db");
        let db = Arc::new(Database::open(&path).unwrap());
        db.execute("INSERT INTO accounts(username,platform,added_at,profile_fetched_at,auto_analyze,analyze_history) VALUES('fixture','lichess',0,?,0,0)",&[json!(now())]).unwrap();
        let service = local_service(db.clone(), &server.base);
        service
            .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
            .unwrap();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 100);
        assert_eq!(
            db.scalar("SELECT lichess_cursor_ms FROM accounts", &[])
                .unwrap(),
            LATEST
        );
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM lichess_pending_games", &[])
                .unwrap(),
            2
        );
        drop(service);
        drop(db);
        let db = Arc::new(Database::open(&path).unwrap());
        let service = local_service(db.clone(), &server.base);
        service
            .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
            .unwrap();
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(),
            100,
            "ongoing and unsolicited games must not be stored"
        );
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM lichess_pending_games WHERE checked_at>0",
                &[]
            )
            .unwrap(),
            2
        );
        // A second immediate sync discovers only the inclusive boundary, without
        // hammering the ongoing-ID endpoint.
        service
            .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
            .unwrap();
        assert_eq!(
            server
                .requests
                .lock()
                .unwrap()
                .iter()
                .filter(|r| r.starts_with("POST "))
                .count(),
            1
        );
        finished.store(true, Ordering::SeqCst);
        db.execute("UPDATE lichess_pending_games SET checked_at=0", &[])
            .unwrap();
        service
            .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
            .unwrap();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 102);
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM lichess_pending_games", &[])
                .unwrap(),
            0
        );
        assert_eq!(
            db.scalar("SELECT lichess_cursor_ms FROM accounts", &[])
                .unwrap(),
            LATEST
        );
    }

    #[test]
    fn rate_limited_sync_does_not_write_or_claim_a_success_and_resumes_when_due() {
        for profile_fresh in [false, true] {
            let limited = Arc::new(AtomicBool::new(true));
            let server_limited = limited.clone();
            let server = FixtureServer::start(move |request| {
                if server_limited.load(Ordering::SeqCst) {
                    (429, "Retry-After: 1\r\n".into(), String::new(), false)
                } else if request.starts_with("GET /api/user/") {
                    (
                        200,
                        String::new(),
                        json!({"username":"fixture","perfs":{}}).to_string(),
                        false,
                    )
                } else {
                    (
                        200,
                        String::new(),
                        format!("{}\n", fixture_game("aftercooldown", 1770000000123)),
                        false,
                    )
                }
            });
            let db = Arc::new(Database::open(":memory:").unwrap());
            db.execute("INSERT INTO accounts(username,platform,added_at,profile_fetched_at,last_sync_at,sync_enabled,auto_analyze,analyze_history) VALUES('fixture','lichess',0,?,123,0,0,0)",&[json!(if profile_fresh {now()} else {0})]).unwrap();
            let service = local_service(db.clone(), &server.base);
            let before = db.one("SELECT * FROM accounts", &[]).unwrap().unwrap();
            let revision = db
                .scalar("SELECT revision FROM data_revision WHERE id=1", &[])
                .unwrap();
            service.request_sync(1);
            for _ in 0..4 {
                service.sync_tick().unwrap();
            }
            assert_eq!(
                db.one("SELECT * FROM accounts", &[]).unwrap().unwrap(),
                before
            );
            assert_eq!(
                db.scalar("SELECT revision FROM data_revision WHERE id=1", &[])
                    .unwrap(),
                revision
            );
            assert_eq!(db.scalar("SELECT COUNT(*) FROM events", &[]).unwrap(), 0);
            assert_eq!(server.requests.lock().unwrap().len(), 1);
            assert!(
                service.requested.lock().unwrap().contains(&1),
                "manual requests survive cooldown without waking the loop"
            );
            limited.store(false, Ordering::SeqCst);
            *service.lichess_client.serial.lock().unwrap() =
                Some(Instant::now() - Duration::from_secs(1));
            service.sync_tick().unwrap();
            assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 1);
            assert!(
                db.scalar("SELECT last_sync_at FROM accounts", &[])
                    .unwrap()
                    .as_i64()
                    .unwrap()
                    > 123
            );
        }
    }

    #[test]
    fn interrupted_stream_commits_complete_prefix_and_resumes_without_error_backoff() {
        const FIRST: i64 = 1770000000123;
        let request_count = Arc::new(AtomicU64::new(0));
        let server_count = request_count.clone();
        let server = FixtureServer::start(move |request| {
            let first = fixture_game("prefix", FIRST);
            if server_count.fetch_add(1, Ordering::SeqCst) == 0 {
                (
                    200,
                    "X-Fixture-Truncate: yes\r\n".into(),
                    format!("{first}\n{{\"id\":\"partial"),
                    false,
                )
            } else {
                assert!(request.contains(&format!("since={}", FIRST - 60_000)));
                (
                    200,
                    String::new(),
                    format!("{first}\n{}\n", fixture_game("resumed", FIRST + 1000)),
                    false,
                )
            }
        });
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute("INSERT INTO accounts(username,platform,added_at,profile_fetched_at,auto_analyze,analyze_history) VALUES('fixture','lichess',0,?,0,0)",&[json!(now())]).unwrap();
        let service = local_service(db.clone(), &server.base);
        service.sync_tick().unwrap();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 1);
        assert_eq!(
            db.scalar("SELECT lichess_cursor_ms FROM accounts", &[])
                .unwrap(),
            FIRST - 60_000
        );
        assert_eq!(
            db.scalar("SELECT history_complete FROM accounts", &[])
                .unwrap(),
            0
        );
        assert_eq!(
            db.scalar("SELECT sync_status FROM accounts", &[]).unwrap(),
            "idle"
        );
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM events WHERE level='warning'", &[])
                .unwrap(),
            0
        );
        assert!(service.requested.lock().unwrap().contains(&1));
        service.sync_tick().unwrap();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 2);
        assert_eq!(
            db.scalar("SELECT history_complete FROM accounts", &[])
                .unwrap(),
            1
        );
        assert_eq!(server.requests.lock().unwrap().len(), 2);
    }

    #[test]
    fn pending_export_pins_redirects_and_validates_the_id_batch() {
        let server = FixtureServer::start(|_| {
            (
                307,
                "Location: /api/games/user/other\r\n".into(),
                String::new(),
                false,
            )
        });
        let client = LichessClient::fixture(server.base.clone());
        let stop = AtomicBool::new(false);
        let error = client
            .pending(
                "fixture",
                &["safeid".into()],
                "",
                &stop,
                Instant::now() + Duration::from_secs(2),
            )
            .err()
            .unwrap();
        assert!(
            error
                .to_string()
                .contains("outside this account and origin")
        );
        for ids in [vec!["bad,id".into()], vec!["ok".into(); 301], vec![]] {
            assert!(
                client
                    .pending(
                        "fixture",
                        &ids,
                        "",
                        &stop,
                        Instant::now() + Duration::from_secs(2)
                    )
                    .is_err()
            );
        }
        assert_eq!(server.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn interrupted_pending_response_rotates_unseen_ids_before_its_completed_prefix() {
        let calls = Arc::new(AtomicU64::new(0));
        let server_calls = calls.clone();
        let server = FixtureServer::start(move |request| {
            if !request.starts_with("POST ") {
                return (200, String::new(), String::new(), false);
            }
            let ids = request.split_once("\r\n\r\n").unwrap().1;
            let make_ongoing = |id: &str| {
                let mut game = fixture_game(id, 1770000000123);
                game["status"] = json!("started");
                game["pgn"] = Value::Null;
                game
            };
            if server_calls.fetch_add(1, Ordering::SeqCst) == 0 {
                assert_eq!(ids, "aaa,bbb,ccc");
                (
                    200,
                    "X-Fixture-Truncate: yes\r\n".into(),
                    format!("{}\n{{\"id\":", make_ongoing("ccc")),
                    false,
                )
            } else {
                assert_eq!(
                    ids, "aaa,bbb",
                    "reverse-order prefixes must not continually replay already checked IDs"
                );
                (
                    200,
                    String::new(),
                    format!("{}\n{}\n", make_ongoing("bbb"), make_ongoing("aaa")),
                    false,
                )
            }
        });
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute("INSERT INTO accounts(username,platform,added_at,profile_fetched_at,history_complete,auto_analyze,analyze_history) VALUES('fixture','lichess',0,?,1,0,0)",&[json!(now())]).unwrap();
        for id in ["aaa", "bbb", "ccc"] {
            db.execute("INSERT INTO lichess_pending_games(account_id,game_id,created_at_ms) VALUES(1,?,1770000000000)",&[json!(id)]).unwrap();
        }
        let service = local_service(db.clone(), &server.base);
        service.sync_tick().unwrap();
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM lichess_pending_games WHERE checked_at>0",
                &[]
            )
            .unwrap(),
            1
        );
        assert!(service.requested.lock().unwrap().contains(&1));
        service.sync_tick().unwrap();
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM lichess_pending_games WHERE checked_at>0",
                &[]
            )
            .unwrap(),
            3
        );
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 0);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn interruption_before_the_first_record_backs_off_and_reports_persistent_failure_once() {
        let recovered = Arc::new(AtomicBool::new(false));
        let server_recovered = recovered.clone();
        let server = FixtureServer::start(move |_| {
            if server_recovered.load(Ordering::SeqCst) {
                (
                    200,
                    String::new(),
                    format!("{}\n", fixture_game("recovered", 1770000000123)),
                    false,
                )
            } else {
                (
                    200,
                    "X-Fixture-Truncate: yes\r\n".into(),
                    "{\"id\":".into(),
                    false,
                )
            }
        });
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute("INSERT INTO accounts(username,platform,added_at,profile_fetched_at,auto_analyze,analyze_history) VALUES('fixture','lichess',0,?,0,0)",&[json!(now())]).unwrap();
        let before = db.one("SELECT * FROM accounts", &[]).unwrap().unwrap();
        let service = local_service(db.clone(), &server.base);
        let mut revision_after_error = None;
        for (index, seconds) in [30, 60, 120, 240, 480, 600, 600].into_iter().enumerate() {
            if index > 0 {
                expire_no_progress(&service);
            }
            service.sync_tick().unwrap();
            let retries = service.lichess_client.retries.lock().unwrap();
            let retry = retries.get(&1).unwrap();
            assert_eq!(retry.failures, index as u32 + 1);
            let remaining = retry.until.saturating_duration_since(Instant::now());
            assert!(
                remaining > Duration::from_secs(seconds - 1)
                    && remaining <= Duration::from_secs(seconds)
            );
            drop(retries);
            assert_eq!(server.requests.lock().unwrap().len(), index + 1);
            // A manual request cannot bypass the retry deadline either.
            service.request_sync(1);
            service.sync_tick().unwrap();
            assert_eq!(server.requests.lock().unwrap().len(), index + 1);
            if index < 2 {
                assert_eq!(
                    db.one("SELECT * FROM accounts", &[]).unwrap().unwrap(),
                    before
                );
                assert_eq!(db.scalar("SELECT COUNT(*) FROM events", &[]).unwrap(), 0);
            } else {
                assert_eq!(
                    db.scalar("SELECT sync_status FROM accounts", &[]).unwrap(),
                    "error"
                );
                assert!(
                    db.scalar("SELECT sync_error FROM accounts", &[])
                        .unwrap()
                        .as_str()
                        .unwrap()
                        .contains("no progress")
                );
                assert_eq!(db.scalar("SELECT COUNT(*) FROM events", &[]).unwrap(), 1);
                let revision = db
                    .scalar("SELECT revision FROM data_revision", &[])
                    .unwrap();
                if let Some(previous) = &revision_after_error {
                    assert_eq!(&revision, previous);
                } else {
                    revision_after_error = Some(revision);
                }
            }
            assert!(
                db.scalar("SELECT last_sync_at FROM accounts", &[])
                    .unwrap()
                    .is_null()
            );
        }
        recovered.store(true, Ordering::SeqCst);
        expire_no_progress(&service);
        service.sync_tick().unwrap();
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 1);
        assert_eq!(
            db.scalar("SELECT sync_status FROM accounts", &[]).unwrap(),
            "idle"
        );
        assert!(
            db.scalar("SELECT sync_error FROM accounts", &[])
                .unwrap()
                .is_null()
        );
        assert!(
            !service
                .lichess_client
                .retries
                .lock()
                .unwrap()
                .contains_key(&1)
        );
        // Progress/success clears the old failure count.
        recovered.store(false, Ordering::SeqCst);
        service.request_sync(1);
        service.sync_tick().unwrap();
        assert_eq!(
            service
                .lichess_client
                .retries
                .lock()
                .unwrap()
                .get(&1)
                .unwrap()
                .failures,
            1
        );
    }

    fn expire_no_progress(service: &Services) {
        if let Some(retry) = service.lichess_client.retries.lock().unwrap().get_mut(&1) {
            retry.until = Instant::now() - Duration::from_secs(1);
        }
    }

    #[test]
    fn empty_incomplete_pending_slice_does_not_publish_a_successful_discovery() {
        for deadline in [false, true] {
            let server = FixtureServer::start(move |request| {
                if request.starts_with("POST ") {
                    (
                        200,
                        if deadline {
                            String::new()
                        } else {
                            "X-Fixture-Truncate: yes\r\n".into()
                        },
                        "{\"id\":".into(),
                        deadline,
                    )
                } else {
                    (200, String::new(), String::new(), false)
                }
            });
            let db = Arc::new(Database::open(":memory:").unwrap());
            db.execute("INSERT INTO accounts(username,platform,added_at,profile_fetched_at,last_sync_at,history_complete,lichess_cursor_ms,auto_analyze,analyze_history) VALUES('fixture','lichess',0,?,123,1,1770000000000,0,0)",&[json!(now())]).unwrap();
            db.execute("INSERT INTO lichess_pending_games(account_id,game_id,created_at_ms) VALUES(1,'pending',1760000000000)",&[]).unwrap();
            let before = db.one("SELECT * FROM accounts", &[]).unwrap().unwrap();
            let revision = db
                .scalar("SELECT revision FROM data_revision", &[])
                .unwrap();
            let mut service = local_service(db.clone(), &server.base);
            Arc::get_mut(&mut service)
                .unwrap()
                .lichess_client
                .export_slice = Duration::from_millis(500);
            for attempt in 0..2 {
                if attempt > 0 {
                    expire_no_progress(&service);
                }
                service
                    .sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap())
                    .unwrap();
                assert_eq!(
                    db.one("SELECT * FROM accounts", &[]).unwrap().unwrap(),
                    before
                );
                assert_eq!(
                    db.scalar("SELECT revision FROM data_revision", &[])
                        .unwrap(),
                    revision
                );
                assert_eq!(
                    db.scalar("SELECT checked_at FROM lichess_pending_games", &[])
                        .unwrap(),
                    0
                );
                assert_eq!(
                    db.scalar("SELECT misses FROM lichess_pending_games", &[])
                        .unwrap(),
                    0
                );
                assert_eq!(db.scalar("SELECT COUNT(*) FROM events", &[]).unwrap(), 0);
                assert_eq!(server.requests.lock().unwrap().len(), (attempt + 1) * 2);
                service.sync_tick().unwrap();
                assert_eq!(
                    server.requests.lock().unwrap().len(),
                    (attempt + 1) * 2,
                    "complete discovery must not bypass pending backoff"
                );
            }
        }
    }

    #[test]
    fn a_failed_export_still_imports_the_finished_pending_games() {
        let server = FixtureServer::start(move |request| {
            if request.starts_with("POST ") {
                // The rechecked game has finished.
                let game = fixture_game("done", 1770000000123);
                return (
                    200,
                    String::new(),
                    format!(
                        "{game}
"
                    ),
                    false,
                );
            }
            (500, String::new(), "{}".into(), false)
        });
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute("INSERT INTO accounts(username,platform,added_at,profile_fetched_at,history_complete,auto_analyze,analyze_history) VALUES('fixture','lichess',0,?,1,0,0)",&[json!(now())]).unwrap();
        db.execute("INSERT INTO lichess_pending_games(account_id,game_id,created_at_ms) VALUES(1,'done',1770000000000)",&[]).unwrap();
        let service = local_service(db.clone(), &server.base);
        let result = service.sync_account(&db.one("SELECT * FROM accounts", &[]).unwrap().unwrap());
        assert!(result.is_err(), "the export failure is still reported");
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM games WHERE source='lichess'", &[])
                .unwrap(),
            1
        );
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM lichess_pending_games", &[])
                .unwrap(),
            0
        );
    }
    #[test]
    fn missing_pending_ids_expire_only_after_complete_omissions_and_seen_ids_reset() {
        let phase = Arc::new(AtomicU64::new(0));
        let server_phase = phase.clone();
        let server = FixtureServer::start(move |request| {
            if !request.starts_with("POST ") {
                return (200, String::new(), String::new(), false);
            }
            match server_phase.load(Ordering::SeqCst) {
                1 => (
                    200,
                    "X-Fixture-Truncate: yes\r\n".into(),
                    "{\"id\":".into(),
                    false,
                ),
                2 => {
                    let mut present = fixture_game("returns", 1770000000123);
                    present["status"] = json!("started");
                    present["pgn"] = Value::Null;
                    let mut foreign = fixture_game("foreign", 1770000000123);
                    foreign["players"]["white"]["user"]["name"] = json!("stranger");
                    (200, String::new(), format!("{present}\n{foreign}\n"), false)
                }
                _ => (200, String::new(), String::new(), false),
            }
        });
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute("INSERT INTO accounts(username,platform,added_at,profile_fetched_at,history_complete,auto_analyze,analyze_history) VALUES('fixture','lichess',0,?,1,0,0),('other','lichess',0,?,1,0,0)",&[json!(now()),json!(now())]).unwrap();
        for id in ["gone", "returns", "foreign"] {
            db.execute("INSERT INTO lichess_pending_games(account_id,game_id,created_at_ms) VALUES(1,?,1770000000000)",&[json!(id)]).unwrap();
        }
        db.execute("INSERT INTO lichess_pending_games(account_id,game_id,created_at_ms,misses) VALUES(2,'gone',1770000000000,2)",&[]).unwrap();
        let service = local_service(db.clone(), &server.base);
        let sync = || {
            service
                .sync_account(
                    &db.one("SELECT * FROM accounts WHERE id=1", &[])
                        .unwrap()
                        .unwrap(),
                )
                .unwrap()
        };
        sync();
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM lichess_pending_games WHERE account_id=1 AND misses=1",
                &[]
            )
            .unwrap(),
            3
        );
        db.execute(
            "UPDATE lichess_pending_games SET checked_at=0 WHERE account_id=1",
            &[],
        )
        .unwrap();
        phase.store(1, Ordering::SeqCst);
        sync();
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM lichess_pending_games WHERE account_id=1 AND misses=1",
                &[]
            )
            .unwrap(),
            3,
            "an incomplete response must not age missing IDs"
        );
        phase.store(2, Ordering::SeqCst);
        expire_no_progress(&service);
        sync();
        assert_eq!(
            db.scalar(
                "SELECT misses FROM lichess_pending_games WHERE account_id=1 AND game_id='returns'",
                &[]
            )
            .unwrap(),
            0
        );
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM lichess_pending_games WHERE account_id=1 AND misses=2",
                &[]
            )
            .unwrap(),
            2
        );
        db.execute(
            "UPDATE lichess_pending_games SET checked_at=0 WHERE account_id=1",
            &[],
        )
        .unwrap();
        phase.store(0, Ordering::SeqCst);
        sync();
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM lichess_pending_games WHERE account_id=1",
                &[]
            )
            .unwrap(),
            1
        );
        // Dropped games are reported, so a lost game is visible on Activity.
        let warning = db
            .scalar(
                "SELECT message FROM events WHERE message LIKE 'Stopped waiting%'",
                &[],
            )
            .unwrap();
        assert!(
            warning
                .as_str()
                .unwrap()
                .contains("2 unfinished Lichess game(s)"),
            "{warning}"
        );
        assert_eq!(
            db.scalar(
                "SELECT misses FROM lichess_pending_games WHERE account_id=1",
                &[]
            )
            .unwrap(),
            1
        );
        assert_eq!(
            db.scalar(
                "SELECT misses FROM lichess_pending_games WHERE account_id=2",
                &[]
            )
            .unwrap(),
            2
        );
        db.execute(
            "UPDATE lichess_pending_games SET checked_at=0 WHERE account_id=1",
            &[],
        )
        .unwrap();
        phase.store(2, Ordering::SeqCst);
        sync();
        let requests = server.requests.lock().unwrap();
        let ids = requests
            .iter()
            .rev()
            .find(|r| r.starts_with("POST "))
            .unwrap()
            .split_once("\r\n\r\n")
            .unwrap()
            .1;
        assert_eq!(
            ids, "returns",
            "expired IDs must no longer crowd the pending batch"
        );
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 0);
    }

    #[test]
    fn pending_results_before_a_discovery_429_do_not_change_account_metadata() {
        let limited = Arc::new(AtomicBool::new(true));
        let server_limited = limited.clone();
        let server = FixtureServer::start(move |request| {
            if request.starts_with("GET /api/user/") {
                (
                    200,
                    String::new(),
                    json!({"username":"fixture","perfs":{"blitz":{"rating":1999}}}).to_string(),
                    false,
                )
            } else if request.starts_with("POST ") {
                let mut ongoing = fixture_game("ongoing", 1770000000123);
                ongoing["status"] = json!("started");
                ongoing["pgn"] = Value::Null;
                let mut bad = fixture_game("bad", 1770000002123);
                bad["speed"] = json!("unknown");
                (
                    200,
                    String::new(),
                    format!(
                        "{ongoing}\n{}\n{bad}\n",
                        fixture_game("done", 1770000001123)
                    ),
                    false,
                )
            } else if server_limited.load(Ordering::SeqCst) {
                (429, "Retry-After: 1\r\n".into(), String::new(), false)
            } else {
                (200, String::new(), String::new(), false)
            }
        });
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute("INSERT INTO accounts(username,platform,added_at,profile_fetched_at,last_sync_at,sync_status,sync_error,display_name,history_complete,auto_analyze,analyze_history) VALUES('fixture','lichess',0,0,123,'error','Earlier failure','Existing',1,0,0)",&[]).unwrap();
        for id in ["bad", "done", "ongoing"] {
            db.execute("INSERT INTO lichess_pending_games(account_id,game_id,created_at_ms,misses) VALUES(1,?,1770000000000,1)",&[json!(id)]).unwrap();
        }
        let before = db.one("SELECT * FROM accounts", &[]).unwrap().unwrap();
        let service = local_service(db.clone(), &server.base);
        service.sync_account(&before).unwrap();
        assert_eq!(
            db.one("SELECT * FROM accounts", &[]).unwrap().unwrap(),
            before,
            "429 must not publish profile, success timestamp, idle status, cursor or last-new-game metadata"
        );
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(), 1);
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM account_games", &[])
                .unwrap(),
            1
        );
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM lichess_pending_games", &[])
                .unwrap(),
            2
        );
        assert!(
            db.scalar("SELECT checked_at FROM lichess_pending_games", &[])
                .unwrap()
                .as_i64()
                .unwrap()
                > 0
        );
        assert_eq!(
            db.scalar("SELECT misses FROM lichess_pending_games", &[])
                .unwrap(),
            0
        );
        service.sync_tick().unwrap();
        assert_eq!(
            server.requests.lock().unwrap().len(),
            3,
            "cooldown must not issue another request"
        );
        assert_eq!(
            db.one("SELECT * FROM accounts", &[]).unwrap().unwrap(),
            before
        );
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM events", &[]).unwrap(),
            0,
            "rate-limited slices publish only imported games, memberships and pending bookkeeping"
        );
        limited.store(false, Ordering::SeqCst);
        *service.lichess_client.serial.lock().unwrap() =
            Some(Instant::now() - Duration::from_secs(1));
        db.execute("UPDATE lichess_pending_games SET checked_at=0", &[])
            .unwrap();
        service.sync_tick().unwrap();
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM lichess_pending_games", &[])
                .unwrap(),
            1
        );
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM events WHERE level='warning'", &[])
                .unwrap(),
            1,
            "the deferred mapping diagnostic must not be lost"
        );
        assert_eq!(
            db.scalar("SELECT sync_status FROM accounts", &[]).unwrap(),
            "idle"
        );
    }

    #[test]
    fn retry_backoff_is_fenced_by_account_generation_range_and_cursor() {
        let client = LichessClient::fixture(Url::parse("http://127.0.0.1:9").unwrap());
        let account =
            json!({"sync_generation":"first","import_since":null,"lichess_cursor_ms":123});
        let first = RetryScope::from_account(&account);
        client.defer_no_progress(1, &first);
        assert!(client.retry_blocked(1, &first));
        for next in [
            json!({"sync_generation":"second","import_since":null,"lichess_cursor_ms":123}),
            json!({"sync_generation":"first","import_since":"2025/01","lichess_cursor_ms":null}),
            json!({"sync_generation":"first","import_since":null,"lichess_cursor_ms":456}),
        ] {
            client.defer_no_progress(1, &first);
            assert!(!client.retry_blocked(1, &RetryScope::from_account(&next)));
        }
    }

    #[test]
    fn interrupted_profile_is_reported_as_a_gateway_failure() {
        let server = FixtureServer::start(|_| {
            (
                200,
                "X-Fixture-Truncate: yes\r\n".into(),
                "{\"username\":".into(),
                false,
            )
        });
        let client = LichessClient::fixture(server.base.clone());
        let error = client
            .profile("fixture", "", &AtomicBool::new(false))
            .unwrap_err();
        assert_eq!(error.downcast_ref::<crate::ApiError>().unwrap().0, 502);
        assert!(
            error
                .to_string()
                .contains("profile transport was interrupted")
        );
    }
}
