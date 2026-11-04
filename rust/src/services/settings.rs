use crate::{
    api_error,
    db::{Database, query_connection},
};
use anyhow::Result;
use serde_json::{Value, json};

pub fn default_settings() -> Value {
    let cpus = std::thread::available_parallelism().map_or(2, usize::from);
    json!({"analysis_depth":16,"analysis_multipv":2,"max_seconds_per_position":5.0,
        "workers":(cpus/2).clamp(1,8),"threads_per_worker":1,"hash_mb":64,"interactive_depth":20,
        "sync_interval_seconds":180,"active_sync_interval_seconds":60,"auto_analyze_new":true,
        "analyze_history":true,"contact":""})
}
pub fn get_settings(db: &Database) -> Result<Value> {
    Ok(merge_stored(
        db.query("SELECT key,value FROM settings", &[])?,
    ))
}
pub(super) fn get_settings_until(
    db: &Database,
    deadline: std::time::Instant,
    cancelled: impl Fn() -> bool,
) -> Result<Option<Value>> {
    Ok(db
        .read_until(deadline, cancelled, |connection| {
            query_connection(connection, "SELECT key,value FROM settings", &[])
        })?
        .map(merge_stored))
}
fn merge_stored(rows: Vec<Value>) -> Value {
    let mut settings = default_settings();
    for row in rows {
        let key = row["key"].as_str().unwrap_or("");
        if settings.get(key).is_some()
            && let Some(v) = row["value"]
                .as_str()
                .and_then(|s| serde_json::from_str::<Value>(s).ok())
        {
            settings[key] = v;
        }
    }
    settings
}
pub(super) fn validate_resources(settings: &Value) -> Result<()> {
    let cpus = std::thread::available_parallelism().map_or(2, usize::from) as u64;
    let workers = settings["workers"]
        .as_u64()
        .filter(|n| (1..=cpus.min(32)).contains(n))
        .ok_or_else(|| api_error(400, "Invalid stored worker count; update engine settings"))?;
    let hash = settings["hash_mb"]
        .as_u64()
        .filter(|n| (16..=4096).contains(n))
        .ok_or_else(|| api_error(400, "Invalid stored hash memory; update engine settings"))?;
    let threads = settings["threads_per_worker"]
        .as_u64()
        .filter(|n| (1..=cpus).contains(n))
        .ok_or_else(|| api_error(400, "Invalid stored engine threads; update engine settings"))?;
    if workers * hash > 8192 {
        return Err(api_error(
            400,
            "Combined worker hash memory must not exceed 8192 MiB",
        ));
    }
    if workers * threads > cpus * 2 {
        return Err(api_error(
            400,
            "Combined engine threads must not exceed twice the CPU count",
        ));
    }
    Ok(())
}
pub fn save_settings(db: &Database, patch: &Value) -> Result<Value> {
    let patch = patch
        .as_object()
        .ok_or_else(|| api_error(422, "Settings must be an object"))?;
    let defaults = default_settings();
    let cpus = std::thread::available_parallelism().map_or(2, usize::from) as f64;
    let mut clean = serde_json::Map::new();
    for (key, value) in patch {
        let default = defaults
            .get(key)
            .ok_or_else(|| api_error(400, format!("Unknown setting: {key}")))?;
        let v = if default.is_boolean() {
            if !value.is_boolean() {
                return Err(api_error(400, format!("{key} must be true/false")));
            }
            value.clone()
        } else if default.is_number() {
            let n = value
                .as_f64()
                .ok_or_else(|| api_error(400, format!("{key} must be a number")))?;
            let (low, high) = match key.as_str() {
                "analysis_depth" => (6., 30.),
                "analysis_multipv" => (1., 5.),
                "max_seconds_per_position" => (0.2, 60.),
                "workers" => (1., cpus.min(32.)),
                "threads_per_worker" => (1., cpus),
                "hash_mb" => (16., 4096.),
                "interactive_depth" => (8., 32.),
                "sync_interval_seconds" => (60., 86400.),
                "active_sync_interval_seconds" => (30., 3600.),
                _ => unreachable!(),
            };
            if !n.is_finite() || n < low || n > high {
                return Err(api_error(
                    400,
                    format!("{key} must be between {low} and {high}"),
                ));
            }
            if default.is_i64() || default.is_u64() {
                json!(n as i64)
            } else {
                json!(n)
            }
        } else {
            let s = value
                .as_str()
                .ok_or_else(|| api_error(400, format!("{key} must be a short string")))?;
            if s.chars().count() > 200 {
                return Err(api_error(400, format!("{key} must be a short string")));
            }
            json!(s)
        };
        clean.insert(key.clone(), v);
    }
    // Validate the combined settings in the same transaction as the writes, so
    // concurrent worker/hash updates cannot each pass against an obsolete value.
    db.transaction(|tx| {
        let mut combined=merge_stored(query_connection(tx,"SELECT key,value FROM settings",&[])?);
        for (key,value) in &clean {combined[key]=value.clone();}
        validate_resources(&combined)?;
        for (key,value) in &clean {tx.execute("INSERT INTO settings(key,value) VALUES(?,?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",rusqlite::params![key,value.to_string()])?;}
        Ok(combined)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_reject_unknown_and_non_numeric_atomically() {
        let db = Database::open(":memory:").unwrap();
        assert!(save_settings(&db, &json!({"workers":true})).is_err());
        assert!(
            save_settings(&db, &json!({"analysis_depth":8,"auth_password_hash":"bad"})).is_err()
        );
        assert_eq!(get_settings(&db).unwrap()["analysis_depth"], 16);
        assert_eq!(
            save_settings(&db, &json!({"analysis_depth":8})).unwrap()["analysis_depth"],
            8
        );
    }
    #[test]
    fn aggregate_engine_limits_prevent_multiplying_resource_budgets() {
        let db = Database::open(":memory:").unwrap();
        let cpus = std::thread::available_parallelism().map_or(2, usize::from);
        if cpus >= 3 {
            assert!(
                save_settings(&db, &json!({"workers":3,"hash_mb":4096}))
                    .unwrap_err()
                    .to_string()
                    .contains("8192")
            );
            assert!(
                save_settings(&db, &json!({"workers":3,"threads_per_worker":cpus}))
                    .unwrap_err()
                    .to_string()
                    .contains("twice")
            );
        }
        assert_eq!(get_settings(&db).unwrap()["hash_mb"], 64);
        assert!(
            save_settings(
                &db,
                &json!({"workers":1,"hash_mb":4096,"threads_per_worker":cpus})
            )
            .is_ok()
        );
    }
    #[test]
    fn concurrent_resource_changes_validate_combined_state_atomically() {
        use std::sync::{Arc, Barrier};
        if std::thread::available_parallelism().map_or(2, usize::from) < 3 {
            return;
        }
        let db = Arc::new(Database::open(":memory:").unwrap());
        save_settings(&db, &json!({"workers":1,"hash_mb":64})).unwrap();
        let gate = Arc::new(Barrier::new(3));
        let mut handles = Vec::new();
        for patch in [json!({"workers":3}), json!({"hash_mb":4096})] {
            let (db, gate) = (Arc::clone(&db), Arc::clone(&gate));
            handles.push(std::thread::spawn(move || {
                gate.wait();
                save_settings(&db, &patch).is_ok()
            }));
        }
        gate.wait();
        let successes = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|passed| *passed)
            .count();
        assert_eq!(successes, 1);
        validate_resources(&get_settings(&db).unwrap()).unwrap();
    }
}
