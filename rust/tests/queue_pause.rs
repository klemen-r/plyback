//! Queue controls must survive closing and reopening an isolated database.
use plyback::{Query, db::Database, services::Services};
use serde_json::{Value, json};
use std::sync::Arc;

fn request(service: &Services, method: &str, path: &str) -> anyhow::Result<Value> {
    service
        .handle(method, path, &Query::new(), &json!({}))
        .unwrap()
        .map(|(_, body)| body)
}

#[test]
fn pause_and_resume_survive_reopened_database() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("queue.db");
    for (action, expected) in [("pause", true), ("resume", false)] {
        {
            let service = Services::new(Arc::new(Database::open(&path).unwrap())).unwrap();
            assert_eq!(
                request(&service, "POST", &format!("/api/queue/{action}")).unwrap()["paused"],
                expected
            );
        }
        let service = Services::new(Arc::new(Database::open(&path).unwrap())).unwrap();
        service.start();
        let paused = request(&service, "GET", "/api/queue").unwrap()["paused"].clone();
        service.stop();
        assert_eq!(
            paused, expected,
            "queue {action} was lost when the database reopened"
        );
    }
}

#[test]
fn failed_pause_or_resume_does_not_change_the_queue_state() {
    for (initial, action, expected) in [("resume", "pause", false), ("pause", "resume", true)] {
        let db = Arc::new(Database::open(":memory:").unwrap());
        let service = Services::new(Arc::clone(&db)).unwrap();
        request(&service, "POST", &format!("/api/queue/{initial}")).unwrap();
        // Reject settings writes without changing or locking any live data.
        db.execute(
            "CREATE TRIGGER reject_settings_insert BEFORE INSERT ON settings BEGIN SELECT RAISE(ABORT, 'injected settings failure'); END",
            &[],
        )
        .unwrap();
        db.execute(
            "CREATE TRIGGER reject_settings_update BEFORE UPDATE ON settings BEGIN SELECT RAISE(ABORT, 'injected settings failure'); END",
            &[],
        )
        .unwrap();
        assert!(request(&service, "POST", &format!("/api/queue/{action}")).is_err());
        assert_eq!(
            request(&service, "GET", "/api/queue").unwrap()["paused"],
            expected
        );
        let reopened = Services::new(db).unwrap();
        assert_eq!(
            request(&reopened, "GET", "/api/queue").unwrap()["paused"],
            expected
        );
    }
}

#[test]
fn concurrent_queue_controls_keep_memory_and_storage_in_sync() {
    let db = Arc::new(Database::open(":memory:").unwrap());
    let service = Services::new(Arc::clone(&db)).unwrap();
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let threads = (0..8)
        .map(|i| {
            let service = Arc::clone(&service);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                for turn in 0..16 {
                    let action = if (i + turn) % 2 == 0 {
                        "pause"
                    } else {
                        "resume"
                    };
                    request(&service, "POST", &format!("/api/queue/{action}")).unwrap();
                }
            })
        })
        .collect::<Vec<_>>();
    for thread in threads {
        thread.join().unwrap();
    }
    let current = request(&service, "GET", "/api/queue").unwrap()["paused"].clone();
    let reopened = Services::new(db).unwrap();
    assert_eq!(
        request(&reopened, "GET", "/api/queue").unwrap()["paused"],
        current
    );
}
