use plyback::{engine::Engine, pgn::STARTING_FEN};
use std::{
    path::PathBuf,
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn fixture(mode: &str) -> (tempfile::TempDir, PathBuf) {
    static BINARY: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
    let (_, compiled) = BINARY.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let binary = dir.path().join(if cfg!(windows) {
            "fixture.exe"
        } else {
            "fixture"
        });
        let source =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/uci_fixture.rs");
        assert!(
            std::process::Command::new("rustc")
                .args(["--edition=2024"])
                .arg(source)
                .arg("-o")
                .arg(&binary)
                .status()
                .unwrap()
                .success()
        );
        (dir, binary)
    });
    let dir = tempfile::tempdir().unwrap();
    let path = dir
        .path()
        .join(format!("{mode}{}", if cfg!(windows) { ".exe" } else { "" }));
    std::fs::copy(compiled, &path).unwrap();
    (dir, path)
}

#[test]
fn partial_iterations_are_not_mixed_and_process_is_reused() {
    let (_dir, path) = fixture("partial");
    let mut engine = Engine::with_path(path.clone(), 1, 16).unwrap();
    let cancel = AtomicBool::new(false);
    for _ in 0..2 {
        let result = engine.analyse(STARTING_FEN, 12, 2, 1.0, &cancel).unwrap();
        assert_eq!(result[0]["depth"], 8);
        assert_eq!(result[1]["depth"], 8);
        assert_eq!(result[0]["cp"], 20);
    }
    drop(engine);
    let log = std::fs::read_to_string(path.with_extension("log")).unwrap();
    assert_eq!(log.lines().filter(|s| s.starts_with("start ")).count(), 1);
    assert_eq!(log.lines().filter(|s| s.starts_with("go ")).count(), 2);
}

#[test]
fn cancellation_has_a_bounded_deadline_even_when_stop_is_ignored() {
    for mode in ["hang", "stubborn"] {
        let (_dir, path) = fixture(mode);
        let mut engine = Engine::with_path(path, 1, 16).unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let setter = cancel.clone();
        let trigger = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            setter.store(true, Ordering::SeqCst);
        });
        let started = Instant::now();
        let error = engine
            .analyse(STARTING_FEN, 32, 2, 60.0, &cancel)
            .unwrap_err();
        trigger.join().unwrap();
        assert_eq!(error.downcast_ref::<plyback::ApiError>().unwrap().0, 499);
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}

#[test]
fn engine_exit_retries_once_and_reports_unavailable() {
    let (_dir, path) = fixture("exit");
    let mut engine = Engine::with_path(path.clone(), 1, 16).unwrap();
    let error = engine
        .analyse(STARTING_FEN, 12, 2, 1.0, &AtomicBool::new(false))
        .unwrap_err();
    assert_eq!(error.downcast_ref::<plyback::ApiError>().unwrap().0, 503);
    drop(engine);
    let log = std::fs::read_to_string(path.with_extension("log")).unwrap();
    assert_eq!(log.lines().filter(|s| s.starts_with("start ")).count(), 2);
}

fn searches(path: &std::path::Path) -> usize {
    std::fs::read_to_string(path.with_extension("log"))
        .unwrap_or_default()
        .lines()
        .filter(|s| s.starts_with("go "))
        .count()
}

/// Hold the shared connection on another thread until `release` is sent.
fn hold_connection(
    db: &Arc<plyback::db::Database>,
) -> (std::sync::mpsc::Sender<()>, std::thread::JoinHandle<()>) {
    let holder = db.clone();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel::<()>();
    let thread = std::thread::spawn(move || {
        holder
            .transaction(|_| {
                ready_tx.send(()).unwrap();
                release_rx.recv().unwrap();
                Ok(())
            })
            .unwrap()
    });
    ready_rx.recv().unwrap();
    (release_tx, thread)
}

#[test]
fn brief_database_contention_does_not_drop_cache_entries() {
    let (dir, path) = fixture("complete");
    let mut engine = Engine::with_path(path.clone(), 1, 16).unwrap();
    let db = Arc::new(plyback::db::Database::open(dir.path().join("cache.db")).unwrap());
    engine.attach_database(db.clone());
    let cancel = AtomicBool::new(false);
    // Another worker holds the connection for longer than one SQLite lock
    // attempt; the cache write waits for it instead of being skipped.
    let (release, thread) = hold_connection(&db);
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        release.send(()).unwrap();
    });
    let first = engine.analyse(STARTING_FEN, 12, 2, 1.0, &cancel).unwrap();
    releaser.join().unwrap();
    thread.join().unwrap();
    assert_eq!(searches(&path), 1);
    // The same contention on the read side is a delayed hit, not a re-search.
    let (release, thread) = hold_connection(&db);
    let releaser = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        release.send(()).unwrap();
    });
    let warm = engine.analyse(STARTING_FEN, 12, 2, 1.0, &cancel).unwrap();
    releaser.join().unwrap();
    thread.join().unwrap();
    assert_eq!(warm, first);
    assert_eq!(searches(&path), 1);
}

#[test]
fn cache_waits_are_bounded_and_cancellable() {
    let (dir, path) = fixture("complete");
    let mut engine = Engine::with_path(path, 1, 16).unwrap();
    let db = Arc::new(plyback::db::Database::open(dir.path().join("cache.db")).unwrap());
    engine.attach_database(db.clone());
    // A connection held indefinitely costs at most the bounded wait on the
    // read and on the write; the search result is still returned.
    let (release, thread) = hold_connection(&db);
    let start = Instant::now();
    let result = engine.analyse(STARTING_FEN, 12, 2, 1.0, &AtomicBool::new(false));
    let elapsed = start.elapsed();
    release.send(()).unwrap();
    thread.join().unwrap();
    assert!(result.is_ok());
    assert!(elapsed < Duration::from_secs(6), "{elapsed:?}");
    // An external SQLite writer is bounded the same way.
    let external = rusqlite::Connection::open(&db.path).unwrap();
    external.execute_batch("BEGIN IMMEDIATE").unwrap();
    let start = Instant::now();
    let result = engine.analyse(STARTING_FEN, 13, 2, 1.0, &AtomicBool::new(false));
    let elapsed = start.elapsed();
    external.execute_batch("ROLLBACK").unwrap();
    assert!(result.is_ok());
    assert!(elapsed < Duration::from_secs(6), "{elapsed:?}");
    // Cancellation does not wait behind a held connection.
    let (release, thread) = hold_connection(&db);
    let cancel = Arc::new(AtomicBool::new(false));
    let setter = cancel.clone();
    let trigger = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        setter.store(true, Ordering::SeqCst);
    });
    let start = Instant::now();
    let error = engine
        .analyse(STARTING_FEN, 14, 2, 1.0, &cancel)
        .unwrap_err();
    let elapsed = start.elapsed();
    trigger.join().unwrap();
    release.send(()).unwrap();
    thread.join().unwrap();
    assert_eq!(error.downcast_ref::<plyback::ApiError>().unwrap().0, 499);
    assert!(elapsed < Duration::from_millis(500), "{elapsed:?}");
}

#[test]
fn disconnected_http_request_stops_the_native_engine() {
    use std::io::{Read, Write};
    use std::net::{Shutdown, TcpListener, TcpStream};
    use std::process::{Child, Command, Stdio};
    struct Server(Child);
    impl Drop for Server {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let (dir, path) = fixture("stubborn");
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let mut command = Command::new(env!("CARGO_BIN_EXE_plyback"));
    command
        .args([
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--no-background",
        ])
        .arg("--data")
        .arg(dir.path().join("data"))
        .env("PLYBACK_STOCKFISH", &path)
        .env_remove("PLYBACK_PASSWORD")
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let mut server = Server(command.spawn().unwrap());
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(mut stream) = TcpStream::connect(("127.0.0.1", port)) {
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            stream.write_all(format!("GET /api/health HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n").as_bytes()).unwrap();
            let mut response = String::new();
            stream.read_to_string(&mut response).unwrap();
            if response.contains("200 OK") {
                break;
            }
        }
        assert!(server.0.try_wait().unwrap().is_none(), "server exited");
        assert!(Instant::now() < deadline, "server did not start");
        std::thread::sleep(Duration::from_millis(20));
    }
    let body = serde_json::json!({"fen":STARTING_FEN,"depth":32,"multipv":2}).to_string();
    let mut request = TcpStream::connect(("127.0.0.1", port)).unwrap();
    write!(request,"POST /api/engine/analyse HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nX-Requested-With: plyback\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{body}",body.len()).unwrap();
    request.flush().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let log = std::fs::read_to_string(path.with_extension("log")).unwrap_or_default();
        if log.lines().any(|line| line.starts_with("go ")) {
            break;
        }
        assert!(Instant::now() < deadline, "engine search did not begin");
        std::thread::sleep(Duration::from_millis(10));
    }
    request.shutdown(Shutdown::Both).unwrap();
    drop(request);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let log = std::fs::read_to_string(path.with_extension("log")).unwrap();
        if log.lines().any(|line| line == "stop") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "disconnect did not cancel the engine"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    // Give the bounded forced-reap path time to finish before killing the server.
    std::thread::sleep(Duration::from_millis(700));
}
