mod queue;
mod routes;
mod settings;
pub mod store;
mod sync;

use crate::{ApiResult, Query, db::Database, locks::RecoverLock};
use anyhow::Result;
use serde_json::{Value, json};
pub use settings::{default_settings, get_settings, save_settings};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::JoinHandle,
    time::Duration,
};

pub const PRIORITY_MANUAL: i64 = 100;
pub const PRIORITY_NEW: i64 = 50;
pub const PRIORITY_REPORT: i64 = 30;
pub const PRIORITY_HISTORY: i64 = 10;

pub struct Services {
    pub db: Arc<Database>,
    stop: AtomicBool,
    started: AtomicBool,
    paused: AtomicBool,
    pause_control: Mutex<()>,
    generation: AtomicU64,
    wake: (Mutex<u64>, Condvar),
    handles: Mutex<Vec<JoinHandle<()>>>,
    progress: Mutex<HashMap<String, (Value, Arc<AtomicBool>)>>,
    pending_releases: Mutex<Vec<queue::PendingRelease>>,
    engine_name: Mutex<Option<String>>,
    engine_error: Mutex<Option<String>>,
    worker_count: AtomicU64,
    requested: Mutex<HashSet<i64>>,
    current_sync: Mutex<Option<i64>>,
    /// Accounts whose last sync found the database busy, and when to try them again.
    sync_busy_until: Mutex<HashMap<i64, std::time::Instant>>,
    /// When each account's last sync failed. Retries are paced from it, so last_sync_at keeps
    /// meaning the last successful sync (the boundary between history and new games).
    sync_failed_at: Mutex<HashMap<i64, i64>>,
    client: sync::ChessComClient,
    lichess_client: sync::LichessClient,
}

impl Services {
    pub fn new(db: Arc<Database>) -> Result<Arc<Self>> {
        // Queue state is internal to the controls, rather than an editable
        // engine setting. Restore it before any background worker can start.
        let paused = db
            .one("SELECT value FROM settings WHERE key='queue_paused'", &[])?
            .map(|row| serde_json::from_str::<bool>(row["value"].as_str().unwrap_or("")))
            .transpose()?
            .unwrap_or(false);
        Ok(Arc::new(Self {
            db,
            stop: AtomicBool::new(false),
            started: AtomicBool::new(false),
            paused: AtomicBool::new(paused),
            pause_control: Mutex::new(()),
            generation: AtomicU64::new(0),
            wake: (Mutex::new(0), Condvar::new()),
            handles: Mutex::new(Vec::new()),
            progress: Mutex::new(HashMap::new()),
            pending_releases: Mutex::new(Vec::new()),
            engine_name: Mutex::new(None),
            engine_error: Mutex::new(None),
            worker_count: AtomicU64::new(0),
            requested: Mutex::new(HashSet::new()),
            current_sync: Mutex::new(None),
            sync_busy_until: Mutex::new(HashMap::new()),
            sync_failed_at: Mutex::new(HashMap::new()),
            client: sync::ChessComClient::new()?,
            lichess_client: sync::LichessClient::new()?,
        }))
    }
    pub fn start(self: &Arc<Self>) {
        if self.started.swap(true, Ordering::SeqCst) {
            return;
        }
        self.stop.store(false, Ordering::SeqCst);
        if let Err(error) = self.db.execute(
            "UPDATE jobs SET status='queued',attempts=MAX(0,attempts-1),claim_token=NULL WHERE status='running'",
            &[],
        ) {
            tracing::error!(%error,"Could not recover queue");
        }
        let _ = self.db.execute(
            "UPDATE accounts SET sync_status='idle' WHERE sync_status='syncing'",
            &[],
        );
        if let Ok(rows) = self.db.query(
            "SELECT game_id FROM game_analysis WHERE version<?",
            &[json!(crate::ANALYSIS_VERSION)],
        ) {
            let ids = rows
                .iter()
                .filter_map(|r| r["game_id"].as_i64())
                .collect::<Vec<_>>();
            let _ = self.enqueue(&ids, 5, None, false);
        }
        let queue = Arc::clone(self);
        let sync = Arc::clone(self);
        let mut handles = self.handles.lock().recover();
        handles.push(std::thread::spawn(move || queue.supervise_workers()));
        handles.push(std::thread::spawn(move || sync.sync_loop()));
    }
    /// Broadcast cancellation before HTTP graceful drain, without joining threads.
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        for (_, cancel) in self.progress.lock().recover().values() {
            cancel.store(true, Ordering::SeqCst);
        }
        self.notify();
    }
    pub fn stop(&self) {
        self.request_stop();
        let handles = std::mem::take(&mut *self.handles.lock().recover());
        for handle in handles {
            if handle.join().is_err() {
                tracing::error!("Background service panicked");
            }
        }
        self.started.store(false, Ordering::SeqCst);
    }
    pub fn apply_settings(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.notify();
    }
    fn set_paused(&self, paused: bool) -> Result<()> {
        // Serialize persistence with publication to workers. Concurrent pause
        // and resume requests must leave memory agreeing with the durable row.
        let _control = self.pause_control.lock().recover();
        self.db.execute(
            "INSERT INTO settings(key,value) VALUES('queue_paused',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            &[json!(paused.to_string())],
        )?;
        self.paused.store(paused, Ordering::SeqCst);
        if !paused {
            self.notify();
        }
        Ok(())
    }
    pub fn handle(&self, method: &str, path: &str, q: &Query, body: &Value) -> Option<ApiResult> {
        self.route(method, path, q, body)
    }
    fn notify(&self) {
        let mut generation = self.wake.0.lock().recover();
        *generation = generation.wrapping_add(1);
        self.wake.1.notify_all();
    }
    fn db_wait(&self) -> (std::time::Instant, impl Fn() -> bool + '_) {
        (std::time::Instant::now() + Duration::from_secs(2), || {
            self.stop.load(Ordering::SeqCst)
        })
    }
    fn wait(&self, duration: Duration) {
        self.wait_since(self.wake_generation(), duration);
    }
    fn wake_generation(&self) -> u64 {
        *self.wake.0.lock().recover()
    }
    /// Observe before checking for work so a notification between the check
    /// and this wait cannot be lost. Returns whether the wait timed out.
    fn wait_since(&self, observed: u64, duration: Duration) -> bool {
        let lock = self.wake.0.lock().recover();
        self.wake
            .1
            .wait_timeout_while(lock, duration, |generation| {
                *generation == observed && !self.stop.load(Ordering::SeqCst)
            })
            .recover()
            .1
            .timed_out()
    }
    pub fn engine_status(&self) -> Value {
        let located = crate::engine::locate_stockfish();
        let error = self
            .engine_error
            .lock()
            .recover()
            .clone()
            .or_else(|| located.as_ref().err().map(ToString::to_string));
        json!({"name":*self.engine_name.lock().recover(),"path":located.ok().map(|p|p.to_string_lossy().to_string()),"error":error})
    }
    pub fn sync_status(&self) -> Value {
        json!({"current_account":*self.current_sync.lock().recover()})
    }
    fn request_sync(&self, id: i64) {
        self.requested.lock().recover().insert(id);
        self.notify();
    }
}

#[cfg(test)]
mod wake_tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn poisoned_service_state_and_condvar_remain_usable() {
        let db = Arc::new(Database::open(":memory:").unwrap());
        let service = Services::new(db).unwrap();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = service.wake.0.lock().unwrap();
            panic!("interrupted notification");
        }));
        assert!(panic.is_err());
        service.notify();
        let observed = service.wake_generation();
        assert_eq!(observed, 1);
        assert!(service.wait_since(observed, Duration::ZERO));
    }

    #[test]
    fn settings_change_while_working_then_stop_joins_every_worker() {
        const CHILD: &str = "PLYBACK_RECONFIGURATION_CHILD";
        if std::env::var_os(CHILD).is_none() {
            // Set the engine override only in a child process: parallel unit tests
            // must retain their own Stockfish selection.
            let dir = tempfile::tempdir().unwrap();
            let engine = dir
                .path()
                .join(if cfg!(windows) { "gate.exe" } else { "gate" });
            assert!(
                std::process::Command::new("rustc")
                    .args(["--edition=2024"])
                    .arg(
                        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                            .join("tests/fixtures/uci_fixture.rs")
                    )
                    .arg("-o")
                    .arg(&engine)
                    .status()
                    .unwrap()
                    .success()
            );
            let test =
                "services::wake_tests::settings_change_while_working_then_stop_joins_every_worker";
            let mut child = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", test, "--nocapture", "--test-threads=1"])
                .env(CHILD, "1")
                .env("PLYBACK_STOCKFISH", &engine)
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .unwrap();
            // Drain both pipes while the child runs: a long panic message must not
            // block the child on a full pipe and turn into a timeout.
            use std::io::Read;
            let drain = |pipe: Box<dyn std::io::Read + Send>| {
                std::thread::spawn(move || {
                    let mut text = String::new();
                    let _ = std::io::BufReader::new(pipe).read_to_string(&mut text);
                    text
                })
            };
            let stdout = drain(Box::new(child.stdout.take().unwrap()));
            let stderr = drain(Box::new(child.stderr.take().unwrap()));
            let deadline = Instant::now() + Duration::from_secs(45);
            let mut timed_out = false;
            while child.try_wait().unwrap().is_none() {
                if Instant::now() >= deadline {
                    child.kill().unwrap();
                    timed_out = true;
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            let status = child.wait().unwrap();
            let (stdout, stderr) = (stdout.join().unwrap(), stderr.join().unwrap());
            assert!(
                !timed_out,
                "worker shutdown timed out:
{stdout}
{stderr}"
            );
            assert!(
                status.success(),
                "{stdout}
{stderr}"
            );
            // A renamed test would match nothing and still exit successfully.
            assert!(
                stdout.contains("1 passed"),
                "child ran no test:
{stdout}"
            );
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open(dir.path().join("workers.db")).unwrap());
        save_settings(&db, &serde_json::json!({"workers":1,"hash_mb":16,"analysis_depth":30,"max_seconds_per_position":60.0})).unwrap();
        let service = Services::new(Arc::clone(&db)).unwrap();
        struct StopOnDrop(Arc<Services>);
        impl Drop for StopOnDrop {
            fn drop(&mut self) {
                self.0.stop();
            }
        }
        let _cleanup = StopOnDrop(Arc::clone(&service));
        service
            .handle(
                "POST",
                "/api/games/import",
                &Query::new(),
                &serde_json::json!({"pgn":"1. e4 e5 *","analyze":true}),
            )
            .unwrap()
            .unwrap();
        service.start();
        let engine = std::path::PathBuf::from(std::env::var_os("PLYBACK_STOCKFISH").unwrap());
        let wait_for_search = |count: usize| {
            let deadline = Instant::now() + Duration::from_secs(15);
            loop {
                let log = std::fs::read_to_string(engine.with_extension("log")).unwrap_or_default();
                if log.lines().filter(|line| line.starts_with("go ")).count() >= count {
                    return log;
                }
                assert!(
                    Instant::now() < deadline,
                    "worker did not search: {}",
                    service.stats()
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        };
        wait_for_search(1);
        assert_eq!(service.stats()["running"], 1);
        save_settings(&db, &serde_json::json!({"hash_mb":32})).unwrap();
        service.apply_settings();
        let log = wait_for_search(2);
        assert!(log.contains("setoption name Hash value 16"));
        assert!(log.contains("setoption name Hash value 32"));
        assert!(
            log.lines()
                .filter(|line| line.starts_with("start "))
                .count()
                >= 2
        );
        // Reconfigure again and stop at once, while the replacement worker is still
        // blocked in its gated search: stop() must join the retiring worker too.
        save_settings(&db, &serde_json::json!({"hash_mb":64})).unwrap();
        service.apply_settings();
        let started = Instant::now();
        service.stop();
        assert!(started.elapsed() < Duration::from_secs(5));
        // Every worker, the supervisor and the sync loop hold a clone; only this
        // test's two remain once all of them have been joined.
        assert_eq!(Arc::strong_count(&service), 2);
        assert_eq!(service.stats()["workers"], 0);
        assert_eq!(service.stats()["running"], 0);
        assert!(service.progress.lock().unwrap().is_empty());
        let starts = |log: &str| {
            log.lines()
                .filter(|line| line.starts_with("start "))
                .count()
        };
        let after_stop = starts(&std::fs::read_to_string(engine.with_extension("log")).unwrap());
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            starts(&std::fs::read_to_string(engine.with_extension("log")).unwrap()),
            after_stop,
            "an engine started after stop()"
        );
    }

    fn enqueue_in_empty_check_race(samples: usize) -> Vec<f64> {
        let db = Arc::new(Database::open(":memory:").unwrap());
        db.execute(
            "INSERT INTO games(uuid,pgn,imported_at) VALUES('wake-test','1. e4 e5 *',0)",
            &[],
        )
        .unwrap();
        let service = Services::new(db.clone()).unwrap();
        let mut samples_ms = Vec::new();
        for _ in 0..samples {
            let observed = service.wake_generation();
            assert_eq!(
                db.scalar("SELECT COUNT(*) FROM jobs WHERE status='queued'", &[])
                    .unwrap(),
                0
            );
            service.enqueue(&[1], PRIORITY_NEW, None, false).unwrap();
            let started = Instant::now();
            // A timeout is a lost notification, independent of host timing.
            assert!(!service.wait_since(observed, Duration::from_millis(500)));
            samples_ms.push(started.elapsed().as_secs_f64() * 1000.0);
            db.execute("DELETE FROM jobs", &[]).unwrap();
        }
        samples_ms
    }

    #[test]
    fn enqueue_between_empty_check_and_wait_is_not_lost() {
        enqueue_in_empty_check_race(1);
    }

    #[test]
    fn unchanged_generation_waits_and_stop_does_not() {
        let service = Services::new(Arc::new(Database::open(":memory:").unwrap())).unwrap();
        let observed = service.wake_generation();
        assert!(service.wait_since(observed, Duration::from_millis(1)));
        service.request_stop();
        let observed = service.wake_generation();
        assert!(!service.wait_since(observed, Duration::from_millis(500)));
    }

    /// Force the enqueue between checking an empty queue and waiting.
    #[test]
    #[ignore = "explicit idle enqueue latency measurement"]
    fn benchmark_idle_enqueue_race() {
        println!(
            "idle_enqueue_samples_ms={:?}",
            enqueue_in_empty_check_race(5)
        );
    }
}
