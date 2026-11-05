//! Opt-in real queue benchmark. Supply a self-contained backup with --source;
//! all SQLite work happens in temporary directories under ../benchmark-output.
//! Never point this tool at a live database. A cold/warm mismatch fails the run.
use anyhow::{Result, ensure};
use clap::Parser;
use plyback::{
    db::Database,
    services::{Services, get_settings, save_settings},
};
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tracing::{
    Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context, prelude::*};

#[derive(Parser)]
struct Args {
    /// Self-contained SQLite backup or generated fixture, never the live database.
    #[arg(long)]
    source: PathBuf,
    /// New aggregate JSON file inside benchmark-output (no PGNs or identities).
    #[arg(long)]
    out: PathBuf,
    #[arg(long, default_value_t = 10)]
    games: usize,
    /// Override review worker count in disposable copies; search budgets are unchanged.
    #[arg(long)]
    workers: Option<usize>,
}

fn hash_file(path: &Path) -> Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0; 65_536];
    loop {
        let size = file.read(&mut buffer)?;
        if size == 0 {
            break;
        }
        digest.update(&buffer[..size]);
    }
    Ok(hex::encode(digest.finalize()))
}

fn validate_snapshot(path: &Path) -> Result<()> {
    ensure!(path.is_file(), "Source must be an existing snapshot file");
    ensure!(
        !path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().eq_ignore_ascii_case("plyback.db")),
        "Refusing the application's database name; supply a separate backup"
    );
    for suffix in ["-wal", "-shm", "-journal"] {
        let mut sidecar = path.as_os_str().to_os_string();
        sidecar.push(suffix);
        ensure!(
            !PathBuf::from(sidecar).try_exists()?,
            "Supply a self-contained snapshot without SQLite sidecars"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_validation_rejects_live_names_and_all_sqlite_sidecars() {
        let directory = tempfile::tempdir().unwrap();
        let snapshot = directory.path().join("snapshot.sqlite");
        std::fs::write(&snapshot, b"source bytes must stay untouched").unwrap();
        let before = hash_file(&snapshot).unwrap();
        validate_snapshot(&snapshot).unwrap();
        for name in ["plyback.db", "Plyback.db"] {
            let live_name = directory.path().join(name);
            std::fs::write(&live_name, b"do not open this database").unwrap();
            assert!(validate_snapshot(&live_name).is_err());
        }
        for suffix in ["-wal", "-shm", "-journal"] {
            let sidecar = directory.path().join(format!("snapshot.sqlite{suffix}"));
            std::fs::write(&sidecar, b"pending database state").unwrap();
            assert!(validate_snapshot(&snapshot).is_err());
            std::fs::remove_file(sidecar).unwrap();
        }
        validate_snapshot(&snapshot).unwrap();
        assert_eq!(hash_file(&snapshot).unwrap(), before);
    }
}

#[derive(Clone, Default)]
struct Timings(Arc<Mutex<Vec<Value>>>);
struct Fields(Map<String, Value>);
impl Visit for Fields {
    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().into(), json!(value));
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.0
            .insert(field.name().into(), json!(format!("{value:?}")));
    }
}
impl<S: Subscriber> Layer<S> for Timings {
    fn on_event(&self, event: &tracing::Event<'_>, _ctx: Context<'_, S>) {
        if event.metadata().target() == "plyback::review_timing" {
            let mut fields = Fields(Map::new());
            event.record(&mut fields);
            self.0.lock().unwrap().push(Value::Object(fields.0));
        }
    }
}

// Export timing/count totals only: no per-position values, game identities or PGNs.
fn aggregate_timings(events: &[Value]) -> BTreeMap<String, BTreeMap<String, u64>> {
    let mut totals: BTreeMap<String, BTreeMap<String, u64>> = BTreeMap::new();
    for event in events {
        let mut name = event["event"].as_str().unwrap_or("unknown").to_owned();
        if let Some(kind) = event["kind"].as_str() {
            name.push('/');
            name.push_str(kind);
        }
        let group = totals.entry(name).or_default();
        *group.entry("calls".into()).or_default() += 1;
        for field in [
            "elapsed_us",
            "analysis_us",
            "storage_us",
            "searches",
            "cache_hits",
            "shared_search_hits",
        ] {
            if let Some(value) = event[field].as_u64() {
                *group.entry(field.into()).or_default() += value;
            }
        }
    }
    totals
}

// Compare every stored field except measurement timestamps and internal IDs.
fn stored_results(db: &Database, ids: &[i64]) -> Result<Vec<Value>> {
    let mut results = Vec::new();
    for id in ids {
        let mut summary = db.query("SELECT * FROM game_analysis WHERE game_id=?", &[json!(id)])?;
        for row in &mut summary {
            for key in ["game_id", "duration_ms", "analyzed_at", "revision"] {
                row.as_object_mut().unwrap().remove(key);
            }
        }
        let mut moves = db.query(
            "SELECT * FROM moves WHERE game_id=? ORDER BY ply",
            &[json!(id)],
        )?;
        for row in &mut moves {
            row.as_object_mut().unwrap().remove("game_id");
        }
        results.push(json!([summary, moves]));
    }
    Ok(results)
}

fn result_digest(db: &Database, ids: &[i64]) -> Result<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(
        &stored_results(db, ids)?,
    )?)))
}

// Field counts help detect shared-key cache overwrites between concurrent cold
// workers without exporting any result values or player identities.
fn differing_fields(cold: &Database, warm: &Database, ids: &[i64]) -> Result<Value> {
    let mut counts = Map::new();
    for (a, b) in stored_results(cold, ids)?
        .iter()
        .zip(stored_results(warm, ids)?)
    {
        for (index, table) in ["game_analysis", "moves"].iter().enumerate() {
            ensure!(
                a[index].as_array().unwrap().len() == b[index].as_array().unwrap().len(),
                "Stored row count changed"
            );
            for (left, right) in a[index]
                .as_array()
                .unwrap()
                .iter()
                .zip(b[index].as_array().unwrap())
            {
                for (field, value) in left.as_object().unwrap() {
                    if value != &right[field] {
                        let key = format!("{table}.{field}");
                        let count = counts.get(&key).and_then(Value::as_u64).unwrap_or(0);
                        counts.insert(key, json!(count + 1));
                    }
                }
            }
        }
    }
    Ok(Value::Object(counts))
}

fn run(db: Arc<Database>, ids: &[i64], timings: &Timings) -> Result<Value> {
    db.execute("DELETE FROM jobs", &[])?;
    db.execute("DELETE FROM game_analysis", &[])?;
    db.execute("DELETE FROM moves", &[])?;
    timings.0.lock().unwrap().clear();
    let service = Services::new(db.clone())?;
    service.start();
    let started = Instant::now();
    let result = (|| -> Result<Value> {
        ensure!(
            service.enqueue(ids, 50, Some(16), false)? == ids.len(),
            "Not all games queued"
        );
        let mut completion = vec![None; ids.len()];
        loop {
            let elapsed = started.elapsed().as_secs_f64();
            let completed = db.query("SELECT game_id FROM game_analysis", &[])?;
            let observed = started.elapsed().as_secs_f64();
            for row in completed {
                if let Some(index) = ids
                    .iter()
                    .position(|id| Some(*id) == row["game_id"].as_i64())
                {
                    completion[index].get_or_insert(observed);
                }
            }
            if completion.iter().all(Option::is_some) {
                break;
            }
            ensure!(elapsed < 1800.0, "Queue benchmark exceeded 30 minutes");
            ensure!(
                service.stats()["failed"] == 0,
                "Queue failed: {}",
                service.stats()
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        let wall_seconds = started.elapsed().as_secs_f64();
        // Stop joins workers, including their final timing event; excluded from wall time.
        service.stop();
        let timing_totals = aggregate_timings(&timings.0.lock().unwrap());
        Ok(
            json!({"wall_seconds":wall_seconds,"completion_seconds":completion,"digest":result_digest(&db,ids)?,
            "cache_entries":db.scalar("SELECT COUNT(*) FROM position_cache",&[])?,"timings":timing_totals}),
        )
    })();
    service.stop();
    result
}

fn main() -> Result<()> {
    ensure!(!cfg!(debug_assertions), "Use cargo run --release");
    let args = Args::parse();
    ensure!((1..=50).contains(&args.games), "Choose 1..50 games");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .canonicalize()?;
    let output_dir = root.join("benchmark-output");
    std::fs::create_dir_all(&output_dir)?;
    let output_dir = output_dir.canonicalize()?;
    let destination = if args.out.is_absolute() {
        args.out
    } else {
        root.join(args.out)
    };
    ensure!(
        destination.parent().unwrap().canonicalize()? == output_dir,
        "Output must be directly inside benchmark-output"
    );
    ensure!(!destination.exists(), "Refusing to overwrite output");
    let snapshot = if args.source.is_absolute() {
        args.source
    } else {
        root.join(args.source)
    }
    .canonicalize()?;
    validate_snapshot(&snapshot)?;
    let original_hash = hash_file(&snapshot)?;
    let cold_dir = tempfile::tempdir_in(&output_dir)?;
    let cold_path = cold_dir.path().join("review.db");
    std::fs::copy(&snapshot, &cold_path)?;
    validate_snapshot(&snapshot)?;
    ensure!(
        hash_file(&cold_path)? == original_hash && hash_file(&snapshot)? == original_hash,
        "Source snapshot changed while copying"
    );
    let db = Arc::new(Database::open(&cold_path)?);
    // Remove network accounts and report work only in the disposable copy.
    db.execute("DELETE FROM reports", &[])?;
    db.execute("DELETE FROM accounts", &[])?;
    db.execute("DELETE FROM position_cache", &[])?;
    let mut patch =
        json!({"analysis_depth":16,"analysis_multipv":2,"max_seconds_per_position":5.0});
    if let Some(workers) = args.workers {
        patch["workers"] = json!(workers);
    }
    save_settings(&db, &patch)?;
    let settings = get_settings(&db)?;
    let rows = db.query("SELECT id,plies FROM games WHERE rules IN ('chess','chess960') AND plies>0 ORDER BY end_time DESC,id DESC LIMIT ?", &[json!(args.games)])?;
    ensure!(
        rows.len() == args.games,
        "Snapshot has too few reviewable games"
    );
    let ids = rows
        .iter()
        .map(|row| row["id"].as_i64().unwrap())
        .collect::<Vec<_>>();
    let timings = Timings::default();
    tracing_subscriber::registry().with(timings.clone()).init();
    eprintln!(
        "Starting {} recent games: {} workers x {} threads, {} MiB hash",
        ids.len(),
        settings["workers"],
        settings["threads_per_worker"],
        settings["hash_mb"]
    );
    let cold = run(db.clone(), &ids, &timings)?;
    let warm_dir = tempfile::tempdir_in(&output_dir)?;
    let warm_path = warm_dir.path().join("review.db");
    Database::backup(&cold_path, &warm_path)?;
    eprintln!(
        "Cold complete: {:.3}s; starting warm copy",
        cold["wall_seconds"].as_f64().unwrap()
    );
    let warm_db = Arc::new(Database::open(&warm_path)?);
    let warm = run(warm_db.clone(), &ids, &timings)?;
    let warm_output_equal = cold["digest"] == warm["digest"];
    let warm_differing_fields = differing_fields(&db, &warm_db, &ids)?;
    validate_snapshot(&snapshot)?;
    ensure!(
        hash_file(&snapshot)? == original_hash,
        "Source snapshot changed"
    );
    let result = json!({"settings":{"depth":16,"multipv":2,"time_cap":5.0,"threat_depth":10,"workers":settings["workers"],"threads_per_worker":settings["threads_per_worker"],"hash_mb":settings["hash_mb"]},
        "plies":rows.iter().map(|row|row["plies"].clone()).collect::<Vec<_>>(),"cold":cold,"warm":warm,
        "snapshot_unchanged":true,"warm_output_equal":warm_output_equal,"warm_differing_fields":warm_differing_fields,"platform":std::env::consts::OS,"logical_cpus":std::thread::available_parallelism()?.get()});
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)?
        .write_all(&serde_json::to_vec_pretty(&result)?)?;
    eprintln!("Saved aggregate timings to {}", destination.display());
    ensure!(
        warm_output_equal,
        "Warm re-review changed stored results; see the saved differing-field counts"
    );
    Ok(())
}
