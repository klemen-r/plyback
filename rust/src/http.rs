//! HTTP adapter. Blocking SQLite, KDF and UCI work is kept off the async reactor;
//! a fixed permit budget bounds work even when clients disconnect.
use crate::{
    ANALYSIS_VERSION, APP_VERSION, ApiError, ApiResult, Query, api_error,
    config::{Config, MAX_BODY_BYTES},
    db::Database,
    engine::Engine,
    insights::Insights,
    params::{RangeError, body_integer, optional_string, query_integer, required_string},
    security,
    services::{self, Services},
};
use axum::{
    Router,
    body::{Body, HttpBody, to_bytes},
    extract::{ConnectInfo, State},
    http::{HeaderMap, HeaderValue, Request, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    path::{Component, Path},
    sync::{
        Arc, Mutex, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::Semaphore;

pub struct App {
    pub config: Config,
    pub db: Arc<Database>,
    pub services: Arc<Services>,
    pub insights: Arc<Insights>,
    repertoire: crate::repertoire::Repertoire,
    interactive: Mutex<Option<Engine>>,
    limiter: security::RateLimiter,
    permits: Arc<Semaphore>,
    /// Request bodies being received, per client. Uploads hold no work permit, so a slow or
    /// stalled body cannot starve other requests; these caps bound what they can hold instead.
    uploads: Mutex<HashMap<String, usize>>,
    requests: Mutex<Vec<Weak<AtomicBool>>>,
    shutting_down: AtomicBool,
    local_hosts: Vec<String>,
}

/// Concurrent request bodies one client may be sending, and all clients together.
const UPLOADS_PER_CLIENT: usize = 4;
const UPLOADS_TOTAL: usize = 32;

/// One upload in progress, released when the body has been read or the request is dropped.
struct Upload<'a> {
    uploads: &'a Mutex<HashMap<String, usize>>,
    client: String,
}

impl<'a> Upload<'a> {
    fn start(uploads: &'a Mutex<HashMap<String, usize>>, client: &str) -> Option<Self> {
        let mut active = uploads.lock().unwrap_or_else(|p| p.into_inner());
        if active.values().sum::<usize>() >= UPLOADS_TOTAL
            || active.get(client).copied().unwrap_or(0) >= UPLOADS_PER_CLIENT
        {
            return None;
        }
        *active.entry(client.to_owned()).or_insert(0) += 1;
        Some(Self {
            uploads,
            client: client.to_owned(),
        })
    }
}

impl Drop for Upload<'_> {
    fn drop(&mut self) {
        let mut active = self.uploads.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(count) = active.get_mut(&self.client) {
            *count -= 1;
            if *count == 0 {
                active.remove(&self.client);
            }
        }
    }
}

impl App {
    pub fn open(config: Config) -> anyhow::Result<Arc<Self>> {
        let local_hosts = local_hosts(&config.host)?;
        let db = Arc::new(Database::open(config.db_path())?);
        let services = Services::new(db.clone())?;
        let insights = Insights::new(db.clone(), services.clone());
        Ok(Arc::new(Self {
            config,
            repertoire: crate::repertoire::Repertoire::new(db.clone()),
            db,
            services,
            insights,
            interactive: Mutex::new(None),
            limiter: security::RateLimiter::default(),
            permits: Arc::new(Semaphore::new(16)),
            uploads: Mutex::new(HashMap::new()),
            requests: Mutex::new(Vec::new()),
            shutting_down: AtomicBool::new(false),
            local_hosts,
        }))
    }
    pub fn start(self: &Arc<Self>) {
        if self.config.background {
            self.services.start();
            self.insights.start();
        }
    }
    pub fn cancel_requests(&self) {
        for request in self
            .requests
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .filter_map(Weak::upgrade)
        {
            request.store(true, Ordering::Release);
        }
    }
    pub fn request_stop(&self) {
        self.shutting_down.store(true, Ordering::Release);
        self.cancel_requests();
        self.services.request_stop();
        self.insights.request_stop();
        self.repertoire.request_stop();
    }
    pub fn stop(&self) {
        self.request_stop();
        self.insights.stop();
        self.repertoire.stop();
        self.services.stop();
        self.interactive
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
    }
    fn password_protected(&self) -> anyhow::Result<bool> {
        let protected = security::password_hash(&self.db)?.is_some();
        if !protected && self.config.requires_password() {
            return Err(api_error(
                503,
                "Network access requires a password. Set one with plyback set-password.",
            ));
        }
        Ok(protected)
    }

    fn check_forwarded_access(&self, meta: &Meta, protected: bool) -> anyhow::Result<()> {
        if !protected && meta.forwarded {
            let host =
                security::host_name(&meta.host).ok_or_else(|| api_error(400, "Invalid host."))?;
            if !self
                .config
                .allowed_hosts
                .iter()
                .any(|allowed| allowed != "*" && allowed == &host)
            {
                return Err(api_error(
                    403,
                    "Forwarded access requires a password. Set one with plyback set-password, or explicitly list the request Host in PLYBACK_ALLOWED_HOSTS.",
                ));
            }
        }
        Ok(())
    }

    fn preflight(&self, meta: &Meta) -> anyhow::Result<()> {
        let host =
            security::host_name(&meta.host).ok_or_else(|| api_error(400, "Invalid host."))?;
        let protected = self.password_protected()?;
        if !self
            .config
            .allowed_hosts
            .iter()
            .any(|s| s == "*" || s == &host)
            && !security::is_loopback_host(&host)
            && bind_host(&self.config.host).as_ref() != Some(&host)
            && !(protected && self.local_hosts.contains(&host))
        {
            return Err(api_error(
                400,
                "Unknown host. Add this name to PLYBACK_ALLOWED_HOSTS to allow access.",
            ));
        }
        self.check_forwarded_access(meta, protected)?;
        if is_api_path(&meta.path) {
            if !matches!(meta.method.as_str(), "GET" | "HEAD" | "OPTIONS") {
                if !meta.csrf.eq_ignore_ascii_case("plyback") {
                    return Err(api_error(403, "Missing request header."));
                }
                if let Some(origin) = &meta.origin {
                    let actual = security::origin(origin);
                    if actual.is_none()
                        || actual != security::origin(&format!("{}://{}", meta.scheme, meta.host))
                    {
                        return Err(api_error(403, "Cross-origin request refused."));
                    }
                }
            }
            if protected
                && !matches!(
                    meta.path.as_str(),
                    "/api/health" | "/api/auth/session" | "/api/auth/login"
                )
                && !security::session_valid(&self.db, security::cookie_token(&meta.cookies))?
            {
                return Err(api_error(401, "Sign in to continue."));
            }
            if let Some((rule, rate)) = security::rate_rule(&meta.method, &meta.path)
                && !self.limiter.allow(&meta.client, rule, rate)
            {
                return Err(api_error(429, "Too many requests. Try again in a minute."));
            }
        }
        Ok(())
    }

    fn dispatch_body(
        &self,
        meta: &Meta,
        body: &Value,
        cancel: &AtomicBool,
    ) -> anyhow::Result<(u16, ResponseData, Option<String>)> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(api_error(503, "Server is shutting down."));
        }
        let method = if meta.method == "HEAD" {
            "GET"
        } else {
            &meta.method
        };
        let token = security::cookie_token(&meta.cookies);
        // Authentication may have changed while an upload was arriving. Check
        // immediately before dispatch, without charging the rate limit twice.
        let protected = self.password_protected()?;
        self.check_forwarded_access(meta, protected)?;
        if !matches!(
            meta.path.as_str(),
            "/api/health" | "/api/auth/session" | "/api/auth/login"
        ) && protected
            && !security::session_valid(&self.db, token)?
        {
            return Err(api_error(401, "Sign in to continue."));
        }
        let secure = self.config.secure_cookies || meta.scheme == "https";
        let ok = |value| Ok((200, ResponseData::Json(value), None));
        match (method, meta.path.as_str()) {
            ("GET", "/api/health") => return ok(json!({"ok":true,"version":APP_VERSION})),
            ("GET", "/api/openapi.json") => {
                return ok(serde_json::from_str(include_str!("../openapi.json"))?);
            }
            ("GET", "/api/auth/session") => return ok(security::session_state(&self.db, token)?),
            ("POST", "/api/auth/login") => {
                let password = required_string(body, "password", 256)?;
                if let Some(stored) = security::password_hash(&self.db)? {
                    if !security::verify_password(password, &stored) {
                        return Err(api_error(401, "That password is not correct."));
                    }
                    let token =
                        security::create_session_checked(&self.db, &meta.user_agent, &stored)?;
                    return Ok((
                        200,
                        ResponseData::Json(
                            json!({"authenticated":true,"password_set":true,"auth_required":true}),
                        ),
                        Some(security::session_cookie(&token, secure)),
                    ));
                }
                return ok(security::session_state(&self.db, token)?);
            }
            ("POST", "/api/auth/logout") => {
                security::end_session(&self.db, token)?;
                return Ok((
                    200,
                    ResponseData::Json(json!({"ok":true})),
                    Some(security::clear_cookie(secure)),
                ));
            }
            ("POST", "/api/auth/password") => {
                let current = optional_string(body, "current", 256)?;
                let stored = security::password_hash(&self.db)?;
                if let Some(stored) = &stored
                    && !security::verify_password(current.unwrap_or_default(), stored)
                {
                    return Err(api_error(403, "The current password is not correct."));
                }
                let password = optional_string(body, "password", 256)?;
                if let Some(password) = password {
                    security::validate_password(password)?;
                    let token = security::rotate_password(
                        &self.db,
                        stored.as_deref(),
                        Some(password),
                        &meta.user_agent,
                    )?
                    .expect("setting a password creates a session");
                    return Ok((
                        200,
                        ResponseData::Json(
                            json!({"authenticated":true,"password_set":true,"auth_required":true}),
                        ),
                        Some(security::session_cookie(&token, secure)),
                    ));
                }
                if !security::is_loopback_host(&self.config.host) {
                    return Err(api_error(
                        400,
                        "Stop network access and restart with a loopback bind before removing the password.",
                    ));
                }
                security::rotate_password(&self.db, stored.as_deref(), None, &meta.user_agent)?;
                return Ok((
                    200,
                    ResponseData::Json(
                        json!({"authenticated":true,"password_set":false,"auth_required":false}),
                    ),
                    Some(security::clear_cookie(secure)),
                ));
            }
            ("GET", "/api/settings") => return ok(services::get_settings(&self.db)?),
            ("PUT", "/api/settings") => {
                let settings = services::save_settings(&self.db, body)?;
                if ["workers", "threads_per_worker", "hash_mb"]
                    .iter()
                    .any(|k| body.get(k).is_some())
                {
                    self.services.apply_settings();
                }
                return ok(settings);
            }
            ("GET", "/api/status") => {
                return ok(json!({
                    "version":APP_VERSION,"analysis_version":ANALYSIS_VERSION,
                    "data_revision":self.db.scalar("SELECT revision FROM data_revision WHERE id=1",&[])?,
                    "engine":self.services.engine_status(),"queue":self.services.stats(),"sync":self.services.sync_status(),
                    "accounts":self.db.scalar("SELECT COUNT(*) FROM accounts",&[])?,
                    "games":self.db.scalar("SELECT COUNT(*) FROM games",&[])?,
                    "analyzed":self.db.scalar("SELECT COUNT(*) FROM game_analysis",&[])?,
                    "db_size":std::fs::metadata(&self.db.path).ok().map(|m|m.len()),
                    "depth_presets":{"fast":12,"standard":16,"deep":20},
                    "cpu_count":std::thread::available_parallelism().map(usize::from).unwrap_or(2)
                }));
            }
            ("GET", "/api/events") => {
                let limit = query_integer(&meta.query, "limit", 100, 1, 500, RangeError::Invalid)?;
                let rows = if let Some(account) = meta.query.get("account_id") {
                    let id = account
                        .parse::<i64>()
                        .map_err(|_| api_error(422, "Invalid account_id"))?;
                    self.db.query(
                        "SELECT * FROM events WHERE account_id=? ORDER BY id DESC LIMIT ?",
                        &[json!(id), json!(limit)],
                    )?
                } else {
                    self.db.query(
                        "SELECT * FROM events ORDER BY id DESC LIMIT ?",
                        &[json!(limit)],
                    )?
                };
                return ok(json!(rows));
            }
            ("POST", "/api/engine/analyse") => {
                let fen = required_string(body, "fen", 256)?;
                let settings = services::get_settings(&self.db)?;
                let depth = body_integer(
                    body,
                    "depth",
                    settings["interactive_depth"].as_i64().unwrap_or(20),
                    6,
                    32,
                )? as u32;
                let multipv = body_integer(body, "multipv", 3, 1, 5)? as u32;
                let board =
                    crate::pgn::board(fen, false).or_else(|_| crate::pgn::board(fen, true))?;
                if let Some(cp) = crate::engine::terminal_eval(&board, 1) {
                    let terminal = json!({"cp":cp,"mate":(cp.abs() >= crate::engine::MATE_THRESHOLD).then_some(0),"pv":[],"depth":0,"nodes":0});
                    let lines = crate::analysis::interactive_lines(fen, &[terminal])?;
                    return ok(json!({"fen":fen,"lines":lines}));
                }
                let mut engine = loop {
                    if cancel.load(Ordering::Acquire) {
                        return Err(api_error(499, "Analysis request was cancelled"));
                    }
                    match self.interactive.try_lock() {
                        Ok(lock) => break lock,
                        Err(std::sync::TryLockError::Poisoned(err)) => {
                            let mut lock = err.into_inner();
                            // A panic can interrupt UCI framing; start a fresh engine.
                            lock.take();
                            self.interactive.clear_poison();
                            break lock;
                        }
                        Err(std::sync::TryLockError::WouldBlock) => {
                            std::thread::sleep(Duration::from_millis(10))
                        }
                    }
                };
                if engine.is_none() {
                    let mut created = Engine::new(2, 128)?;
                    created.attach_database(self.db.clone());
                    *engine = Some(created);
                }
                let result = engine
                    .as_mut()
                    .unwrap()
                    .analyse(fen, depth, multipv, 4.0, cancel);
                if result.as_ref().err().is_some_and(|error| {
                    error
                        .downcast_ref::<ApiError>()
                        .is_none_or(|error| error.0 == 503)
                }) {
                    engine.take();
                }
                let lines = crate::analysis::interactive_lines(fen, &result?)?;
                return ok(json!({"fen":fen,"lines":lines}));
            }
            _ => {}
        }
        if let Some(result) = self.repertoire.handle(method, &meta.path, &meta.query) {
            return Ok((200, ResponseData::Serialized(result?), None));
        }
        let result: Option<ApiResult> = self
            .insights
            .handle(method, &meta.path, &meta.query, body)
            .or_else(|| crate::practice::handle(&self.db, method, &meta.path, &meta.query, body))
            .or_else(|| self.services.handle(method, &meta.path, &meta.query, body));
        let (status, value) = result.unwrap_or_else(|| Err(api_error(404, "Not found")))?;
        Ok((status, ResponseData::Json(value), None))
    }
    #[cfg(test)]
    fn dispatch(
        &self,
        meta: &Meta,
        body: &Value,
        cancel: &AtomicBool,
    ) -> anyhow::Result<(u16, Value, Option<String>)> {
        let (status, data, cookie) = self.dispatch_body(meta, body, cancel)?;
        let value = match data {
            ResponseData::Json(value) => value,
            ResponseData::Serialized(bytes) => serde_json::from_slice(&bytes)?,
        };
        Ok((status, value, cookie))
    }
}

enum ResponseData {
    Json(Value),
    Serialized(Arc<[u8]>),
}
impl IntoResponse for ResponseData {
    fn into_response(self) -> Response {
        match self {
            Self::Json(value) => axum::Json(value).into_response(),
            Self::Serialized(bytes) => (
                [("content-type", "application/json")],
                axum::body::Bytes::from_owner(bytes),
            )
                .into_response(),
        }
    }
}

#[derive(Clone)]
struct Meta {
    method: String,
    path: String,
    query: Query,
    host: String,
    scheme: String,
    origin: Option<String>,
    csrf: String,
    cookies: String,
    user_agent: String,
    forwarded: bool,
    client: String,
}
fn header(headers: &HeaderMap, key: &str) -> String {
    headers
        .get(key)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned()
}

fn bind_host(bind: &str) -> Option<String> {
    if let Ok(IpAddr::V6(ip)) = bind.parse() {
        security::host_name(&format!("[{ip}]"))
    } else {
        security::host_name(bind)
    }
}

// Discover OS-owned names once, after CLI bind overrides. Never resolve an
// incoming Host: an attacker's DNS name can resolve to one of our interfaces.
fn local_hosts(bind: &str) -> anyhow::Result<Vec<String>> {
    if !bind.parse::<IpAddr>().is_ok_and(|ip| ip.is_unspecified()) {
        return Ok(Vec::new());
    }
    let mut hosts = Vec::new();
    for interface in if_addrs::get_if_addrs()? {
        let ip = interface.ip();
        if ip.is_unspecified() {
            continue;
        }
        let name = match ip {
            IpAddr::V4(_) => ip.to_string(),
            IpAddr::V6(_) => format!("[{ip}]"),
        };
        if let Some(host) = security::host_name(&name) {
            hosts.push(host);
        }
    }
    if let Some(name) = hostname::get()?.to_str()
        && let Some(host) = security::host_name(name)
    {
        hosts.push(host);
    }
    hosts.sort();
    hosts.dedup();
    Ok(hosts)
}

fn client_ip(peer: IpAddr, headers: &HeaderMap, trusted: &[IpAddr]) -> IpAddr {
    let peer = peer.to_canonical();
    let is_trusted = |ip| trusted.iter().any(|p| p.to_canonical() == ip);
    if !is_trusted(peer) {
        return peer;
    }
    // XFF takes precedence. Never fall through an invalid preferred header to
    // another potentially attacker-controlled source. Invalid chains use the
    // peer bucket, preserving the limiter rather than manufacturing identities.
    let hops = if headers.contains_key("x-forwarded-for") {
        forwarded_hops(headers, "x-forwarded-for", false)
    } else {
        forwarded_hops(headers, "forwarded", true)
    };
    let Some(hops) = hops else { return peer };
    let mut client = peer;
    for hop in hops.into_iter().rev() {
        if !is_trusted(client) {
            break;
        }
        client = hop;
    }
    client
}

fn forwarded_hops(headers: &HeaderMap, name: &str, standard: bool) -> Option<Vec<IpAddr>> {
    let mut hops = Vec::new();
    // Preserve wire order across repeated fields, as well as comma lists.
    for value in headers.get_all(name) {
        for element in split_forwarded(value.to_str().ok()?, ',')? {
            if hops.len() >= 64 {
                return None;
            }
            let ip = if standard {
                let mut address = None;
                for parameter in split_forwarded(element, ';')? {
                    let (key, value) = parameter.trim().split_once('=')?;
                    if key.eq_ignore_ascii_case("for") {
                        if address.is_some() {
                            return None;
                        }
                        let value = value.trim();
                        let value = if value.starts_with('"') {
                            value.strip_prefix('"')?.strip_suffix('"')?
                        } else {
                            value
                        };
                        address = Some(
                            value
                                .parse::<IpAddr>()
                                .ok()
                                .or_else(|| value.parse::<SocketAddr>().ok().map(|s| s.ip()))
                                .or_else(|| {
                                    value.strip_prefix('[')?.strip_suffix(']')?.parse().ok()
                                })?,
                        );
                    }
                }
                address?
            } else {
                element.trim().parse::<IpAddr>().ok()?
            };
            hops.push(ip.to_canonical());
        }
    }
    if hops.is_empty() { None } else { Some(hops) }
}

// Quoted metadata may contain delimiters: splitting it naively could manufacture
// a forged `for` hop. Escaped quoted strings are unsupported and fail closed.
fn split_forwarded(value: &str, delimiter: char) -> Option<Vec<&str>> {
    let mut quoted = false;
    let mut start = 0;
    let mut parts = Vec::new();
    for (offset, c) in value.char_indices() {
        match c {
            '\\' => return None,
            '"' => quoted = !quoted,
            c if c == delimiter && !quoted => {
                parts.push(&value[start..offset]);
                start = offset + c.len_utf8();
            }
            _ => {}
        }
    }
    if quoted {
        return None;
    }
    parts.push(&value[start..]);
    Some(parts)
}

pub fn router(app: Arc<App>) -> Router {
    Router::new().fallback(handle).with_state(app)
}

struct CancelOnDrop(Arc<AtomicBool>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

async fn handle(State(app): State<Arc<App>>, request: Request<Body>) -> Response {
    let path = request.uri().path().to_owned();
    let is_head = request.method() == "HEAD";
    let mut response = handle_inner(app, request).await;
    secure_headers(&mut response, &path);
    if is_head {
        *response.body_mut() = Body::empty();
    }
    response
}

async fn handle_inner(app: Arc<App>, request: Request<Body>) -> Response {
    if app.shutting_down.load(Ordering::Acquire) {
        return failure(api_error(503, "Server is shutting down."));
    }
    let headers = request.headers();
    let declared_length = headers
        .get("content-length")
        .map(|length| length.to_str().ok().and_then(|v| v.parse::<u64>().ok()));
    if declared_length.is_some_and(|n| n.is_none_or(|n| n > MAX_BODY_BYTES as u64)) {
        return failure(api_error(413, "Request is too large."));
    }
    let declares_body = headers.contains_key("transfer-encoding")
        || declared_length.is_some_and(|n| n.is_some_and(|n| n > 0));
    if declares_body
        && matches!(
            request.method().as_str(),
            "GET" | "HEAD" | "OPTIONS" | "DELETE"
        )
    {
        return failure(api_error(400, "This request takes no body."));
    }
    let Ok(permit) = app.permits.clone().try_acquire_owned() else {
        return failure(api_error(503, "Server is busy. Try again shortly."));
    };
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    // Only a local or configured proxy may say the browser connected over HTTPS.
    let from_proxy = peer.is_some_and(|p| {
        let ip = p.ip().to_canonical();
        ip.is_loopback()
            || app
                .config
                .trusted_proxies
                .iter()
                .any(|proxy| proxy.to_canonical() == ip)
    });
    let scheme = if from_proxy && header(headers, "x-forwarded-proto") == "https" {
        "https"
    } else {
        "http"
    };
    let uri = request.uri();
    if uri.path().len() > 4096 || uri.query().is_some_and(|q| q.len() > 16384) {
        return failure(api_error(414, "Request URL is too long."));
    }
    let meta = Meta {
        method: request.method().to_string(),
        path: uri.path().trim_end_matches('/').to_owned(),
        query: url::form_urlencoded::parse(uri.query().unwrap_or("").as_bytes())
            .into_owned()
            .collect(),
        host: header(headers, "host"),
        scheme: scheme.into(),
        origin: headers.get("origin").map(|_| header(headers, "origin")),
        csrf: header(headers, "x-requested-with"),
        cookies: header(headers, "cookie"),
        user_agent: header(headers, "user-agent"),
        forwarded: [
            "forwarded",
            "x-forwarded-for",
            "x-forwarded-host",
            "x-forwarded-proto",
            "x-real-ip",
        ]
        .iter()
        .any(|name| headers.contains_key(*name)),
        client: peer
            .map(|p| client_ip(p.ip(), headers, &app.config.trusted_proxies).to_string())
            .unwrap_or_else(|| "local-test".into()),
    };
    let (state, metadata) = (app.clone(), meta.clone());
    let permit =
        match tokio::task::spawn_blocking(move || (state.preflight(&metadata), permit)).await {
            Ok((Ok(()), permit)) => permit,
            Ok((Err(err), _)) => return failure(err),
            Err(err) => return failure(err.into()),
        };
    if meta.path == "/api/docs"
        && meta.method == "GET"
        && std::env::var("PLYBACK_API_DOCS").as_deref() == Ok("1")
    {
        return (
            StatusCode::TEMPORARY_REDIRECT,
            [("location", "/api/openapi.json")],
        )
            .into_response();
    }
    if !is_api_path(&meta.path) {
        if !matches!(meta.method.as_str(), "GET" | "HEAD") {
            return failure(api_error(405, "Method not allowed"));
        }
        let path = meta.path.clone();
        let root = app.config.frontend.clone();
        let encoding = header(request.headers(), "accept-encoding");
        return match tokio::task::spawn_blocking(move || {
            let _permit = permit;
            static_file(&root, &path, &encoding)
        })
        .await
        {
            Ok(Ok((mime, data, is_index, encoding))) => {
                let mut response = (StatusCode::OK, [("content-type", mime)], data).into_response();
                response
                    .headers_mut()
                    .insert("vary", HeaderValue::from_static("Accept-Encoding"));
                if let Some(encoding) = encoding {
                    response
                        .headers_mut()
                        .insert("content-encoding", HeaderValue::from_static(encoding));
                }
                if is_index {
                    response
                        .headers_mut()
                        .insert("cache-control", HeaderValue::from_static("no-cache"));
                }
                response
            }
            Ok(Err(err)) => failure(err),
            Err(err) => failure(err.into()),
        };
    }
    // Receive a body without holding a work permit: a client that announces a body and then
    // sends it slowly (or never) must not be able to occupy every permit.
    // Ask the body itself: HTTP/2 can send one without a length header.
    let (data, permit) = if !request.body().is_end_stream() {
        drop(permit);
        let data = {
            let Some(_upload) = Upload::start(&app.uploads, &meta.client) else {
                return failure(api_error(
                    429,
                    "Too many uploads in progress. Try again shortly.",
                ));
            };
            match tokio::time::timeout(
                Duration::from_secs(15),
                to_bytes(request.into_body(), MAX_BODY_BYTES),
            )
            .await
            {
                Ok(Ok(data)) => data,
                Ok(Err(_)) => return failure(api_error(413, "Request is too large.")),
                Err(_) => return failure(api_error(408, "Request body timed out.")),
            }
        };
        let Ok(permit) = app.permits.clone().try_acquire_owned() else {
            return failure(api_error(503, "Server is busy. Try again shortly."));
        };
        (data, permit)
    } else {
        (Default::default(), permit)
    };
    let body = if data.is_empty() {
        json!({})
    } else {
        match serde_json::from_slice::<Value>(&data) {
            Ok(value) if value.is_object() => value,
            _ => return failure(api_error(422, "Expected a JSON object.")),
        }
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let _cancel_guard = CancelOnDrop(cancel.clone());
    {
        let mut requests = app.requests.lock().unwrap_or_else(|p| p.into_inner());
        // Serialize registration with cancellation so an upload finishing during
        // shutdown cannot start a new, uncancelled engine request.
        if app.shutting_down.load(Ordering::Acquire) {
            return failure(api_error(503, "Server is shutting down."));
        }
        requests.retain(|p| p.strong_count() > 0);
        requests.push(Arc::downgrade(&cancel));
    }
    match tokio::task::spawn_blocking(move || {
        let _permit = permit;
        app.dispatch_body(&meta, &body, &cancel)
    })
    .await
    {
        Ok(Ok((status, value, cookie))) => {
            let mut response = (
                StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                value,
            )
                .into_response();
            if let Some(cookie) = cookie.and_then(|c| HeaderValue::from_str(&c).ok()) {
                response.headers_mut().insert("set-cookie", cookie);
            }
            response
        }
        Ok(Err(err)) => failure(err),
        Err(err) => failure(err.into()),
    }
}

fn failure(error: anyhow::Error) -> Response {
    let (status, detail) = if let Some(ApiError(status, message)) = error.downcast_ref::<ApiError>()
    {
        (*status, message.as_str())
    } else {
        tracing::error!(error=%error,"Request failed");
        (500, "An internal error occurred.")
    };
    let mut response = (
        StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
        axum::Json(json!({"detail":detail})),
    )
        .into_response();
    if matches!(status, 429 | 503) {
        response
            .headers_mut()
            .insert("retry-after", HeaderValue::from_static("60"));
    }
    response
}

fn secure_headers(response: &mut Response, path: &str) {
    let success = response.status().is_success();
    let headers = response.headers_mut();
    for (name, value) in [
        ("content-security-policy", security::CSP),
        ("x-content-type-options", "nosniff"),
        ("x-frame-options", "DENY"),
        ("referrer-policy", "strict-origin-when-cross-origin"),
        (
            "permissions-policy",
            "camera=(), microphone=(), geolocation=(), payment=()",
        ),
        ("cross-origin-opener-policy", "same-origin"),
        ("cross-origin-resource-policy", "same-origin"),
    ] {
        headers.insert(name, HeaderValue::from_static(value));
    }
    if !is_api_path(path) {
        headers.insert("vary", HeaderValue::from_static("Accept-Encoding"));
    }
    // Never pin a transient error (including an encoding-specific 406) in a
    // browser or intermediary cache under an otherwise immutable asset URL.
    if is_api_path(path) || !success {
        headers.insert("cache-control", HeaderValue::from_static("no-store"));
    } else if path.starts_with("/assets/") {
        headers
            .entry("cache-control")
            .or_insert(HeaderValue::from_static(
                "public, max-age=31536000, immutable",
            ));
    }
}

type StaticFile = (&'static str, Vec<u8>, bool, Option<&'static str>);

// Only public, prebuilt assets are compressed. API responses (including anything
// involving credentials) retain their existing no-store, uncompressed behavior.
fn accepted_encodings(header: &str) -> (Vec<&'static str>, bool) {
    let mut br = None;
    let mut gzip = None;
    let mut identity = None;
    let mut wildcard = None;
    for item in header.split(',') {
        let mut parts = item.split(';');
        let name = parts.next().unwrap_or_default().trim();
        let quality = parts
            .find_map(|part| {
                let (key, value) = part.trim().split_once('=')?;
                key.trim().eq_ignore_ascii_case("q").then(|| {
                    value
                        .trim()
                        .parse::<f32>()
                        .ok()
                        .filter(|q| (0.0..=1.0).contains(q))
                        .unwrap_or(0.0)
                })
            })
            .unwrap_or(1.0);
        if name.eq_ignore_ascii_case("br") {
            br = Some(quality);
        } else if name.eq_ignore_ascii_case("gzip") {
            gzip = Some(quality);
        } else if name.eq_ignore_ascii_case("identity") {
            identity = Some(quality);
        } else if name == "*" {
            wildcard = Some(quality);
        }
    }
    let mut options = [
        ("br", br.or(wildcard).unwrap_or(0.0)),
        ("gzip", gzip.or(wildcard).unwrap_or(0.0)),
        // Honor an explicit preference for identity. Otherwise it remains the
        // fallback when none of the client's accepted compressed files exists.
        ("identity", identity.unwrap_or(0.0)),
    ];
    options.sort_by(|a, b| b.1.total_cmp(&a.1));
    let compressed = options
        .into_iter()
        .filter(|(_, q)| *q > 0.0)
        .map(|(name, _)| name)
        .collect();
    let identity = identity.unwrap_or(if wildcard == Some(0.0) { 0.0 } else { 1.0 }) > 0.0;
    (compressed, identity)
}

fn static_file(root: &Path, encoded: &str, accept_encoding: &str) -> anyhow::Result<StaticFile> {
    let mut decoded = Vec::new();
    let bytes = encoded.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 2 >= bytes.len() {
                return Err(api_error(400, "Invalid path"));
            }
            let hex = std::str::from_utf8(&bytes[i + 1..i + 3])
                .map_err(|_| api_error(400, "Invalid path"))?;
            decoded.push(u8::from_str_radix(hex, 16).map_err(|_| api_error(400, "Invalid path"))?);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    let decoded = std::str::from_utf8(&decoded).map_err(|_| api_error(400, "Invalid path"))?;
    if decoded.contains(['\\', '\0']) {
        return Err(api_error(400, "Invalid path"));
    }
    let relative = Path::new(decoded.trim_start_matches('/'));
    if relative
        .components()
        .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return Err(api_error(400, "Invalid path"));
    }
    let root = root
        .canonicalize()
        .map_err(|_| api_error(404, "Frontend is not built. Run setup first."))?;
    let candidate = root.join(relative);
    let (file, index) = if candidate.is_file() {
        (candidate, false)
    } else if decoded.starts_with("/assets/") {
        return Err(api_error(404, "Not found"));
    } else {
        (root.join("index.html"), true)
    };
    let file = file
        .canonicalize()
        .map_err(|_| api_error(404, "Not found"))?;
    if !file.starts_with(&root) {
        return Err(api_error(404, "Not found"));
    }
    let metadata = std::fs::metadata(&file)?;
    if metadata.len() > 20 * 1024 * 1024 {
        return Err(api_error(413, "Asset is too large"));
    }
    let mime = match file.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "json" => "application/json",
        "webmanifest" => "application/manifest+json",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ico" => "image/x-icon",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    };
    let (encodings, identity) = accepted_encodings(accept_encoding);
    for encoding in encodings {
        if encoding == "identity" {
            return Ok((mime, std::fs::read(file)?, index, None));
        }
        let mut sibling = file.as_os_str().to_os_string();
        sibling.push(if encoding == "br" { ".br" } else { ".gz" });
        // Apply the same canonical containment check to compressed siblings.
        // An absent, stale, oversized or escaped variant falls back to identity.
        if let Ok(compressed) = Path::new(&sibling).canonicalize()
            && compressed.starts_with(&root)
            && let Ok(compressed_metadata) = std::fs::metadata(&compressed)
            && compressed_metadata.is_file()
            && compressed_metadata.len() <= 20 * 1024 * 1024
            && compressed_metadata
                .modified()
                .ok()
                .zip(metadata.modified().ok())
                .is_some_and(|(variant, original)| variant >= original)
        {
            return Ok((mime, std::fs::read(compressed)?, index, Some(encoding)));
        }
    }
    if !identity {
        return Err(api_error(406, "No acceptable asset encoding is available."));
    }
    Ok((mime, std::fs::read(file)?, index, None))
}

/// Whether a request goes to the API. The security checks and the router must agree: a
/// path such as "/apix" that only the router treated as API skipped CSRF and sign-in checks.
fn is_api_path(path: &str) -> bool {
    path == "/api" || path.starts_with("/api/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelled_interactive_request_neither_waits_nor_starts_engine() {
        let directory = tempfile::tempdir().unwrap();
        let app = App::open(Config {
            data: directory.path().join("data"),
            frontend: directory.path().join("web"),
            host: "127.0.0.1".into(),
            port: 0,
            allowed_hosts: vec![],
            trusted_proxies: vec![],
            secure_cookies: false,
            background: false,
        })
        .unwrap();
        // Include an unlocked, already-cancelled request to detect accidental
        // engine creation as well as both cancellation paths behind a busy lock.
        for (hold_lock, already_cancelled) in [(false, true), (true, true), (true, false)] {
            let busy = hold_lock.then(|| app.interactive.lock().unwrap());
            let cancel = Arc::new(AtomicBool::new(already_cancelled));
            let worker_cancel = Arc::clone(&cancel);
            let worker_app = Arc::clone(&app);
            let (sender, receiver) = std::sync::mpsc::channel();
            let worker = std::thread::spawn(move || {
                let meta = Meta {
                    method: "POST".into(),
                    path: "/api/engine/analyse".into(),
                    query: Query::new(),
                    host: "localhost".into(),
                    scheme: "http".into(),
                    origin: None,
                    csrf: "plyback".into(),
                    cookies: String::new(),
                    user_agent: String::new(),
                    forwarded: false,
                    client: "local-test".into(),
                };
                sender
                    .send(worker_app.dispatch(
                        &meta,
                        &json!({"fen": crate::pgn::STARTING_FEN}),
                        &worker_cancel,
                    ))
                    .unwrap();
            });
            if !already_cancelled {
                std::thread::sleep(Duration::from_millis(50));
                cancel.store(true, Ordering::Release);
            }
            // Generous on a loaded machine: a request blocked on the held lock still
            // times out, because the lock is only released after this wait.
            let result = receiver.recv_timeout(Duration::from_secs(5));
            // Release before joining even on failure, so a locking regression
            // reports a failed assertion instead of leaving the test hung.
            drop(busy);
            worker.join().unwrap();
            let error = result
                .expect("cancelled request waited for the engine lock")
                .unwrap_err();
            assert_eq!(error.downcast_ref::<ApiError>().unwrap().0, 499);
            assert!(app.interactive.lock().unwrap().is_none());
        }
        app.stop();
    }

    #[test]
    fn network_password_is_rechecked_after_preflight() {
        let directory = tempfile::tempdir().unwrap();
        let app = App::open(Config {
            data: directory.path().join("db"),
            frontend: directory.path().join("web"),
            host: "0.0.0.0".into(),
            port: 0,
            allowed_hosts: vec![],
            trusted_proxies: vec![],
            secure_cookies: false,
            background: false,
        })
        .unwrap();
        security::set_password(&app.db, Some("test-password")).unwrap();
        let token = security::create_session(&app.db, "test").unwrap();
        let meta = Meta {
            method: "PUT".into(),
            path: "/api/settings".into(),
            query: Query::new(),
            host: "localhost".into(),
            scheme: "http".into(),
            origin: None,
            csrf: "plyback".into(),
            cookies: security::session_cookie(&token, false),
            user_agent: "test".into(),
            forwarded: false,
            client: "local-test".into(),
        };
        app.preflight(&meta).unwrap();
        security::set_password(&app.db, None).unwrap();
        let err = app
            .dispatch(
                &meta,
                &json!({"auto_analyze_new": false}),
                &AtomicBool::new(false),
            )
            .unwrap_err();
        assert_eq!(err.downcast_ref::<ApiError>().unwrap().0, 503);
        assert_eq!(
            services::get_settings(&app.db).unwrap()["auto_analyze_new"],
            true
        );
    }

    #[test]
    fn forwarded_access_is_rechecked_after_password_removal() {
        for explicitly_allowed in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let app = App::open(Config {
                data: directory.path().join("db"),
                frontend: directory.path().join("web"),
                host: "127.0.0.1".into(),
                port: 0,
                allowed_hosts: if explicitly_allowed {
                    vec!["localhost".into()]
                } else {
                    vec![]
                },
                trusted_proxies: vec![],
                secure_cookies: false,
                background: false,
            })
            .unwrap();
            security::set_password(&app.db, Some("test-password")).unwrap();
            let token = security::create_session(&app.db, "test").unwrap();
            let meta = Meta {
                method: "PUT".into(),
                path: "/api/settings".into(),
                query: Query::new(),
                host: "localhost".into(),
                scheme: "http".into(),
                origin: None,
                csrf: "plyback".into(),
                cookies: security::session_cookie(&token, false),
                user_agent: "test".into(),
                forwarded: true,
                client: "local-test".into(),
            };
            app.preflight(&meta).unwrap();
            security::set_password(&app.db, None).unwrap();
            let result = app.dispatch(
                &meta,
                &json!({"auto_analyze_new": false}),
                &AtomicBool::new(false),
            );
            if explicitly_allowed {
                assert_eq!(result.unwrap().0, 200);
            } else {
                let error = result.unwrap_err();
                assert_eq!(error.downcast_ref::<ApiError>().unwrap().0, 403);
                assert!(error.to_string().contains("password"));
            }
            assert_eq!(
                services::get_settings(&app.db).unwrap()["auto_analyze_new"],
                !explicitly_allowed
            );
        }
    }

    #[test]
    fn proxy_client_ip_validates_chains_and_header_precedence() {
        let trusted: Vec<IpAddr> = ["127.0.0.1", "::1", "192.0.2.99"]
            .map(|s| s.parse().unwrap())
            .into();
        let peer = "127.0.0.1".parse().unwrap();
        let check = |name, value: &str, expected: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(name, value.parse().unwrap());
            assert_eq!(
                client_ip(peer, &headers, &trusted),
                expected.parse::<IpAddr>().unwrap(),
                "{value}"
            );
        };
        check(
            "x-forwarded-for",
            "198.51.100.7, 192.0.2.99",
            "198.51.100.7",
        );
        check("x-forwarded-for", "198.51.100.7, 192.0.2.8", "192.0.2.8");
        check(
            "x-forwarded-for",
            "::ffff:198.51.100.7, ::1",
            "198.51.100.7",
        );
        check(
            "forwarded",
            "for=198.51.100.7;proto=https, for=192.0.2.99",
            "198.51.100.7",
        );
        check(
            "forwarded",
            "For=\"[2001:db8::7]:1234\";host=chess.test",
            "2001:db8::7",
        );
        check("forwarded", "for=\"[2001:db8::7]\"", "2001:db8::7");
        check("forwarded", "for=\"198.51.100.7:1234\"", "198.51.100.7");
        check(
            "forwarded",
            "for=198.51.100.7;ext=\"a,for=192.0.2.7;ext=b\"",
            "198.51.100.7",
        );
        check(
            "forwarded",
            "for=198.51.100.7;ext=\"a;for=192.0.2.7\"",
            "198.51.100.7",
        );
        for invalid in [
            "",
            "unknown",
            "198.51.100.7,",
            "bad, 192.0.2.99",
            "198.51.100.7:1234",
        ] {
            check("x-forwarded-for", invalid, "127.0.0.1");
        }
        for invalid in [
            "",
            "for=unknown",
            "for=_hidden",
            "by=192.0.2.99",
            "for=198.51.100.7;for=192.0.2.99",
            "for=\"[::1]",
            "for=198.51.100.7,",
            "for=198.51.100.7;broken",
        ] {
            check("forwarded", invalid, "127.0.0.1");
        }
        let mut headers = HeaderMap::new();
        headers.append("x-forwarded-for", "198.51.100.7".parse().unwrap());
        headers.append("x-forwarded-for", "192.0.2.99".parse().unwrap());
        assert_eq!(
            client_ip(peer, &headers, &trusted),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
        assert_eq!(
            client_ip("192.0.2.8".parse().unwrap(), &headers, &trusted),
            "192.0.2.8".parse::<IpAddr>().unwrap()
        );
        headers.insert("forwarded", "for=198.51.100.8".parse().unwrap());
        assert_eq!(
            client_ip(peer, &headers, &trusted),
            "198.51.100.7".parse::<IpAddr>().unwrap()
        );
        headers.insert("x-forwarded-for", "bad".parse().unwrap());
        assert_eq!(client_ip(peer, &headers, &trusted), peer);
        headers.remove("x-forwarded-for");
        headers.append("forwarded", "for=192.0.2.99".parse().unwrap());
        assert_eq!(
            client_ip(peer, &headers, &trusted),
            "198.51.100.8".parse::<IpAddr>().unwrap()
        );
        assert_eq!(client_ip(peer, &HeaderMap::new(), &trusted), peer);
        headers.insert(
            "x-forwarded-for",
            vec!["192.0.2.99"; 65].join(",").parse().unwrap(),
        );
        assert_eq!(client_ip(peer, &headers, &trusted), peer);
    }

    #[test]
    fn static_variants_negotiate_without_changing_mime_or_spa_fallback() {
        assert_eq!(accepted_encodings("gzip, br").0, ["br", "gzip"]);
        assert_eq!(accepted_encodings("br;q=0, gzip;Q=0.8, *;q=1").0, ["gzip"]);
        assert_eq!(
            accepted_encodings("br;q=NaN, gzip;q=2").0,
            Vec::<&str>::new()
        );
        assert!(accepted_encodings("").0.is_empty());
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        std::fs::create_dir(root.join("assets")).unwrap();
        std::fs::write(root.join("index.html"), "index").unwrap();
        std::fs::write(root.join("assets/app.js"), "identity").unwrap();
        std::fs::write(root.join("assets/app.js.br"), "brotli").unwrap();
        std::fs::write(root.join("assets/app.js.gz"), "gzip").unwrap();
        let (mime, data, index, encoding) =
            static_file(root, "/assets/app.js", "gzip, br").unwrap();
        assert_eq!(mime, "text/javascript; charset=utf-8");
        assert_eq!(data, b"brotli");
        assert!(!index);
        assert_eq!(encoding, Some("br"));
        assert_eq!(
            static_file(root, "/assets/app.js", "gzip").unwrap().1,
            b"gzip"
        );
        assert_eq!(
            static_file(root, "/assets/app.js", "br;q=0.5,identity;q=1")
                .unwrap()
                .1,
            b"identity"
        );
        assert_eq!(
            static_file(root, "/assets/app.js", "br;q=0,gzip;q=0")
                .unwrap()
                .1,
            b"identity"
        );
        assert_eq!(
            static_file(root, "/games/1", "br").unwrap(),
            ("text/html; charset=utf-8", b"index".to_vec(), true, None)
        );
        assert!(static_file(root, "/assets/missing.js", "br").is_err());
        assert!(static_file(root, "/%2e%2e/secret", "br").is_err());
        assert!(static_file(root, "/", "identity;q=0").is_err());
        assert!(static_file(root, "/", "br,identity;q=0").is_err());
        assert!(static_file(root, "/", "*;q=0").is_err());
        assert!(static_file(root, "/", "*;q=0,identity;q=1").is_ok());
        // A deployment replacing an original without rebuilding its variants must
        // serve the new original rather than stale JavaScript.
        std::fs::File::options()
            .write(true)
            .open(root.join("assets/app.js.br"))
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH))
            .unwrap();
        assert_eq!(
            static_file(root, "/assets/app.js", "br").unwrap().1,
            b"identity"
        );
    }

    #[cfg(unix)]
    #[test]
    fn compressed_variant_cannot_escape_frontend_root() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("web");
        std::fs::create_dir(&root).unwrap();
        std::fs::write(root.join("index.html"), "index").unwrap();
        std::fs::write(directory.path().join("secret"), "private").unwrap();
        std::os::unix::fs::symlink(directory.path().join("secret"), root.join("index.html.br"))
            .unwrap();
        assert_eq!(static_file(&root, "/", "br").unwrap().1, b"index");
    }

    #[test]
    fn terminal_positions_bypass_a_busy_engine() {
        let directory = tempfile::tempdir().unwrap();
        let app = App::open(Config {
            data: directory.path().join("data"),
            frontend: directory.path().join("web"),
            host: "127.0.0.1".into(),
            port: 0,
            allowed_hosts: vec![],
            trusted_proxies: vec![],
            secure_cookies: false,
            background: false,
        })
        .unwrap();
        let busy = app.interactive.lock().unwrap();
        let worker_app = app.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            let meta = Meta {
                method: "POST".into(),
                path: "/api/engine/analyse".into(),
                query: Query::new(),
                host: "localhost".into(),
                scheme: "http".into(),
                origin: None,
                csrf: "plyback".into(),
                cookies: String::new(),
                user_agent: String::new(),
                forwarded: false,
                client: "local-test".into(),
            };
            let results = [
                ("7k/8/6QK/8/8/8/8/8 b - - 0 1", 0),
                ("7k/6Q1/6K1/8/8/8/8/8 b - - 0 1", 100_000),
                ("7k/8/6K1/8/8/8/8/8 w - - 0 1", 0),
            ]
            .map(|(fen, expected)| {
                let (_, value, _) = worker_app
                    .dispatch(&meta, &json!({"fen":fen}), &AtomicBool::new(false))
                    .unwrap();
                assert_eq!(value["lines"][0]["cp"], expected);
                assert_eq!(value["lines"][0]["depth"], 0);
                assert_eq!(value["lines"][0]["uci"], json!([]));
                value
            });
            sender.send(results).unwrap();
        });
        let result = receiver.recv_timeout(Duration::from_secs(2));
        drop(busy);
        let finished = worker.join();
        assert!(
            result.is_ok(),
            "terminal response waited for the engine lock"
        );
        finished.unwrap();
        assert!(app.interactive.lock().unwrap().is_none());
        app.stop();
    }
}
