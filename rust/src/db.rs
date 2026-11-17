//! SQLite compatibility and atomic migrations. Every returned row owns its values;
//! no caller can retain a database lock while doing network, engine or CPU work.
use crate::board;
use anyhow::{Context, Result, bail};
use rusqlite::{
    Connection, OptionalExtension, Transaction, TransactionBehavior, params_from_iter,
    types::{Value as SqlValue, ValueRef},
};
use serde_json::{Map, Value};
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, MutexGuard, TryLockError},
    time::{Duration, Instant},
};

pub const MIGRATIONS: &[&str] = &[
    include_str!("../migrations/001.sql"),
    include_str!("../migrations/002.sql"),
    include_str!("../migrations/003.sql"),
    include_str!("../migrations/004.sql"),
    include_str!("../migrations/005.sql"),
    include_str!("../migrations/006.sql"),
    include_str!("../migrations/007.sql"),
    include_str!("../migrations/008.sql"),
    include_str!("../migrations/009.sql"),
    include_str!("../migrations/010.sql"),
];

pub struct Database {
    pub path: PathBuf,
    connection: Mutex<Connection>,
    pub(crate) position_searches: crate::engine::SearchFlights,
    pub(crate) practice_candidates: crate::practice::CandidateCache,
}

struct ShortConnection<'a> {
    connection: MutexGuard<'a, Connection>,
}
impl Drop for ShortConnection<'_> {
    fn drop(&mut self) {
        if let Err(error) = self.connection.busy_timeout(Duration::from_secs(60)) {
            tracing::error!(%error,"Could not restore SQLite busy timeout");
        }
    }
}
/// Record an activity event and keep only the newest 5,000.
pub fn insert_event(
    tx: &Transaction<'_>,
    level: &str,
    source: &str,
    message: &str,
    account_id: Option<i64>,
) -> Result<()> {
    tx.execute(
        "INSERT INTO events(ts,level,source,message,account_id) VALUES(?,?,?,?,?)",
        rusqlite::params![crate::now(), level, source, message, account_id],
    )?;
    tx.execute(
        "DELETE FROM events WHERE id < (SELECT MAX(id)-5000 FROM events)",
        [],
    )?;
    Ok(())
}
fn is_busy(error: &anyhow::Error) -> bool {
    error.downcast_ref::<rusqlite::Error>().is_some_and(|e|matches!(e,
        rusqlite::Error::SqliteFailure(code,_) if matches!(code.code,rusqlite::ErrorCode::DatabaseBusy|rusqlite::ErrorCode::DatabaseLocked)))
}

impl Database {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let connection = Connection::open(&path)?;
        let version_table: bool = connection.query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_version')",
            [], |r| r.get(0),
        )?;
        if version_table {
            let version = connection
                .query_row("SELECT version FROM schema_version", [], |r| {
                    r.get::<_, i64>(0)
                })
                .optional()?
                .unwrap_or(0);
            if version < 0 || version as usize > MIGRATIONS.len() {
                bail!("Database schema v{version} requires a newer version of Plyback");
            }
        }
        connection.busy_timeout(Duration::from_secs(60))?;
        connection.execute_batch(
            "PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;
             PRAGMA cache_size=-65536;",
        )?;
        let db = Self {
            path,
            connection: Mutex::new(connection),
            position_searches: crate::engine::SearchFlights::default(),
            practice_candidates: crate::practice::CandidateCache::default(),
        };
        db.migrate()?;
        Ok(db)
    }
    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        self.connection
            .lock()
            .map_err(|_| anyhow::anyhow!("Database lock was poisoned"))
    }
    pub fn migrate(&self) -> Result<()> {
        let mut c = self.lock()?;
        // Check an existing version before any mutations, including journal changes.
        c.execute_batch("CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL)")?;
        let current = c
            .query_row("SELECT version FROM schema_version", [], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?
            .unwrap_or(0);
        if current < 0 || current as usize > MIGRATIONS.len() {
            bail!("Database schema v{current} requires a newer version of Plyback");
        }
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(current as usize) {
            let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
            tx.execute_batch(sql)
                .with_context(|| format!("Schema migration {} failed", i + 1))?;
            if i == 2 {
                let rows = query_connection(
                    &tx,
                    "SELECT rowid,fen_before FROM moves WHERE phase='endgame' AND endgame_type IS NULL",
                    &[],
                )?;
                for row in rows {
                    tx.execute(
                        "UPDATE moves SET endgame_type=? WHERE rowid=?",
                        rusqlite::params![
                            board::endgame_type(
                                row["fen_before"]
                                    .as_str()
                                    .unwrap_or("")
                                    .split(' ')
                                    .next()
                                    .unwrap_or("")
                            ),
                            row["rowid"].as_i64()
                        ],
                    )?;
                }
            }
            tx.execute("DELETE FROM schema_version", [])?;
            tx.execute(
                "INSERT INTO schema_version(version) VALUES(?)",
                [i as i64 + 1],
            )?;
            tx.commit()?;
        }
        Ok(())
    }
    pub fn query(&self, sql: &str, params: &[Value]) -> Result<Vec<Value>> {
        query_connection(&*self.lock()?, sql, params)
    }
    /// Extract owned, typed rows without an intermediate JSON allocation. Keep
    /// this closure limited to database reads; compute after it returns.
    pub fn read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
        f(&*self.lock()?)
    }
    fn try_short_connection(&self) -> Result<Option<ShortConnection<'_>>> {
        let connection = match self.connection.try_lock() {
            Ok(connection) => connection,
            Err(TryLockError::WouldBlock) => return Ok(None),
            Err(TryLockError::Poisoned(_)) => bail!("Database lock was poisoned"),
        };
        connection.busy_timeout(Duration::from_millis(25))?;
        Ok(Some(ShortConnection { connection }))
    }
    /// Optional caches must never delay cancellation behind foreground database work.
    pub fn try_read<T>(&self, f: impl FnOnce(&Connection) -> Result<T>) -> Result<Option<T>> {
        let Some(lease) = self.try_short_connection()? else {
            return Ok(None);
        };
        match f(&lease.connection) {
            Ok(value) => Ok(Some(value)),
            Err(error) if is_busy(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }
    /// Best-effort cache write. Contention is a miss; other failures remain visible.
    pub fn try_transaction<T>(
        &self,
        f: impl FnOnce(&Transaction<'_>) -> Result<T>,
    ) -> Result<Option<T>> {
        let Some(mut lease) = self.try_short_connection()? else {
            return Ok(None);
        };
        let result = (|| {
            let tx = lease
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            let value = f(&tx)?;
            tx.commit()?;
            Ok(value)
        })();
        match result {
            Ok(value) => Ok(Some(value)),
            Err(error) if is_busy(&error) => Ok(None),
            Err(error) => Err(error),
        }
    }
    /// Retry brief lock attempts until cancelled or the deadline expires. The
    /// closure may run again after SQLite contention, so it must only read rows
    /// and return owned values, without external side effects.
    pub fn read_until<T>(
        &self,
        deadline: Instant,
        cancelled: impl Fn() -> bool,
        mut f: impl FnMut(&Connection) -> Result<T>,
    ) -> Result<Option<T>> {
        let value =
            self.retry_until(deadline, &cancelled, |lease| f(&lease.connection).map(Some))?;
        Ok(value.filter(|_| !cancelled()))
    }
    /// Cancellable worker persistence with a deadline for acquiring the lock.
    /// Mutex contention never blocks a thread;
    /// SQLite lock attempts last at most 25ms. A cancelled transaction rolls back
    /// before commit. Callbacks must keep all effects inside this transaction and
    /// check cancellation inside long row loops, since they can be retried.
    pub fn transaction_until<T>(
        &self,
        deadline: Instant,
        cancelled: impl Fn() -> bool,
        mut f: impl FnMut(&Transaction<'_>) -> Result<T>,
    ) -> Result<Option<T>> {
        self.retry_until(deadline, &cancelled, |lease| {
            let tx = lease
                .connection
                .transaction_with_behavior(TransactionBehavior::Immediate)?;
            if cancelled() || Instant::now() >= deadline {
                return Ok(None);
            }
            let value = f(&tx)?;
            if cancelled() {
                return Ok(None);
            }
            tx.commit()?;
            Ok(Some(value))
        })
    }
    /// Run `attempt` on a short lease until it finishes, the deadline passes or the work is
    /// cancelled. Lock contention (a held mutex or a busy SQLite) means try again in 10ms;
    /// `Ok(None)` from the attempt ends the loop without a value.
    fn retry_until<T>(
        &self,
        deadline: Instant,
        cancelled: &impl Fn() -> bool,
        mut attempt: impl FnMut(&mut ShortConnection<'_>) -> Result<Option<T>>,
    ) -> Result<Option<T>> {
        loop {
            if cancelled() || Instant::now() >= deadline {
                return Ok(None);
            }
            if let Some(mut lease) = self.try_short_connection()? {
                match attempt(&mut lease) {
                    Ok(value) => return Ok(value),
                    Err(error) if is_busy(&error) => {}
                    Err(error) => return Err(error),
                }
            }
            std::thread::sleep(
                Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
            );
        }
    }
    pub fn one(&self, sql: &str, params: &[Value]) -> Result<Option<Value>> {
        Ok(self.query(sql, params)?.into_iter().next())
    }
    pub fn scalar(&self, sql: &str, params: &[Value]) -> Result<Value> {
        Ok(self
            .one(sql, params)?
            .and_then(|r| r.as_object().and_then(|m| m.values().next()).cloned())
            .unwrap_or(Value::Null))
    }
    pub fn execute(&self, sql: &str, params: &[Value]) -> Result<usize> {
        self.transaction(|tx| Ok(tx.execute(sql, params_from_iter(sql_values(params)?))?))
    }
    pub fn transaction<T>(&self, f: impl FnOnce(&Transaction<'_>) -> Result<T>) -> Result<T> {
        let mut c = self.lock()?;
        let tx = c.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let result = f(&tx)?;
        tx.commit()?;
        Ok(result)
    }
    pub fn log_event(&self, level: &str, source: &str, message: &str, account_id: Option<i64>) {
        let result = self.transaction(|tx| insert_event(tx, level, source, message, account_id));
        if let Err(error) = result {
            tracing::warn!(%error,"Could not record event");
        }
    }
    /// SQLite's backup API includes committed WAL pages and never opens the source writable.
    pub fn backup(source: &Path, destination: &Path) -> Result<()> {
        if !source.is_file() {
            bail!("Source database does not exist");
        }
        if destination.exists() {
            bail!("Backup destination already exists");
        }
        let parent = destination.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(parent)?;
        let tmp = parent.join(format!(
            ".plyback-backup-{}-{}.db",
            std::process::id(),
            rand::random::<u64>()
        ));
        let result = (|| {
            let reader =
                Connection::open_with_flags(source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            let mut writer = Connection::open(&tmp)?;
            let backup = rusqlite::backup::Backup::new(&reader, &mut writer)?;
            backup.run_to_completion(128, Duration::from_millis(10), None)?;
            drop(backup);
            let check: String = writer.query_row("PRAGMA quick_check", [], |r| r.get(0))?;
            if check != "ok" {
                bail!("Database snapshot failed integrity check");
            }
            drop(writer);
            // Atomic create-if-absent; rename would silently overwrite on Unix.
            std::fs::hard_link(&tmp, destination)?;
            Ok(())
        })();
        let _ = std::fs::remove_file(tmp);
        result
    }
}

pub fn sql_values(values: &[Value]) -> Result<Vec<SqlValue>> {
    values
        .iter()
        .map(|v| {
            Ok(match v {
                Value::Null => SqlValue::Null,
                Value::Bool(b) => SqlValue::Integer(i64::from(*b)),
                Value::Number(n) => {
                    if let Some(i) = n.as_i64() {
                        SqlValue::Integer(i)
                    } else if let Some(f) = n.as_f64() {
                        SqlValue::Real(f)
                    } else {
                        bail!("Numeric SQL parameter is out of range")
                    }
                }
                Value::String(s) => SqlValue::Text(s.clone()),
                _ => bail!("SQL parameters must be scalar values"),
            })
        })
        .collect()
}
pub fn query_connection(c: &Connection, sql: &str, params: &[Value]) -> Result<Vec<Value>> {
    let mut stmt = c.prepare(sql)?;
    let names: Vec<String> = stmt.column_names().map(|s| s.to_string()).collect();
    let mut rows = stmt.query(params_from_iter(sql_values(params)?))?;
    let mut result = Vec::new();
    while let Some(row) = rows.next()? {
        let mut map = Map::new();
        for (i, name) in names.iter().enumerate() {
            let value = match row.get_ref(i)? {
                ValueRef::Null => Value::Null,
                ValueRef::Integer(i) => i.into(),
                ValueRef::Real(f) => serde_json::Number::from_f64(f)
                    .map(Value::Number)
                    .unwrap_or(Value::Null),
                ValueRef::Text(t) => String::from_utf8_lossy(t).into_owned().into(),
                ValueRef::Blob(b) => hex::encode(b).into(),
            };
            map.insert(name.clone(), value);
        }
        result.push(Value::Object(map));
    }
    Ok(result)
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn migrations_and_transaction_failure_are_atomic() {
        let d = Database::open(":memory:").unwrap();
        assert_eq!(
            d.scalar("SELECT version FROM schema_version", &[]).unwrap(),
            json!(MIGRATIONS.len())
        );
        let before = d.scalar("SELECT revision FROM data_revision", &[]).unwrap();
        let err = d.transaction::<()>(|tx| {
            tx.execute(
                "INSERT INTO accounts(username,added_at) VALUES('fixture',0)",
                [],
            )?;
            bail!("abort")
        });
        assert!(err.is_err());
        assert_eq!(
            d.scalar("SELECT COUNT(*) FROM accounts", &[]).unwrap(),
            json!(0)
        );
        assert_eq!(
            d.scalar("SELECT revision FROM data_revision", &[]).unwrap(),
            before
        );
        d.execute("INSERT INTO settings VALUES('after_failure','true')", &[])
            .unwrap();
    }
    #[test]
    fn failed_commit_rolls_back() {
        let d = Database::open(":memory:").unwrap();
        assert!(
            d.transaction(|tx| {
                tx.execute_batch("PRAGMA defer_foreign_keys=ON")?;
                tx.execute("INSERT INTO account_games VALUES(999,999,'white')", [])?;
                Ok(())
            })
            .is_err()
        );
        assert_eq!(
            d.scalar("SELECT COUNT(*) FROM account_games", &[]).unwrap(),
            json!(0)
        );
    }
    #[test]
    fn backup_is_consistent_and_never_overwrites() {
        let t = tempfile::tempdir().unwrap();
        let src = t.path().join("source.db");
        let dst = t.path().join("backup.db");
        let db = Database::open(&src).unwrap();
        db.execute(
            "INSERT INTO accounts(username,added_at) VALUES('fixture',0)",
            &[],
        )
        .unwrap();
        Database::backup(&src, &dst).unwrap();
        assert!(Database::backup(&src, &dst).is_err());
        let copy = Database::open(dst).unwrap();
        assert_eq!(
            copy.scalar("SELECT COUNT(*) FROM accounts", &[]).unwrap(),
            json!(1)
        );
    }

    fn legacy_database(path: &Path, version: usize) -> Connection {
        let c = Connection::open(path).unwrap();
        c.execute_batch(
            "PRAGMA foreign_keys=ON; CREATE TABLE schema_version(version INTEGER NOT NULL)",
        )
        .unwrap();
        for sql in &MIGRATIONS[..version] {
            c.execute_batch(sql).unwrap();
        }
        c.execute("INSERT INTO schema_version VALUES(?)", [version as i64])
            .unwrap();
        c
    }
    #[test]
    fn upgrading_v2_backfills_endgame_and_preserves_existing_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db");
        let legacy = legacy_database(&path, 2);
        legacy
            .execute(
                "INSERT INTO accounts(id,username,added_at) VALUES(7,'fixture',0)",
                [],
            )
            .unwrap();
        legacy
            .execute(
                "INSERT INTO games(id,uuid,pgn,imported_at) VALUES(12,'fixture-id','1. e4 e5 *',0)",
                [],
            )
            .unwrap();
        legacy
            .execute("INSERT INTO account_games VALUES(7,12,'white')", [])
            .unwrap();
        legacy.execute("INSERT INTO moves(game_id,ply,color,san,uci,fen_before,epd_before,phase) VALUES(12,1,'w','Ra2','a1a2','7k/8/8/8/8/8/7K/R7 w - - 0 1','7k/8/8/8/8/8/7K/R7 w - -','endgame')",[]).unwrap();
        drop(legacy);
        let db = Database::open(&path).unwrap();
        assert_eq!(
            db.scalar("SELECT endgame_type FROM moves", &[]).unwrap(),
            "rook"
        );
        assert_eq!(
            db.scalar("SELECT game_id FROM account_games WHERE account_id=7", &[])
                .unwrap(),
            12
        );
        assert_eq!(
            db.scalar("SELECT version FROM schema_version", &[])
                .unwrap(),
            MIGRATIONS.len() as i64
        );
    }
    #[test]
    fn failed_backfill_keeps_version_and_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("backfill.db");
        let legacy = legacy_database(&path, 2);
        legacy.execute_batch("INSERT INTO games(id,uuid,pgn,imported_at) VALUES(12,'preserved','1. e4 *',7);
            INSERT INTO moves(game_id,ply,color,san,uci,fen_before,epd_before,phase) VALUES(12,1,'w','Ra2','a1a2','7k/8/8/8/8/8/7K/R7 w - - 0 1','7k/8/8/8/8/8/7K/R7 w - -','endgame');
            CREATE TRIGGER reject_backfill BEFORE UPDATE ON moves BEGIN SELECT RAISE(ABORT, 'backfill interrupted'); END;").unwrap();
        let rows = |c: &Connection| query_connection(c, "SELECT * FROM moves", &[]).unwrap();
        let before = rows(&legacy);
        drop(legacy);
        let error = Database::open(&path).err().expect("backfill must fail");
        assert!(error.to_string().contains("backfill interrupted"));
        let legacy = Connection::open(&path).unwrap();
        assert_eq!(
            legacy
                .query_row("SELECT version FROM schema_version", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(rows(&legacy), before);
        assert_eq!(
            legacy
                .query_row("SELECT uuid FROM games", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "preserved"
        );
        assert_eq!(
            legacy
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('moves') WHERE name='endgame_type'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        legacy
            .execute_batch("DROP TRIGGER reject_backfill")
            .unwrap();
        drop(legacy);
        let upgraded = Database::open(&path).unwrap();
        assert_eq!(
            upgraded
                .scalar("SELECT version FROM schema_version", &[])
                .unwrap(),
            MIGRATIONS.len() as i64
        );
        assert_eq!(
            upgraded
                .scalar("SELECT endgame_type FROM moves", &[])
                .unwrap(),
            "rook"
        );
    }
    #[test]
    fn upgrading_v5_preserves_password_sessions_and_ready_report_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("legacy.db");
        let c = legacy_database(&path, 5);
        c.execute(
            "INSERT INTO accounts(id,username,added_at) VALUES(7,'fixture',0)",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO settings VALUES('auth_password_hash','scrypt$16384$8$1$salt$digest')",
            [],
        )
        .unwrap();
        c.execute(
            "INSERT INTO sessions VALUES('token-hash',1,99999999999,2,'test')",
            [],
        )
        .unwrap();
        c.execute("INSERT INTO reports(account_id,title,filters_json,status,created_at,report_json) VALUES(7,'Saved','{}','ready',0,'{\"snapshot\": true}')",[]).unwrap();
        drop(c);
        let db = Database::open(path).unwrap();
        assert_eq!(
            db.scalar(
                "SELECT value FROM settings WHERE key='auth_password_hash'",
                &[]
            )
            .unwrap(),
            "scrypt$16384$8$1$salt$digest"
        );
        assert_eq!(
            db.scalar("SELECT token_hash FROM sessions", &[]).unwrap(),
            "token-hash"
        );
        assert_eq!(
            db.scalar("SELECT report_json FROM reports", &[]).unwrap(),
            "{\"snapshot\": true}"
        );
        assert_eq!(
            db.scalar("SELECT scope_frozen FROM reports", &[]).unwrap(),
            0
        );
    }
    #[test]
    fn failed_migration_can_be_retried_without_partial_schema() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("conflict.db");
        let c = legacy_database(&path, 5);
        c.execute_batch("CREATE TABLE report_games(dummy INTEGER)")
            .unwrap();
        drop(c);
        assert!(Database::open(&path).is_err());
        let c = Connection::open(&path).unwrap();
        assert_eq!(
            c.query_row("SELECT version FROM schema_version", [], |r| r
                .get::<_, i64>(0))
                .unwrap(),
            5
        );
        assert_eq!(
            c.query_row(
                "SELECT COUNT(*) FROM pragma_table_info('reports') WHERE name='scope_frozen'",
                [],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
            0
        );
        c.execute_batch("DROP TABLE report_games").unwrap();
        drop(c);
        assert_eq!(
            Database::open(path)
                .unwrap()
                .scalar("SELECT version FROM schema_version", &[])
                .unwrap(),
            MIGRATIONS.len() as i64
        );
    }
    #[test]
    fn future_schema_is_rejected_without_changing_journal_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("future.db");
        let c = legacy_database(&path, 5);
        c.execute("UPDATE schema_version SET version=99", [])
            .unwrap();
        drop(c);
        assert!(Database::open(&path).is_err());
        let c = Connection::open(path).unwrap();
        assert_eq!(
            c.query_row("PRAGMA journal_mode", [], |r| r.get::<_, String>(0))
                .unwrap(),
            "delete"
        );
    }
    #[test]
    fn optional_cache_skips_contention_and_restores_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("busy.db");
        let db = Database::open(&path).unwrap();
        db.read(|_| {
            assert_eq!(db.try_read(|_| Ok(7))?, None);
            assert_eq!(db.try_transaction(|_| Ok(7))?, None);
            Ok(())
        })
        .unwrap();
        let blocker = Connection::open(&path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let started = std::time::Instant::now();
        assert_eq!(
            db.try_transaction(|tx| {
                tx.execute("INSERT INTO settings VALUES('cache','value')", [])?;
                Ok(())
            })
            .unwrap(),
            None
        );
        assert!(started.elapsed() < Duration::from_millis(500));
        assert_eq!(db.scalar("PRAGMA busy_timeout", &[]).unwrap(), 60000);
        blocker.execute_batch("ROLLBACK").unwrap();
        assert!(db.try_transaction::<()>(|_| bail!("abort")).is_err());
        assert_eq!(db.scalar("PRAGMA busy_timeout", &[]).unwrap(), 60000);
        assert_eq!(
            db.try_transaction(|tx| {
                tx.execute("INSERT INTO settings VALUES('cache','value')", [])?;
                Ok(())
            })
            .unwrap(),
            Some(())
        );
    }
    #[test]
    fn worker_deadlines_and_cancellation_do_not_wait_for_database_locks() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker-busy.db");
        let db = Database::open(&path).unwrap();
        // An in-process reader owns the mutex; both APIs must still honor their
        // deadlines without waiting for that mutex or running the callback.
        db.read(|_| {
            let start = Instant::now();
            assert_eq!(
                db.read_until(start + Duration::from_millis(30), || false, |_| Ok(1))?,
                None
            );
            assert_eq!(
                db.transaction_until(
                    Instant::now() + Duration::from_millis(30),
                    || false,
                    |_| Ok(1)
                )?,
                None
            );
            assert!(start.elapsed() < Duration::from_secs(1));
            Ok(())
        })
        .unwrap();
        let blocker = Connection::open(&path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let cancel = AtomicBool::new(false);
        let called = AtomicBool::new(false);
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(50));
                cancel.store(true, Ordering::SeqCst);
            });
            let start = Instant::now();
            assert_eq!(
                db.transaction_until(
                    start + Duration::from_secs(10),
                    || cancel.load(Ordering::SeqCst),
                    |_| {
                        called.store(true, Ordering::SeqCst);
                        Ok(1)
                    }
                )
                .unwrap(),
                None
            );
            assert!(start.elapsed() < Duration::from_secs(1));
        });
        assert!(!called.load(Ordering::SeqCst));
        blocker.execute_batch("ROLLBACK").unwrap();
        assert_eq!(db.scalar("PRAGMA busy_timeout", &[]).unwrap(), 60000);
    }
    #[test]
    fn worker_cancellation_before_commit_rolls_back_every_write() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let db = Database::open(":memory:").unwrap();
        let cancel = AtomicBool::new(false);
        let saved = db
            .transaction_until(
                Instant::now() + Duration::from_secs(1),
                || cancel.load(Ordering::SeqCst),
                |tx| {
                    tx.execute("INSERT INTO settings VALUES('uncommitted','true')", [])?;
                    cancel.store(true, Ordering::SeqCst);
                    Ok(1)
                },
            )
            .unwrap();
        assert_eq!(saved, None);
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM settings WHERE key='uncommitted'", &[])
                .unwrap(),
            0
        );
        assert_eq!(db.scalar("PRAGMA busy_timeout", &[]).unwrap(), 60000);
        assert_eq!(
            db.transaction_until(
                Instant::now() + Duration::from_secs(1),
                || false,
                |tx| {
                    tx.execute("INSERT INTO settings VALUES('committed','true')", [])?;
                    Ok(1)
                }
            )
            .unwrap(),
            Some(1)
        );
    }

    #[test]
    fn revision_triggers_cover_every_cache_source() {
        // Insights, Openings and report caches are keyed on data_revision; every table they
        // read must bump it on insert, update and delete.
        let db = Database::open(":memory:").unwrap();
        let mut names: Vec<String> = db
            .query(
                "SELECT name FROM sqlite_master WHERE type='trigger' AND name LIKE 'revision_%' ORDER BY name",
                &[],
            )
            .unwrap()
            .into_iter()
            .map(|row| row["name"].as_str().unwrap().to_string())
            .collect();
        let mut expected = Vec::new();
        for table in ["games", "account_games", "game_analysis", "accounts"] {
            for event in ["insert", "update", "delete"] {
                expected.push(format!("revision_{table}_{event}"));
            }
        }
        names.sort();
        expected.sort();
        assert_eq!(names, expected);
        let revision = || {
            db.scalar("SELECT revision FROM data_revision WHERE id=1", &[])
                .unwrap()
        };
        let before = revision();
        db.execute(
            "INSERT INTO games(uuid,pgn,imported_at) VALUES('r','1. e4 *',0)",
            &[],
        )
        .unwrap();
        assert_ne!(revision(), before);
    }
}
