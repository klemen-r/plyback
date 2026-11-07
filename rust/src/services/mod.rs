mod queue;
mod routes;
mod settings;
pub mod store;
mod sync;

use crate::{ApiResult, Query, db::Database};
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
    client: sync::ChessComClient,
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
            client: sync::ChessComClient::new()?,
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
        let mut handles = self.handles.lock().unwrap();
        handles.push(std::thread::spawn(move || queue.supervise_workers()));
        handles.push(std::thread::spawn(move || sync.sync_loop()));
    }
    /// Broadcast cancellation before HTTP graceful drain, without joining threads.
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
        for (_, cancel) in self.progress.lock().unwrap().values() {
            cancel.store(true, Ordering::SeqCst);
        }
        self.notify();
    }
    pub fn stop(&self) {
        self.request_stop();
        let handles = std::mem::take(&mut *self.handles.lock().unwrap());
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
        let _control = self.pause_control.lock().unwrap();
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
        let mut generation = self.wake.0.lock().unwrap();
        *generation = generation.wrapping_add(1);
        self.wake.1.notify_all();
    }
    fn wait(&self, duration: Duration) {
        self.wait_since(self.wake_generation(), duration);
    }
    fn wake_generation(&self) -> u64 {
        *self.wake.0.lock().unwrap()
    }
    /// Observe before checking for work so a notification between the check
    /// and this wait cannot be lost. Returns whether the wait timed out.
    fn wait_since(&self, observed: u64, duration: Duration) -> bool {
        let lock = self.wake.0.lock().unwrap();
        self.wake
            .1
            .wait_timeout_while(lock, duration, |generation| {
                *generation == observed && !self.stop.load(Ordering::SeqCst)
            })
            .unwrap()
            .1
            .timed_out()
    }
    pub fn engine_status(&self) -> Value {
        let located = crate::engine::locate_stockfish();
        let error = self
            .engine_error
            .lock()
            .unwrap()
            .clone()
            .or_else(|| located.as_ref().err().map(ToString::to_string));
        json!({"name":*self.engine_name.lock().unwrap(),"path":located.ok().map(|p|p.to_string_lossy().to_string()),"error":error})
    }
    pub fn sync_status(&self) -> Value {
        json!({"current_account":*self.current_sync.lock().unwrap()})
    }
    fn request_sync(&self, id: i64) {
        self.requested.lock().unwrap().insert(id);
        self.notify();
    }
}

#[cfg(test)]
mod wake_tests {
    use super::*;
    use std::time::Instant;

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
