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

#[test]
fn cache_separates_engine_identity_time_budget_and_halfmove() {
    use plyback::engine::Search;
    let (dir, path) = fixture("complete");
    let db = Arc::new(plyback::db::Database::open(dir.path().join("cache.db")).unwrap());
    let mut engine = Engine::with_path(path.clone(), 1, 16).unwrap();
    engine.attach_database(db.clone());
    let cancel = AtomicBool::new(false);
    let base = Search::position(STARTING_FEN, 8, 2, 1.0);
    let first = engine.search(&base, &cancel).unwrap();
    assert_eq!(engine.search(&base, &cancel).unwrap(), first);
    assert_eq!(searches(&path), 1);
    let mut budget = base.clone();
    budget.time_cap = 2.0;
    engine.search(&budget, &cancel).unwrap();
    assert_eq!(searches(&path), 2);
    let halfmove = Search::position(&STARTING_FEN.replace("0 1", "1 1"), 8, 2, 1.0);
    engine.search(&halfmove, &cancel).unwrap();
    assert_eq!(searches(&path), 3);
    // The advertised version is independently part of the identity, even when
    // the executable path/metadata and search request remain unchanged.
    engine.name = "FakeFish 2".into();
    engine.search(&base, &cancel).unwrap();
    assert_eq!(searches(&path), 4);
    let other_path = dir.path().join(if cfg!(windows) {
        "other-fish.exe"
    } else {
        "other-fish"
    });
    std::fs::copy(&path, &other_path).unwrap();
    let mut other = Engine::with_path(other_path.clone(), 1, 16).unwrap();
    other.attach_database(db.clone());
    other.search(&base, &cancel).unwrap();
    assert_eq!(searches(&other_path), 1);
    assert_eq!(searches(&path), 4);
    assert_eq!(
        db.scalar("SELECT COUNT(*) FROM position_cache", &[])
            .unwrap(),
        5
    );
}

fn wait_for_searches(path: &std::path::Path, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while searches(path) < count {
        assert!(Instant::now() < deadline, "expected {count} searches");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn release_searches(path: &std::path::Path) {
    std::fs::write(path.with_extension("release"), b"ready").unwrap();
}

#[test]
fn concurrent_identical_searches_share_one_result_and_replay_exactly() {
    let (dir, path) = fixture("gate");
    let db = Arc::new(plyback::db::Database::open(dir.path().join("cache.db")).unwrap());
    let barrier = Arc::new(std::sync::Barrier::new(5));
    let mut workers = Vec::new();
    for _ in 0..4 {
        let mut engine = Engine::with_path(path.clone(), 1, 16).unwrap();
        engine.attach_database(Arc::clone(&db));
        let barrier = Arc::clone(&barrier);
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            let result = engine
                .analyse(STARTING_FEN, 12, 2, 1.0, &AtomicBool::new(false))
                .unwrap();
            (result, engine.metrics())
        }));
    }
    barrier.wait();
    wait_for_searches(&path, 1);
    // Keep the first search in flight while the other ready workers join it.
    std::thread::sleep(Duration::from_millis(100));
    release_searches(&path);
    let results: Vec<_> = workers.into_iter().map(|t| t.join().unwrap()).collect();
    for (result, _) in &results {
        assert_eq!(result, &results[0].0);
    }
    assert_eq!(searches(&path), 1);
    assert_eq!(results.iter().map(|(_, m)| m.searches).sum::<u64>(), 1);
    assert_eq!(
        results
            .iter()
            .map(|(_, m)| m.shared_search_hits + m.cache_hits)
            .sum::<u64>(),
        3
    );
    let mut warm = Engine::with_path(path.clone(), 1, 16).unwrap();
    warm.attach_database(Arc::clone(&db));
    assert_eq!(
        warm.analyse(STARTING_FEN, 12, 2, 1.0, &AtomicBool::new(false))
            .unwrap(),
        results[0].0
    );
    assert_eq!(warm.metrics().cache_hits, 1);
    assert_eq!(searches(&path), 1);
    assert_eq!(
        db.scalar("SELECT COUNT(*) FROM position_cache", &[])
            .unwrap(),
        1
    );
}

#[test]
fn concurrent_different_keys_and_databases_search_independently() {
    use plyback::engine::Search;
    let (dir, path) = fixture("gate");
    let db = Arc::new(plyback::db::Database::open(dir.path().join("cache.db")).unwrap());
    let other_db = Arc::new(plyback::db::Database::open(dir.path().join("other.db")).unwrap());
    let base = Search::position(STARTING_FEN, 12, 2, 1.0);
    let mut history = base.clone();
    history.moves = ["g1f3", "g8f6", "f3g1", "f6g8"].map(String::from).to_vec();
    let mut roots = base.clone();
    roots.root_moves = vec!["d2d4".into(), "e2e4".into()];
    let mut nodes = base.clone();
    nodes.nodes = Some(1000);
    let requests = [base.clone(), history, roots, nodes];
    let mut workers = Vec::new();
    for (database, request) in requests
        .iter()
        .cloned()
        .map(|r| (Arc::clone(&db), r))
        .chain(std::iter::once((Arc::clone(&other_db), base)))
    {
        let mut engine = Engine::with_path(path.clone(), 1, 16).unwrap();
        engine.attach_database(database);
        workers.push(std::thread::spawn(move || {
            let result = engine.search(&request, &AtomicBool::new(false)).unwrap();
            assert_eq!(engine.metrics().shared_search_hits, 0);
            result
        }));
    }
    // Every distinct key/owner must make progress while other searches are
    // blocked; a single coarse database or FEN lock would deadlock this gate.
    wait_for_searches(&path, workers.len());
    release_searches(&path);
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(
        db.scalar("SELECT COUNT(*) FROM position_cache", &[])
            .unwrap(),
        4
    );
    assert_eq!(
        other_db
            .scalar("SELECT COUNT(*) FROM position_cache", &[])
            .unwrap(),
        1
    );
}

#[test]
fn cancelling_a_follower_does_not_cancel_the_shared_search() {
    let (dir, path) = fixture("gate");
    let db = Arc::new(plyback::db::Database::open(dir.path().join("cache.db")).unwrap());
    let mut leader = Engine::with_path(path.clone(), 1, 16).unwrap();
    leader.attach_database(Arc::clone(&db));
    let mut follower = Engine::with_path(path.clone(), 1, 16).unwrap();
    follower.attach_database(Arc::clone(&db));
    let leader = std::thread::spawn(move || {
        leader.analyse(STARTING_FEN, 12, 2, 2.0, &AtomicBool::new(false))
    });
    wait_for_searches(&path, 1);
    let cancel = Arc::new(AtomicBool::new(false));
    let follower_cancel = Arc::clone(&cancel);
    let follower =
        std::thread::spawn(move || follower.analyse(STARTING_FEN, 12, 2, 2.0, &follower_cancel));
    std::thread::sleep(Duration::from_millis(50));
    let started = Instant::now();
    cancel.store(true, Ordering::SeqCst);
    let error = follower.join().unwrap().unwrap_err();
    assert_eq!(error.downcast_ref::<plyback::ApiError>().unwrap().0, 499);
    assert!(started.elapsed() < Duration::from_millis(500));
    assert_eq!(searches(&path), 1);
    release_searches(&path);
    assert!(leader.join().unwrap().is_ok());
    assert_eq!(
        db.scalar("SELECT COUNT(*) FROM position_cache", &[])
            .unwrap(),
        1
    );
}

#[test]
fn cancelling_a_leader_allows_the_follower_to_retry_and_cache() {
    let (dir, path) = fixture("gate");
    let db = Arc::new(plyback::db::Database::open(dir.path().join("cache.db")).unwrap());
    let mut leader = Engine::with_path(path.clone(), 1, 16).unwrap();
    leader.attach_database(Arc::clone(&db));
    let mut follower = Engine::with_path(path.clone(), 1, 16).unwrap();
    follower.attach_database(Arc::clone(&db));
    let cancel = Arc::new(AtomicBool::new(false));
    let leader_cancel = Arc::clone(&cancel);
    let leader =
        std::thread::spawn(move || leader.analyse(STARTING_FEN, 12, 2, 2.0, &leader_cancel));
    wait_for_searches(&path, 1);
    let follower = std::thread::spawn(move || {
        let result = follower
            .analyse(STARTING_FEN, 12, 2, 2.0, &AtomicBool::new(false))
            .unwrap();
        (result, follower.metrics())
    });
    std::thread::sleep(Duration::from_millis(50));
    cancel.store(true, Ordering::SeqCst);
    let error = leader.join().unwrap().unwrap_err();
    assert_eq!(error.downcast_ref::<plyback::ApiError>().unwrap().0, 499);
    wait_for_searches(&path, 2);
    release_searches(&path);
    let (result, metrics) = follower.join().unwrap();
    assert_eq!(metrics.searches, 1);
    let mut warm = Engine::with_path(path.clone(), 1, 16).unwrap();
    warm.attach_database(db);
    assert_eq!(
        warm.analyse(STARTING_FEN, 12, 2, 2.0, &AtomicBool::new(false))
            .unwrap(),
        result
    );
    assert_eq!(searches(&path), 2);
}

#[test]
fn failed_shared_search_releases_waiters_and_does_not_cache_errors() {
    let (dir, path) = fixture("gate-exit");
    let db = Arc::new(plyback::db::Database::open(dir.path().join("cache.db")).unwrap());
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let mut workers = Vec::new();
    for _ in 0..2 {
        let mut engine = Engine::with_path(path.clone(), 1, 16).unwrap();
        engine.attach_database(Arc::clone(&db));
        let done_tx = done_tx.clone();
        workers.push(std::thread::spawn(move || {
            let error = engine
                .analyse(STARTING_FEN, 12, 2, 1.0, &AtomicBool::new(false))
                .unwrap_err();
            done_tx
                .send(error.downcast_ref::<plyback::ApiError>().unwrap().0)
                .unwrap();
        }));
    }
    wait_for_searches(&path, 1);
    std::thread::sleep(Duration::from_millis(50));
    release_searches(&path);
    for _ in 0..2 {
        assert_eq!(done_rx.recv_timeout(Duration::from_secs(5)).unwrap(), 503);
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(searches(&path), 4); // one retry per worker, as without sharing
    assert_eq!(
        db.scalar("SELECT COUNT(*) FROM position_cache", &[])
            .unwrap(),
        0
    );
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
    let frontend = dir.path().join("frontend");
    std::fs::create_dir_all(&frontend).unwrap();
    std::fs::write(frontend.join("index.html"), "<!doctype html>").unwrap();
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
        .arg("--frontend")
        .arg(&frontend)
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
