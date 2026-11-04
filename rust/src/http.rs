//! HTTP adapter. Blocking SQLite, KDF and UCI work is kept off the async reactor;
//! a fixed permit budget bounds work even when clients disconnect.
use crate::{
    ANALYSIS_VERSION, APP_VERSION, ApiError, ApiResult, Query, api_error,
    config::{Config, MAX_BODY_BYTES},
    db::Database,
    engine::Engine,
    insights::Insights,
    security,
    services::{self, Services},
};
use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{ConnectInfo, State},
    http::{HeaderMap, HeaderValue, Request, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::{
    net::SocketAddr,
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
    interactive: Mutex<Option<Engine>>,
    limiter: security::RateLimiter,
    permits: Arc<Semaphore>,
    requests: Mutex<Vec<Weak<AtomicBool>>>,
    shutting_down: AtomicBool,
}

impl App {
    pub fn open(config: Config) -> anyhow::Result<Arc<Self>> {
        let db = Arc::new(Database::open(config.db_path())?);
        let services = Services::new(db.clone())?;
        let insights = Insights::new(db.clone(), services.clone());
        Ok(Arc::new(Self {
            config,
            db,
            services,
            insights,
            interactive: Mutex::new(None),
            limiter: security::RateLimiter::default(),
            permits: Arc::new(Semaphore::new(16)),
            requests: Mutex::new(Vec::new()),
            shutting_down: AtomicBool::new(false),
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
    }
    pub fn stop(&self) {
        self.request_stop();
        self.insights.stop();
        self.services.stop();
        self.interactive
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
    }
    fn preflight(&self, meta: &Meta) -> anyhow::Result<()> {
        let host =
            security::host_name(&meta.host).ok_or_else(|| api_error(400, "Invalid host."))?;
        let protected = security::password_hash(&self.db)?.is_some();
        if !protected
            && !self
                .config
                .allowed_hosts
                .iter()
                .any(|s| s == "*" || s == &host)
            && !security::is_loopback_host(&host)
        {
            return Err(api_error(
                400,
                "Unknown host. Set PLYBACK_ALLOWED_HOSTS or a password to allow access from other machines.",
            ));
        }
        if meta.path.starts_with("/api/") {
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
            for &(method, prefix, rate) in security::RATE_RULES {
                if meta.method == method
                    && meta.path.starts_with(prefix)
                    && !self.limiter.allow(&meta.client, prefix, rate)
                {
                    return Err(api_error(429, "Too many requests. Try again in a minute."));
                }
            }
        }
        Ok(())
    }

    fn dispatch(
        &self,
        meta: &Meta,
        body: &Value,
        cancel: &AtomicBool,
    ) -> anyhow::Result<(u16, Value, Option<String>)> {
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
        if !matches!(
            meta.path.as_str(),
            "/api/health" | "/api/auth/session" | "/api/auth/login"
        ) && security::password_hash(&self.db)?.is_some()
            && !security::session_valid(&self.db, token)?
        {
            return Err(api_error(401, "Sign in to continue."));
        }
        let secure = self.config.secure_cookies || meta.scheme == "https";
        let ok = |value| Ok((200, value, None));
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
                        json!({"authenticated":true,"password_set":true,"auth_required":true}),
                        Some(security::session_cookie(&token, secure)),
                    ));
                }
                return ok(security::session_state(&self.db, token)?);
            }
            ("POST", "/api/auth/logout") => {
                security::end_session(&self.db, token)?;
                return Ok((
                    200,
                    json!({"ok":true}),
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
                        json!({"authenticated":true,"password_set":true,"auth_required":true}),
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
                    json!({"authenticated":true,"password_set":false,"auth_required":false}),
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
                let limit = query_integer(&meta.query, "limit", 100, 1, 500)?;
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
                        Err(std::sync::TryLockError::Poisoned(err)) => break err.into_inner(),
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
        let result: Option<ApiResult> = self
            .insights
            .handle(method, &meta.path, &meta.query, body)
            .or_else(|| self.services.handle(method, &meta.path, &meta.query, body));
        let (status, value) = result.unwrap_or_else(|| Err(api_error(404, "Not found")))?;
        Ok((status, value, None))
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
    client: String,
}
fn header(headers: &HeaderMap, key: &str) -> String {
    headers
        .get(key)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_owned()
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
    let Ok(permit) = app.permits.clone().try_acquire_owned() else {
        return failure(api_error(503, "Server is busy. Try again shortly."));
    };
    let peer = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|c| c.0);
    let headers = request.headers();
    let scheme = if peer.is_some_and(|p| p.ip().is_loopback())
        && header(headers, "x-forwarded-proto") == "https"
    {
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
        client: peer
            .map(|p| p.ip().to_string())
            .unwrap_or_else(|| "local-test".into()),
    };
    if let Some(length) = headers.get("content-length")
        && length
            .to_str()
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .is_none_or(|n| n > MAX_BODY_BYTES as u64)
    {
        return failure(api_error(413, "Request is too large."));
    }
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
    if !meta.path.starts_with("/api") {
        if !matches!(meta.method.as_str(), "GET" | "HEAD") {
            return failure(api_error(405, "Method not allowed"));
        }
        let path = meta.path.clone();
        let root = app.config.frontend.clone();
        return match tokio::task::spawn_blocking(move || {
            let _permit = permit;
            static_file(&root, &path)
        })
        .await
        {
            Ok(Ok((mime, data, is_index))) => {
                let mut response = (StatusCode::OK, [("content-type", mime)], data).into_response();
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
    let data = match tokio::time::timeout(
        Duration::from_secs(15),
        to_bytes(request.into_body(), MAX_BODY_BYTES),
    )
    .await
    {
        Ok(Ok(data)) => data,
        Ok(Err(_)) => return failure(api_error(413, "Request is too large.")),
        Err(_) => return failure(api_error(408, "Request body timed out.")),
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
        app.dispatch(&meta, &body, &cancel)
    })
    .await
    {
        Ok(Ok((status, value, cookie))) => {
            let mut response = (
                StatusCode::from_u16(status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
                axum::Json(value),
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
    if path.starts_with("/api") {
        headers.insert("cache-control", HeaderValue::from_static("no-store"));
    } else if path.starts_with("/assets/") {
        headers
            .entry("cache-control")
            .or_insert(HeaderValue::from_static(
                "public, max-age=31536000, immutable",
            ));
    }
}

fn optional_string<'a>(
    body: &'a Value,
    key: &str,
    limit: usize,
) -> anyhow::Result<Option<&'a str>> {
    match body.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) if value.chars().count() <= limit => Ok(Some(value)),
        _ => Err(api_error(
            422,
            format!("{key} must be a string of at most {limit} characters"),
        )),
    }
}
fn required_string<'a>(body: &'a Value, key: &str, limit: usize) -> anyhow::Result<&'a str> {
    optional_string(body, key, limit)?.ok_or_else(|| api_error(422, format!("{key} is required")))
}
fn body_integer(body: &Value, key: &str, default: i64, min: i64, max: i64) -> anyhow::Result<i64> {
    let value = match body.get(key) {
        None | Some(Value::Null) => default,
        Some(value) => value
            .as_i64()
            .ok_or_else(|| api_error(422, format!("{key} must be an integer")))?,
    };
    if !(min..=max).contains(&value) {
        return Err(api_error(
            422,
            format!("{key} must be between {min} and {max}"),
        ));
    }
    Ok(value)
}
fn query_integer(
    query: &Query,
    key: &str,
    default: i64,
    min: i64,
    max: i64,
) -> anyhow::Result<i64> {
    let value = query
        .get(key)
        .map(|v| {
            v.parse::<i64>()
                .map_err(|_| api_error(422, format!("Invalid {key}")))
        })
        .transpose()?
        .unwrap_or(default);
    if !(min..=max).contains(&value) {
        return Err(api_error(422, format!("Invalid {key}")));
    }
    Ok(value)
}

fn static_file(root: &Path, encoded: &str) -> anyhow::Result<(&'static str, Vec<u8>, bool)> {
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
    if std::fs::metadata(&file)?.len() > 20 * 1024 * 1024 {
        return Err(api_error(413, "Asset is too large"));
    }
    let mime = match file.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "svg" => "image/svg+xml",
        "json" => "application/json",
        "woff2" => "font/woff2",
        "woff" => "font/woff",
        "ico" => "image/x-icon",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    };
    Ok((mime, std::fs::read(file)?, index))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_positions_bypass_a_busy_engine() {
        let directory = tempfile::tempdir().unwrap();
        let app = App::open(Config {
            data: directory.path().join("data"),
            frontend: directory.path().join("web"),
            host: "127.0.0.1".into(),
            port: 0,
            allowed_hosts: vec![],
            secure_cookies: false,
            allow_unauthenticated_remote: false,
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
