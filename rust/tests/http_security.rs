use axum::{
    body::{Body, to_bytes},
    extract::ConnectInfo,
    http::{Request, StatusCode},
};
use plyback::{
    config::Config,
    http::{App, router},
    security,
};
use serde_json::{Value, json};
use std::{net::SocketAddr, sync::Arc};
use tower::ServiceExt;

struct TestApp {
    app: Arc<App>,
    rt: tokio::runtime::Runtime,
    dir: tempfile::TempDir,
}
impl TestApp {
    fn new() -> Self {
        Self::configured(|_| {})
    }
    fn with_host(host: &str) -> Self {
        Self::configured(|config| config.host = host.into())
    }
    fn configured(configure: impl FnOnce(&mut Config)) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let mut config = Config {
            data: dir.path().join("data"),
            frontend: dir.path().join("web"),
            host: "127.0.0.1".into(),
            port: 0,
            allowed_hosts: vec![],
            trusted_proxies: vec![],
            secure_cookies: false,
            allow_unauthenticated_remote: false,
            background: false,
        };
        configure(&mut config);
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
        self.request_from(
            "127.0.0.1:12345".parse().unwrap(),
            method,
            path,
            body,
            extra,
        )
    }
    fn request_from(
        &self,
        peer: SocketAddr,
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
            request.headers_mut().unwrap().insert(
                axum::http::header::HeaderName::from_bytes(key.as_bytes()).unwrap(),
                value.parse().unwrap(),
            );
        }
        let request = request
            .extension(ConnectInfo(peer))
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

#[test]
fn network_bound_server_refuses_requests_after_its_password_is_removed() {
    let test = TestApp::with_host("0.0.0.0");
    security::set_password(&test.app.db, Some("test-password")).unwrap();
    assert_eq!(test.request("GET", "/api/health", json!({}), &[]).0, 200);
    assert_eq!(test.request("GET", "/api/accounts", json!({}), &[]).0, 401);

    // The CLI can remove the hash through a separate connection to the same database.
    let cli = plyback::db::Database::open(test.app.config.db_path()).unwrap();
    security::set_password(&cli, None).unwrap();
    assert_eq!(test.request("GET", "/api/accounts", json!({}), &[]).0, 503);
    assert_eq!(test.request("GET", "/api/health", json!({}), &[]).0, 503);

    security::set_password(&cli, Some("test-password")).unwrap();
    assert_eq!(test.request("GET", "/api/health", json!({}), &[]).0, 200);
    assert_eq!(test.request("GET", "/api/accounts", json!({}), &[]).0, 401);
}
impl Drop for TestApp {
    fn drop(&mut self) {
        self.app.stop();
    }
}

#[test]
fn host_allowlist_applies_before_login_even_with_a_password() {
    let test = TestApp::configured(|c| c.allowed_hosts.push("chess.example.test".into()));
    security::set_password(&test.app.db, Some("test-password")).unwrap();
    for _ in 0..12 {
        assert_eq!(
            test.request(
                "POST",
                "/api/auth/login",
                json!({"password":123}),
                &[("host", "rebind.example.test")]
            )
            .0,
            400
        );
    }
    for host in [
        "localhost:8000",
        "127.0.0.1",
        "[::1]:8000",
        "CHESS.EXAMPLE.TEST:443",
    ] {
        assert_eq!(
            test.request("GET", "/api/health", json!({}), &[("host", host)])
                .0,
            200,
            "{host}"
        );
        assert_eq!(
            test.request(
                "POST",
                "/api/auth/login",
                json!({"password":123}),
                &[("host", host)]
            )
            .0,
            422,
            "{host}"
        );
    }
    for path in ["/api/health", "/api/accounts", "/"] {
        assert_eq!(
            test.request("GET", path, json!({}), &[("host", "rebind.example.test")])
                .0,
            400
        );
    }
}

#[test]
fn explicit_bind_host_is_allowed_with_and_without_password() {
    for bind in ["192.0.2.15", "2001:db8::15", "my-chess.test"] {
        // Without a password a network bind needs PLYBACK_ALLOW_UNAUTHENTICATED_REMOTE;
        // otherwise every request is refused before the Host check.
        let test = TestApp::configured(|c| {
            c.host = bind.into();
            c.allow_unauthenticated_remote = true;
        });
        for protected in [false, true] {
            if protected {
                security::set_password(&test.app.db, Some("test-password")).unwrap();
            }
            let host = if bind.contains(':') {
                format!("[{bind}]")
            } else {
                bind.into()
            };
            assert_eq!(
                test.request("GET", "/api/health", json!({}), &[("host", &host)])
                    .0,
                200
            );
            assert_eq!(
                test.request(
                    "GET",
                    "/api/health",
                    json!({}),
                    &[("host", "unlisted.test")]
                )
                .0,
                400
            );
        }
    }
}

#[test]
fn protected_wildcard_bind_accepts_machine_ips_and_hostname() {
    for bind in ["0.0.0.0", "::"] {
        // Without a password a network bind needs PLYBACK_ALLOW_UNAUTHENTICATED_REMOTE;
        // otherwise every request is refused before the Host check.
        let test = TestApp::configured(|c| {
            c.host = bind.into();
            c.allow_unauthenticated_remote = true;
        });
        let hostname = hostname::get().unwrap().into_string().unwrap();
        if !security::is_loopback_host(&hostname) {
            assert_eq!(
                test.request("GET", "/api/health", json!({}), &[("host", &hostname)])
                    .0,
                400
            );
        }
        security::set_password(&test.app.db, Some("test-password")).unwrap();
        let mut hosts = vec![hostname];
        hosts.extend(
            if_addrs::get_if_addrs()
                .unwrap()
                .into_iter()
                .map(|i| match i.ip() {
                    std::net::IpAddr::V4(ip) => ip.to_string(),
                    std::net::IpAddr::V6(ip) => format!("[{ip}]"),
                }),
        );
        for host in hosts {
            assert_eq!(
                test.request("GET", "/api/health", json!({}), &[("host", &host)])
                    .0,
                200,
                "{bind}: {host}"
            );
        }
        assert_eq!(
            test.request(
                "GET",
                "/api/health",
                json!({}),
                &[("host", "attacker.test")]
            )
            .0,
            400
        );
    }
}

#[test]
fn passwordless_tailscale_serve_keeps_full_api_access() {
    let test = TestApp::configured(|c| c.allowed_hosts.push("plyback.tailnet.example".into()));
    let headers = [
        ("host", "plyback.tailnet.example"),
        ("origin", "https://plyback.tailnet.example"),
        ("x-forwarded-proto", "https"),
        ("x-forwarded-host", "plyback.tailnet.example"),
        ("x-forwarded-for", "100.101.102.103"),
        ("forwarded", "for=100.101.102.103;proto=https"),
        ("x-real-ip", "100.101.102.103"),
    ];
    assert_eq!(test.request("GET", "/", json!({}), &headers).0, 200);
    assert_eq!(
        test.request("GET", "/api/accounts", json!({}), &headers).0,
        200
    );
    assert_eq!(
        test.request(
            "PUT",
            "/api/settings",
            json!({"analysis_depth":12}),
            &headers
        )
        .0,
        200
    );
    let (_, _, body) = test.request("GET", "/api/auth/session", json!({}), &headers);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["password_set"],
        false
    );
}

#[test]
fn only_explicit_trusted_proxies_get_separate_client_buckets() {
    for header in ["x-forwarded-for", "forwarded"] {
        let test = TestApp::configured(|c| c.trusted_proxies.push("127.0.0.1".parse().unwrap()));
        security::set_password(&test.app.db, Some("test-password")).unwrap();
        let forwarded = |ip: &str| {
            if header == "forwarded" {
                format!("for={ip}")
            } else {
                ip.to_owned()
            }
        };
        let first = forwarded("192.0.2.1");
        let second = forwarded("192.0.2.2");
        for _ in 0..10 {
            assert_eq!(
                test.request(
                    "POST",
                    "/api/auth/login",
                    json!({"password":123}),
                    &[(header, &first)]
                )
                .0,
                422
            );
        }
        assert_eq!(
            test.request(
                "POST",
                "/api/auth/login",
                json!({"password":123}),
                &[(header, &first)]
            )
            .0,
            429
        );
        assert_eq!(
            test.request(
                "POST",
                "/api/auth/login",
                json!({"password":123}),
                &[(header, &second)]
            )
            .0,
            422
        );
    }
    // A loopback peer is not implicitly trusted; configuring another proxy does
    // not grant trust to every peer either.
    for trusted in [vec![], vec!["192.0.2.99".parse().unwrap()]] {
        let test = TestApp::configured(|c| c.trusted_proxies = trusted);
        for i in 0..11 {
            let ip = format!("192.0.2.{}", i + 1);
            let forwarded = format!("for={ip}");
            assert_eq!(
                test.request(
                    "POST",
                    "/api/auth/login",
                    json!({"password":123}),
                    &[("x-forwarded-for", &ip), ("forwarded", &forwarded)]
                )
                .0,
                if i < 10 { 422 } else { 429 }
            );
        }
    }
}

#[test]
fn spoofed_chain_prefix_and_malformed_headers_cannot_reset_login_budget() {
    let test = TestApp::configured(|c| c.trusted_proxies.push("127.0.0.1".parse().unwrap()));
    for i in 0..11 {
        let chain = format!("198.51.100.{}, 192.0.2.1", i + 1);
        assert_eq!(
            test.request(
                "POST",
                "/api/auth/login",
                json!({"password":123}),
                &[("x-forwarded-for", &chain)]
            )
            .0,
            if i < 10 { 422 } else { 429 }
        );
    }
    for i in 0..11 {
        let invalid = format!("unknown-{}", i + 1);
        let alternate = format!("for=198.51.100.{}", i + 1);
        assert_eq!(
            test.request(
                "POST",
                "/api/auth/login",
                json!({"password":123}),
                &[("x-forwarded-for", &invalid), ("forwarded", &alternate)]
            )
            .0,
            if i < 10 { 422 } else { 429 }
        );
    }
}

#[test]
fn install_manifest_and_icons_load_without_signing_in() {
    // Browsers fetch the manifest without the session cookie, so a password must not hide it.
    let test = TestApp::new();
    security::set_password(&test.app.db, Some("test-password")).unwrap();
    let root = &test.app.config.frontend;
    std::fs::write(root.join("manifest.webmanifest"), r#"{"name":"Plyback"}"#).unwrap();
    std::fs::write(root.join("icon-192.png"), b"PNG").unwrap();
    let (status, headers, body) = test.request("GET", "/manifest.webmanifest", json!({}), &[]);
    assert_eq!(status, 200);
    assert_eq!(headers["content-type"], "application/manifest+json");
    assert_eq!(body, br#"{"name":"Plyback"}"#);
    let (status, headers, _) = test.request("GET", "/icon-192.png", json!({}), &[]);
    assert_eq!(status, 200);
    assert_eq!(headers["content-type"], "image/png");
    let (status, _, _) = test.request("GET", "/api/accounts", json!({}), &[]);
    assert_eq!(status, 401);
}

#[test]
fn static_compression_preserves_headers_and_never_compresses_api_responses() {
    let test = TestApp::new();
    let assets = test.app.config.frontend.join("assets");
    std::fs::create_dir(&assets).unwrap();
    std::fs::write(assets.join("app.js"), "identity").unwrap();
    std::fs::write(assets.join("app.js.br"), "brotli").unwrap();
    let (status, headers, body) = test.request(
        "GET",
        "/assets/app.js",
        json!({}),
        &[("accept-encoding", "br")],
    );
    assert_eq!(status, 200);
    assert_eq!(body, b"brotli");
    assert_eq!(headers["content-encoding"], "br");
    assert_eq!(headers["vary"], "Accept-Encoding");
    assert_eq!(headers["content-type"], "text/javascript; charset=utf-8");
    assert_eq!(headers["content-security-policy"], security::CSP);
    assert_eq!(
        headers["cache-control"],
        "public, max-age=31536000, immutable"
    );
    let (_, headers, body) = test.request(
        "HEAD",
        "/assets/app.js",
        json!({}),
        &[("accept-encoding", "br")],
    );
    assert!(body.is_empty());
    assert_eq!(headers["content-encoding"], "br");
    let (_, headers, _) = test.request(
        "GET",
        "/api/health",
        json!({}),
        &[("accept-encoding", "br")],
    );
    assert!(!headers.contains_key("content-encoding"));
    assert_eq!(headers["cache-control"], "no-store");
    let (_, headers, _) = test.request("GET", "/games", json!({}), &[("accept-encoding", "br")]);
    assert_eq!(headers["cache-control"], "no-cache");
    let (status, headers, _) = test.request(
        "GET",
        "/games",
        json!({}),
        &[("accept-encoding", "identity;q=0")],
    );
    assert_eq!(status, 406);
    assert_eq!(headers["content-security-policy"], security::CSP);
    for (path, expected) in [("/assets/app.js", 406), ("/assets/missing.js", 404)] {
        let (status, headers, _) = test.request(
            "GET",
            path,
            json!({}),
            &[("accept-encoding", "br;q=0,gzip;q=0,identity;q=0")],
        );
        assert_eq!(status, expected);
        assert_eq!(headers["cache-control"], "no-store");
        assert_eq!(headers["vary"], "Accept-Encoding");
    }
}

#[test]
fn status_revision_observes_changes_even_when_counts_do_not_change() {
    let test = TestApp::new();
    test.app
        .db
        .execute(
            "INSERT INTO games(uuid,pgn,imported_at) VALUES ('revision-test','1. e4 *',0)",
            &[],
        )
        .unwrap();
    let read = || {
        let (status, _, body) = test.request("GET", "/api/status", json!({}), &[]);
        assert_eq!(status, 200);
        serde_json::from_slice::<Value>(&body).unwrap()
    };
    let before = read();
    test.app
        .db
        .execute("UPDATE games SET opening_name='Updated opening'", &[])
        .unwrap();
    let after = read();
    assert_eq!(before["games"], after["games"]);
    assert_eq!(before["analyzed"], after["analyzed"]);
    assert!(after["data_revision"].as_u64().unwrap() > before["data_revision"].as_u64().unwrap());
    assert_eq!(after["data_revision"], read()["data_revision"]);
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
fn the_security_checks_and_the_router_agree_on_what_is_the_api() {
    let test = TestApp::new();
    security::set_password(&test.app.db, Some("test-password")).unwrap();
    // "/api" itself is the API, so it needs a session like every API path.
    assert_eq!(test.request("GET", "/api", json!({}), &[]).0, 401);
    // "/apix" is not the API: it is never dispatched to API handlers unchecked.
    assert_eq!(test.request("POST", "/apix", json!({}), &[]).0, 405);
    let (_, headers, _) = test.request("GET", "/apix", json!({}), &[]);
    assert!(
        !headers
            .get("content-type")
            .is_some_and(|v| v.to_str().unwrap_or("").contains("json")),
        "served as a page, not an API answer"
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
