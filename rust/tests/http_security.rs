use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use plyback::{
    config::Config,
    http::{App, router},
    security,
};
use serde_json::{Value, json};
use std::sync::Arc;
use tower::ServiceExt;

struct TestApp {
    app: Arc<App>,
    rt: tokio::runtime::Runtime,
    dir: tempfile::TempDir,
}
impl TestApp {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let config = Config {
            data: dir.path().join("data"),
            frontend: dir.path().join("web"),
            host: "127.0.0.1".into(),
            port: 0,
            allowed_hosts: vec![],
            secure_cookies: false,
            allow_unauthenticated_remote: false,
            background: false,
        };
        std::fs::create_dir_all(&config.frontend).unwrap();
        std::fs::write(
            config.frontend.join("index.html"),
            "<!doctype html><title>Plyback</title>",
        )
        .unwrap();
        let app = App::open(config).unwrap();
        let rt = tokio::runtime::Runtime::new().unwrap();
        Self { app, rt, dir }
    }
    fn request(
        &self,
        method: &str,
        path: &str,
        body: Value,
        extra: &[(&str, &str)],
    ) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
        let mut request = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "localhost")
            .header("x-requested-with", "plyback");
        for &(key, value) in extra {
            request = request.header(key, value);
        }
        let request = request
            .body(Body::from(serde_json::to_vec(&body).unwrap()))
            .unwrap();
        self.rt.block_on(async {
            let response = router(self.app.clone()).oneshot(request).await.unwrap();
            let (parts, body) = response.into_parts();
            (
                parts.status,
                parts.headers,
                to_bytes(body, 32 * 1024 * 1024).await.unwrap().to_vec(),
            )
        })
    }
}
impl Drop for TestApp {
    fn drop(&mut self) {
        self.app.stop();
    }
}

#[test]
fn shutdown_during_body_upload_cannot_start_late_work() {
    use std::io::{BufRead, BufReader, Write};
    let test = TestApp::new();
    let listener = test
        .rt
        .block_on(async { tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap() });
    let address = listener.local_addr().unwrap();
    let app = test.app.clone();
    let server = test.rt.spawn(async move {
        axum::serve(listener, router(app)).await.unwrap();
    });
    let mut socket = std::net::TcpStream::connect(address).unwrap();
    socket
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let body = r#"{"username":"alice"}"#;
    write!(socket, "POST /api/accounts HTTP/1.1\r\nHost: localhost\r\nX-Requested-With: plyback\r\nContent-Type: application/json\r\nContent-Length: {}\r\nExpect: 100-continue\r\nConnection: close\r\n\r\n", body.len()).unwrap();
    let mut reader = BufReader::new(socket.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    assert!(line.contains("100 Continue"), "{line}");
    // Hyper sends Continue only when the handler polls the body after preflight.
    loop {
        line.clear();
        assert!(reader.read_line(&mut line).unwrap() > 0);
        if line == "\r\n" {
            break;
        }
    }
    test.app.request_stop();
    socket.write_all(body.as_bytes()).unwrap();
    line.clear();
    reader.read_line(&mut line).unwrap();
    assert!(line.contains("503"), "{line}");
    server.abort();
    assert_eq!(
        test.app
            .db
            .scalar("SELECT COUNT(*) FROM accounts", &[])
            .unwrap(),
        0
    );
}

#[test]
fn shutdown_rejects_new_requests_before_engine_or_network_work() {
    let test = TestApp::new();
    test.app.request_stop();
    assert_eq!(test.request("GET", "/api/health", json!({}), &[]).0, 503);
    assert_eq!(
        test.request("POST", "/api/accounts", json!({"username":"alice"}), &[])
            .0,
        503
    );
    assert_eq!(
        test.request("POST", "/api/engine/analyse", json!({"fen":"invalid"}), &[])
            .0,
        503
    );
}

#[test]
fn authentication_survives_existing_hash_and_revokes_sessions() {
    let test = TestApp::new();
    security::set_password(&test.app.db, Some("test-password")).unwrap();
    assert_eq!(test.request("GET", "/api/health", json!({}), &[]).0, 200);
    assert_eq!(test.request("GET", "/api/accounts", json!({}), &[]).0, 401);
    let (status, headers, _) = test.request(
        "POST",
        "/api/auth/login",
        json!({"password":"test-password"}),
        &[],
    );
    assert_eq!(status, 200);
    let cookie = headers["set-cookie"].to_str().unwrap();
    assert!(cookie.contains("HttpOnly"));
    assert!(cookie.contains("SameSite=Strict"));
    let cookie = cookie.split(';').next().unwrap();
    assert_eq!(
        test.request("GET", "/api/accounts", json!({}), &[("cookie", cookie)])
            .0,
        200
    );
    assert_eq!(
        test.request(
            "POST",
            "/api/auth/password",
            json!({"current":"wrong","password":"new-password"}),
            &[("cookie", cookie)]
        )
        .0,
        403
    );
    assert_eq!(
        test.request(
            "POST",
            "/api/auth/password",
            json!({"current":"test-password","password":"new-password"}),
            &[("cookie", cookie)]
        )
        .0,
        200
    );
    assert_eq!(
        test.request("GET", "/api/accounts", json!({}), &[("cookie", cookie)])
            .0,
        401
    );
}

#[test]
fn rejects_csrf_bad_origin_and_oversize_before_parsing() {
    let test = TestApp::new();
    for origin in [
        "http://evil.invalid",
        "https://localhost",
        "http://localhost:9999",
        "null",
    ] {
        let (status, headers, _) = test.request(
            "POST",
            "/api/auth/login",
            json!({"password":"anything"}),
            &[("origin", origin)],
        );
        assert_eq!(status, 403);
        assert_eq!(headers["x-frame-options"], "DENY");
        assert_eq!(headers["cache-control"], "no-store");
    }
    assert_eq!(
        test.request(
            "POST",
            "/api/games/import",
            json!({}),
            &[("content-length", "999999999")]
        )
        .0,
        413
    );
    let response = test
        .rt
        .block_on(
            router(test.app.clone()).oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/login")
                    .header("host", "localhost")
                    .body(Body::from("{}"))
                    .unwrap(),
            ),
        )
        .unwrap();
    assert_eq!(response.status(), 403);
    let response = test
        .rt
        .block_on(
            router(test.app.clone()).oneshot(
                Request::builder()
                    .uri("/api/health")
                    .header("host", "evil.invalid")
                    .body(Body::empty())
                    .unwrap(),
            ),
        )
        .unwrap();
    assert_eq!(response.status(), 400);
}

#[test]
fn validates_inputs_and_prevents_static_path_escape() {
    let test = TestApp::new();
    for body in [
        json!({"fen":"x","depth":0}),
        json!({"fen":"x","multipv":100}),
        json!({"fen":"x","depth":true}),
    ] {
        assert_eq!(
            test.request("POST", "/api/engine/analyse", body, &[]).0,
            422
        );
    }
    assert_eq!(
        test.request("PUT", "/api/settings", json!({"workers":999999}), &[])
            .0,
        400
    );
    assert_eq!(
        test.request("GET", "/api/events?limit=100000", json!({}), &[])
            .0,
        422
    );
    assert_eq!(
        test.request("GET", "/api/no-such-endpoint", json!({}), &[])
            .0,
        404
    );
    std::fs::write(test.dir.path().join("secret"), "DO NOT SERVE").unwrap();
    for path in ["/%2e%2e/secret", "/%2f..%2fsecret", "/assets/no-such.js"] {
        let (status, _, body) = test.request("GET", path, json!({}), &[]);
        assert!(status.is_client_error());
        assert!(!String::from_utf8_lossy(&body).contains("DO NOT SERVE"));
    }
    assert_eq!(test.request("GET", "/games", json!({}), &[]).0, 200);
}

#[test]
fn login_rate_limit_is_per_client_and_hides_internal_data() {
    let test = TestApp::new();
    security::set_password(&test.app.db, Some("test-password")).unwrap();
    for _ in 0..10 {
        assert_eq!(
            test.request("POST", "/api/auth/login", json!({"password":123}), &[])
                .0,
            422
        );
    }
    let (status, headers, body) = test.request(
        "POST",
        "/api/auth/login",
        json!({"password":"test-password"}),
        &[],
    );
    assert_eq!(status, 429);
    assert_eq!(headers["retry-after"], "60");
    assert!(!String::from_utf8_lossy(&body).contains("test-password"));
}
