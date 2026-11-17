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
    include_str!("../migrations/011.sql"),
    include_str!("../migrations/012.sql"),
    include_str!("../migrations/013.sql"),
    include_str!("../migrations/014.sql"),
];

pub struct Database {
    pub path: PathBuf,
    connection: Mutex<Connection>,
    pub(crate) position_searches: crate::engine::SearchFlights,
    pub(crate) practice_candidates: crate::practice::CandidateCache,
    pub(crate) insight_games: crate::insights::LoadedGamesCache,
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
            insight_games: crate::insights::LoadedGamesCache::default(),
        };
        db.migrate()?;
        Ok(db)
    }
    fn recover_connection<'a>(
        &self,
        connection: MutexGuard<'a, Connection>,
    ) -> Result<MutexGuard<'a, Connection>> {
        // A raw BEGIN or a forgotten Transaction can survive unwinding. Never
        // expose its uncommitted changes to the next request or cache operation.
        if !connection.is_autocommit() {
            connection.execute_batch("ROLLBACK")?;
        }
        connection.busy_timeout(Duration::from_secs(60))?;
        // Keep poison set if cleanup fails, so every reuse retries cleanup.
        self.connection.clear_poison();
        Ok(connection)
    }
    fn lock(&self) -> Result<MutexGuard<'_, Connection>> {
        match self.connection.lock() {
            Ok(connection) => Ok(connection),
            Err(error) => self.recover_connection(error.into_inner()),
        }
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
            // SQLite cannot change foreign_keys inside a transaction. Rebuild
            // the accounts parent without firing child ON DELETE CASCADEs. Decided by
            // the migration's own SQL, not its number, so renumbering cannot let the
            // DROP TABLE cascade-delete every account's links, archives and reports.
            let rebuild_accounts = sql.contains("DROP TABLE accounts;");
            if rebuild_accounts {
                c.pragma_update(None, "foreign_keys", false)?;
            }
            let result = (|| -> Result<()> {
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
                                        .unwrap_or(""),
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
                if rebuild_accounts {
                    // Rebuilding accounts must preserve its references. Other
                    // pre-existing orphan rows are unrelated to this migration.
                    let violation = tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM pragma_foreign_key_check WHERE parent='accounts')",
                        [],
                        |row| row.get::<_, bool>(0),
                    )?;
                    if violation {
                        bail!("Schema migration {} violates foreign keys", i + 1);
                    }
                }
                tx.commit()?;
                Ok(())
            })();
            if rebuild_accounts {
                c.pragma_update(None, "foreign_keys", true)?;
            }
            result?;
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
            Err(TryLockError::Poisoned(error)) => self.recover_connection(error.into_inner())?,
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
    fn panic_with_open_transaction(db: &Database) {
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let connection = db.lock().unwrap();
            connection
                .execute_batch("BEGIN IMMEDIATE; INSERT INTO recovery VALUES(9)")
                .unwrap();
            panic!("interrupted database operation");
        }));
        assert!(panic.is_err());
        assert!(db.connection.is_poisoned());
    }

    #[test]
    fn poisoned_connection_rolls_back_before_blocking_and_short_reads() {
        for short in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let db = Database::open(dir.path().join("recovery.db")).unwrap();
            db.execute("CREATE TABLE recovery(value INTEGER)", &[])
                .unwrap();
            db.execute("INSERT INTO recovery VALUES(7)", &[]).unwrap();
            panic_with_open_transaction(&db);
            let sum = if short {
                db.try_read(|connection| {
                    Ok(
                        connection.query_row("SELECT SUM(value) FROM recovery", [], |row| {
                            row.get::<_, i64>(0)
                        })?,
                    )
                })
                .unwrap()
                .unwrap()
            } else {
                db.scalar("SELECT SUM(value) FROM recovery", &[])
                    .unwrap()
                    .as_i64()
                    .unwrap()
            };
            assert_eq!(sum, 7);
            assert!(!db.connection.is_poisoned());
            db.execute("INSERT INTO recovery VALUES(11)", &[]).unwrap();
            assert_eq!(
                db.scalar("SELECT SUM(value) FROM recovery", &[]).unwrap(),
                18
            );
        }
    }

    #[test]
    fn poisoned_connection_rolls_back_before_a_short_transaction() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("recovery.db")).unwrap();
        db.execute("CREATE TABLE recovery(value INTEGER)", &[])
            .unwrap();
        panic_with_open_transaction(&db);
        assert_eq!(
            db.try_transaction(|tx| Ok(tx.execute("INSERT INTO recovery VALUES(11)", [])?))
                .unwrap(),
            Some(1)
        );
        assert!(!db.connection.is_poisoned());
        assert_eq!(
            db.scalar("SELECT SUM(value) FROM recovery", &[]).unwrap(),
            11
        );
    }

    #[test]
    fn panicked_transaction_rolls_back_and_the_next_query_works() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path().join("recovery.db")).unwrap();
        db.execute("CREATE TABLE recovery(value INTEGER)", &[])
            .unwrap();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _ = db.transaction::<()>(|tx| {
                tx.execute("INSERT INTO recovery VALUES(9)", [])?;
                panic!("interrupted transaction");
            });
        }));
        assert!(panic.is_err());
        assert!(db.connection.is_poisoned());
        assert_eq!(db.scalar("SELECT COUNT(*) FROM recovery", &[]).unwrap(), 0);
        assert!(!db.connection.is_poisoned());
        db.execute("INSERT INTO recovery VALUES(11)", &[]).unwrap();
        assert_eq!(
            db.scalar("SELECT SUM(value) FROM recovery", &[]).unwrap(),
            11
        );
    }

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
    fn upgrading_v11_adds_an_exact_persistent_lichess_creation_cursor() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("schema11.db");
        let snapshot = dir.path().join("schema11-copy.db");
        let legacy = legacy_database(&source, 11);
        legacy.execute("INSERT INTO accounts(id,platform,username,added_at) VALUES(7,'chesscom','fixture',0),(8,'lichess','fixture',0)", []).unwrap();
        legacy.execute("INSERT INTO games(id,uuid,pgn,source,start_time,end_time,imported_at) VALUES(12,'lichess:existing','1. e4 e5 *','lichess',1770000000,1770000060,0)", []).unwrap();
        legacy
            .execute("INSERT INTO account_games VALUES(8,12,'white')", [])
            .unwrap();
        drop(legacy);
        Database::backup(&source, &snapshot).unwrap();
        let db = Database::open(&snapshot).unwrap();
        assert_eq!(
            db.scalar("SELECT version FROM schema_version", &[])
                .unwrap(),
            MIGRATIONS.len()
        );
        assert_eq!(db.scalar("SELECT COUNT(*) FROM accounts", &[]).unwrap(), 2);
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM account_games WHERE account_id=8 AND game_id=12",
                &[]
            )
            .unwrap(),
            1
        );
        assert!(
            db.scalar("SELECT lichess_cursor_ms FROM accounts WHERE id=8", &[])
                .unwrap()
                .is_null()
        );
        assert_eq!(
            db.scalar("SELECT start_time FROM games WHERE id=12", &[])
                .unwrap(),
            1770000000
        );
        assert_eq!(
            db.scalar("SELECT end_time FROM games WHERE id=12", &[])
                .unwrap(),
            1770000060
        );
        db.execute(
            "UPDATE accounts SET lichess_cursor_ms=? WHERE id=8",
            &[json!(1770000000123_i64)],
        )
        .unwrap();
        drop(db);
        let reopened = Database::open(&snapshot).unwrap();
        assert_eq!(
            reopened
                .scalar("SELECT lichess_cursor_ms FROM accounts WHERE id=8", &[])
                .unwrap(),
            1770000000123_i64
        );
        assert!(
            reopened
                .scalar("SELECT lichess_cursor_ms FROM accounts WHERE id=7", &[])
                .unwrap()
                .is_null()
        );
        let original =
            Connection::open_with_flags(&source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        assert_eq!(
            original
                .query_row("SELECT version FROM schema_version", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            11
        );
        assert_eq!(original.query_row("SELECT COUNT(*) FROM pragma_table_info('accounts') WHERE name='lichess_cursor_ms'", [], |r| r.get::<_,i64>(0)).unwrap(), 0);
    }
    #[test]
    fn upgrading_v10_snapshot_preserves_accounts_children_and_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("schema10.db");
        let snapshot = dir.path().join("schema10-copy.db");
        let legacy = legacy_database(&source, 10);
        legacy.execute("INSERT INTO accounts(id,username,display_name,avatar,profile_json,stats_json,profile_fetched_at,added_at,last_sync_at,last_new_game_at,sync_status,sync_error,sync_enabled,auto_analyze,analyze_history,import_since,history_complete) VALUES(7,'Fixture','Display','https://example.test/avatar','{\"profile\": true}','{\"stats\": true}',1,2,3,4,'error','old error',0,0,0,'2026/10',1)", []).unwrap();
        legacy.execute("INSERT INTO games(id,uuid,pgn,imported_at) VALUES(12,'chesscom-existing','1. e4 e5 *',0)", []).unwrap();
        legacy
            .execute("INSERT INTO account_games VALUES(7,12,'white')", [])
            .unwrap();
        legacy.execute("INSERT INTO archives(account_id,period,url) VALUES(7,'2026/10','https://api.chess.com/pub/player/fixture/games/2026/10')", []).unwrap();
        legacy.execute("INSERT INTO reports(id,account_id,filters_json,created_at,status,scope_frozen,report_json) VALUES(4,7,'{}',0,'ready',1,'{\"snapshot\": true}')", []).unwrap();
        legacy
            .execute(
                "INSERT INTO report_games VALUES(4,12,'chesscom-existing')",
                [],
            )
            .unwrap();
        legacy.execute("INSERT INTO practice_items(account_id,game_id,ply,due_at,first_seen_at,last_seen_at) VALUES(7,12,1,10,1,1)", []).unwrap();
        legacy
            .execute(
                "INSERT INTO sync_pending(account_id,game_id,priority) VALUES(7,12,10)",
                [],
            )
            .unwrap();
        legacy.execute("INSERT INTO jobs(game_id,status,priority,created_at,followup_priority,followup_depth) VALUES(12,'running',10,0,20,24)", []).unwrap();
        let before = query_connection(&legacy, "SELECT * FROM accounts", &[]).unwrap()[0].clone();
        let revision = legacy
            .query_row("SELECT revision FROM data_revision", [], |r| {
                r.get::<_, i64>(0)
            })
            .unwrap();
        drop(legacy);
        Database::backup(&source, &snapshot).unwrap();
        let db = Database::open(&snapshot).unwrap();
        let account = db
            .one("SELECT * FROM accounts WHERE id=7", &[])
            .unwrap()
            .unwrap();
        for (key, value) in before.as_object().unwrap() {
            assert_eq!(&account[key], value, "{key}");
        }
        assert_eq!(account["platform"], "chesscom");
        assert_eq!(
            db.query("SELECT account_id,game_id,priority FROM sync_pending", &[])
                .unwrap(),
            vec![json!({"account_id":7,"game_id":12,"priority":10})]
        );
        assert_eq!(
            db.query("SELECT followup_priority,followup_depth FROM jobs", &[])
                .unwrap(),
            vec![json!({"followup_priority":20,"followup_depth":24})]
        );
        assert_eq!(
            db.scalar("SELECT version FROM schema_version", &[])
                .unwrap(),
            14
        );
        for table in [
            "account_games",
            "archives",
            "reports",
            "report_games",
            "practice_items",
            "sync_pending",
        ] {
            assert_eq!(
                db.scalar(&format!("SELECT COUNT(*) FROM {table}"), &[])
                    .unwrap(),
                1,
                "{table}"
            );
        }
        assert_eq!(
            db.scalar("SELECT report_json FROM reports", &[]).unwrap(),
            "{\"snapshot\": true}"
        );
        assert_eq!(
            db.scalar("SELECT revision FROM data_revision", &[])
                .unwrap(),
            revision
        );
        assert_eq!(db.scalar("PRAGMA foreign_keys", &[]).unwrap(), 1);
        assert!(
            db.query("PRAGMA foreign_key_check", &[])
                .unwrap()
                .is_empty()
        );
        assert!(
            db.execute(
                "UPDATE accounts SET sync_generation='different' WHERE id=7",
                &[]
            )
            .is_err()
        );
        db.execute(
            "INSERT INTO accounts(platform,username,added_at) VALUES('lichess','fixture',0)",
            &[],
        )
        .unwrap();
        assert!(
            db.execute(
                "INSERT INTO accounts(platform,username,added_at) VALUES('lichess','FIXTURE',0)",
                &[]
            )
            .is_err()
        );
        assert!(
            db.execute(
                "INSERT INTO accounts(username,added_at) VALUES('FIXTURE',0)",
                &[]
            )
            .is_err()
        );
        assert_eq!(
            db.scalar("SELECT COUNT(DISTINCT sync_generation) FROM accounts", &[])
                .unwrap(),
            2
        );
        let original =
            Connection::open_with_flags(&source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        assert_eq!(
            original
                .query_row("SELECT version FROM schema_version", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            10
        );
        db.execute("DELETE FROM accounts WHERE id=7", &[]).unwrap();
        for table in [
            "account_games",
            "archives",
            "reports",
            "report_games",
            "practice_items",
            "sync_pending",
        ] {
            assert_eq!(
                db.scalar(&format!("SELECT COUNT(*) FROM {table}"), &[])
                    .unwrap(),
                0,
                "{table}"
            );
        }
        assert_eq!(
            db.scalar("SELECT uuid FROM games", &[]).unwrap(),
            "chesscom-existing"
        );
    }
    #[test]
    fn failed_v11_rebuild_preserves_v10_schema_and_account_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schema10-conflict.db");
        let legacy = legacy_database(&path, 10);
        legacy
            .execute(
                "INSERT INTO accounts(id,username,added_at) VALUES(7,'fixture',0)",
                [],
            )
            .unwrap();
        legacy
            .execute_batch("CREATE TABLE accounts_new(conflict INTEGER)")
            .unwrap();
        drop(legacy);
        assert!(Database::open(&path).is_err());
        let legacy = Connection::open(&path).unwrap();
        assert_eq!(
            legacy
                .query_row("SELECT version FROM schema_version", [], |r| r
                    .get::<_, i64>(0))
                .unwrap(),
            10
        );
        assert_eq!(
            legacy
                .query_row("SELECT username FROM accounts WHERE id=7", [], |r| r
                    .get::<_, String>(0))
                .unwrap(),
            "fixture"
        );
        assert_eq!(
            legacy
                .query_row(
                    "SELECT COUNT(*) FROM pragma_table_info('accounts') WHERE name='platform'",
                    [],
                    |r| r.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        legacy.execute_batch("DROP TABLE accounts_new").unwrap();
        drop(legacy);
        assert_eq!(
            Database::open(&path)
                .unwrap()
                .scalar("SELECT platform FROM accounts", &[])
                .unwrap(),
            "chesscom"
        );
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
    fn upgrading_v12_keeps_the_import_cursor_and_persists_slice_checkpoints() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schema12.db");
        let legacy = legacy_database(&path, 12);
        legacy.execute("INSERT INTO accounts(id,username,platform,added_at,lichess_cursor_ms) VALUES(1,'fixture','lichess',0,1770000000123)",[]).unwrap();
        drop(legacy);
        let db = Database::open(&path).unwrap();
        assert_eq!(
            db.scalar("SELECT lichess_cursor_ms FROM accounts", &[])
                .unwrap(),
            1770000000123i64
        );
        for key in [
            "lichess_scan_ms",
            "lichess_pending_ms",
            "lichess_finished_ms",
        ] {
            assert!(
                db.scalar(&format!("SELECT {key} FROM accounts"), &[])
                    .unwrap()
                    .is_null()
            );
        }
        db.execute("UPDATE accounts SET lichess_scan_ms=1770000100456,lichess_pending_ms=1770000000123,lichess_finished_ms=1770000100456",&[]).unwrap();
        drop(db);
        let db = Database::open(&path).unwrap();
        assert_eq!(
            db.scalar("SELECT lichess_scan_ms FROM accounts", &[])
                .unwrap(),
            1770000100456i64
        );
        assert_eq!(
            db.scalar("SELECT lichess_pending_ms FROM accounts", &[])
                .unwrap(),
            1770000000123i64
        );
        assert_eq!(
            db.scalar("SELECT lichess_finished_ms FROM accounts", &[])
                .unwrap(),
            1770000100456i64
        );
    }

    #[test]
    fn upgrading_v13_discovers_pending_ids_without_changing_game_or_report_data() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("schema13.db");
        let snapshot = dir.path().join("schema13-copy.db");
        let legacy = legacy_database(&source, 13);
        legacy.execute_batch(
            r#"INSERT INTO accounts(id,platform,username,added_at,lichess_cursor_ms,lichess_scan_ms,lichess_pending_ms,lichess_finished_ms) VALUES
             (1,'lichess','fixture1',0,1770000300456,1770000400789,1770000000123,1770000300456),
             (2,'lichess','fixture2',0,1770000000999,1770000400789,1770000100456,1770000300456),
             (3,'lichess','fixture3',0,NULL,1770000400789,1770000050123,1770000300456),
             (4,'chesscom','fixture1',0,42,43,44,45),
             (5,'lichess','fixture5',0,1770000300456,1770000400789,NULL,1770000300456);
             INSERT INTO games(id,uuid,pgn,source,start_time,end_time,imported_at) VALUES
             (12,'lichess:fixture','1. e4 e5 *','lichess',1770000000,1770000060,0);
             INSERT INTO account_games VALUES(1,12,'white');
             INSERT INTO archives(account_id,period,url) VALUES(1,'2026/10','https://example.test/archive');
             INSERT INTO reports(id,account_id,filters_json,created_at,status,scope_frozen,report_json)
             VALUES(4,1,'{}',0,'ready',1,'{"snapshot":{"games":[12],"source":"fixture"}}');
             INSERT INTO report_games VALUES(4,12,'lichess:fixture');"#,
        ).unwrap();
        let before_accounts =
            query_connection(&legacy, "SELECT * FROM accounts ORDER BY id", &[]).unwrap();
        let before_games =
            query_connection(&legacy, "SELECT * FROM games ORDER BY id", &[]).unwrap();
        let before_reports =
            query_connection(&legacy, "SELECT * FROM reports ORDER BY id", &[]).unwrap();
        let before_archives =
            query_connection(&legacy, "SELECT * FROM archives ORDER BY account_id", &[]).unwrap();
        drop(legacy);
        Database::backup(&source, &snapshot).unwrap();
        let db = Database::open(&snapshot).unwrap();
        assert_eq!(
            db.scalar("SELECT version FROM schema_version", &[])
                .unwrap(),
            MIGRATIONS.len()
        );
        let accounts = db.query("SELECT * FROM accounts ORDER BY id", &[]).unwrap();
        for (index, original) in before_accounts.iter().enumerate() {
            let mut expected = original.clone();
            if expected["platform"] == "lichess" {
                expected["lichess_cursor_ms"] = json!(match index {
                    0 => 1770000000123i64,
                    1 => 1770000000999i64,
                    2 => 1770000050123i64,
                    4 => 1770000300456i64,
                    _ => unreachable!(),
                });
                for key in [
                    "lichess_scan_ms",
                    "lichess_pending_ms",
                    "lichess_finished_ms",
                ] {
                    expected[key] = Value::Null;
                }
            }
            assert_eq!(accounts[index], expected);
        }
        assert_eq!(
            db.query("SELECT * FROM games ORDER BY id", &[]).unwrap(),
            before_games
        );
        assert_eq!(
            db.query("SELECT * FROM reports ORDER BY id", &[]).unwrap(),
            before_reports
        );
        assert_eq!(
            db.query("SELECT * FROM archives ORDER BY account_id", &[])
                .unwrap(),
            before_archives
        );
        assert_eq!(db.scalar("SELECT COUNT(*) FROM report_games WHERE report_id=4 AND game_id=12 AND game_uuid='lichess:fixture'", &[]).unwrap(), 1);
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM account_games WHERE account_id=1 AND game_id=12",
                &[]
            )
            .unwrap(),
            1
        );
        assert_eq!(db.scalar("SELECT COUNT(*) FROM games WHERE typeof(start_time)='integer' AND typeof(end_time)='integer'", &[]).unwrap(), 1);
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM lichess_pending_games", &[])
                .unwrap(),
            0
        );
        assert_eq!(db.scalar("SELECT COUNT(*) FROM sqlite_master WHERE type='index' AND name='lichess_pending_poll'", &[]).unwrap(), 1);

        let revision = db
            .scalar("SELECT revision FROM data_revision", &[])
            .unwrap();
        db.execute("INSERT INTO lichess_pending_games(account_id,game_id,created_at_ms) VALUES(1,'oldgame',1770000000123),(2,'oldgame',1770000000999)", &[]).unwrap();
        assert_eq!(
            db.scalar(
                "SELECT checked_at FROM lichess_pending_games WHERE account_id=1",
                &[]
            )
            .unwrap(),
            0
        );
        assert_eq!(
            db.scalar(
                "SELECT misses FROM lichess_pending_games WHERE account_id=1",
                &[]
            )
            .unwrap(),
            0
        );
        assert!(
            db.execute(
                "UPDATE lichess_pending_games SET misses=NULL WHERE account_id=1",
                &[]
            )
            .is_err()
        );
        assert!(
            db.execute(
                "UPDATE lichess_pending_games SET misses=-1 WHERE account_id=1",
                &[]
            )
            .is_err()
        );
        assert!(db.execute("INSERT INTO lichess_pending_games(account_id,game_id,created_at_ms) VALUES(1,'oldgame',0)", &[]).is_err());
        assert!(db.execute("INSERT INTO lichess_pending_games(account_id,game_id,created_at_ms) VALUES(999,'invalid',0)", &[]).is_err());
        db.execute("INSERT INTO lichess_pending_games(account_id,game_id,created_at_ms,checked_at) VALUES(1,'latergame',1770000100456,5)", &[]).unwrap();
        let pending = db.query("SELECT game_id FROM lichess_pending_games WHERE account_id=1 ORDER BY checked_at,game_id LIMIT 300", &[]).unwrap();
        assert_eq!(
            pending
                .iter()
                .map(|row| row["game_id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["oldgame", "latergame"]
        );
        db.execute("UPDATE lichess_pending_games SET checked_at=10,misses=2 WHERE account_id=1 AND game_id='oldgame'", &[]).unwrap();
        db.execute(
            "DELETE FROM lichess_pending_games WHERE game_id='latergame'",
            &[],
        )
        .unwrap();
        assert_eq!(
            db.scalar("SELECT revision FROM data_revision", &[])
                .unwrap(),
            revision
        );
        assert_eq!(db.scalar("PRAGMA foreign_keys", &[]).unwrap(), 1);
        db.execute("DELETE FROM accounts WHERE id=2", &[]).unwrap();
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM lichess_pending_games WHERE account_id=2",
                &[]
            )
            .unwrap(),
            0
        );
        drop(db);
        let db = Database::open(&snapshot).unwrap();
        assert_eq!(
            db.scalar(
                "SELECT created_at_ms FROM lichess_pending_games WHERE account_id=1",
                &[]
            )
            .unwrap(),
            1770000000123i64
        );
        assert_eq!(
            db.scalar(
                "SELECT checked_at FROM lichess_pending_games WHERE account_id=1",
                &[]
            )
            .unwrap(),
            10
        );
        assert_eq!(
            db.scalar(
                "SELECT misses FROM lichess_pending_games WHERE account_id=1",
                &[]
            )
            .unwrap(),
            2
        );
        assert_eq!(
            db.query("SELECT * FROM reports ORDER BY id", &[]).unwrap(),
            before_reports
        );
        assert!(
            db.query("PRAGMA foreign_key_check", &[])
                .unwrap()
                .is_empty()
        );

        let original =
            Connection::open_with_flags(&source, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        assert_eq!(
            original
                .query_row("SELECT version FROM schema_version", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            13
        );
        assert_eq!(
            original
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE name='lichess_pending_games'",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            0
        );
        assert_eq!(
            original
                .query_row(
                    "SELECT lichess_cursor_ms FROM accounts WHERE id=1",
                    [],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1770000300456i64
        );
    }

    #[test]
    fn rebuilding_accounts_preserves_unrelated_orphans_without_blocking_upgrade() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schema9-unrelated-orphan.db");
        let legacy = legacy_database(&path, 9);
        legacy
            .execute(
                "INSERT INTO accounts(id,username,added_at) VALUES(7,'fixture',0)",
                [],
            )
            .unwrap();
        legacy.execute_batch("PRAGMA foreign_keys=OFF; INSERT INTO account_games VALUES(7,999,'white'); PRAGMA foreign_keys=ON").unwrap();
        assert_eq!(
            legacy
                .query_row("SELECT parent FROM pragma_foreign_key_check", [], |r| {
                    r.get::<_, String>(0)
                })
                .unwrap(),
            "games"
        );
        drop(legacy);
        let db = Database::open(&path).unwrap();
        assert_eq!(
            db.scalar("SELECT version FROM schema_version", &[])
                .unwrap(),
            MIGRATIONS.len()
        );
        assert_eq!(
            db.scalar("SELECT platform FROM accounts WHERE id=7", &[])
                .unwrap(),
            "chesscom"
        );
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM account_games WHERE account_id=7 AND game_id=999",
                &[]
            )
            .unwrap(),
            1
        );
        let orphans = db.query("PRAGMA foreign_key_check", &[]).unwrap();
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0]["parent"], "games");
        assert_eq!(db.scalar("PRAGMA foreign_keys", &[]).unwrap(), 1);
        assert!(db.execute("INSERT INTO archives(account_id,period,url) VALUES(999,'2026/10','https://example.test/archive')", &[]).is_err());
    }
    #[test]
    fn rebuilding_accounts_rejects_account_orphans_and_restores_foreign_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("schema9-account-orphan.db");
        let legacy = legacy_database(&path, 9);
        legacy
            .execute(
                "INSERT INTO accounts(id,username,added_at) VALUES(7,'fixture',0)",
                [],
            )
            .unwrap();
        legacy.execute_batch("PRAGMA foreign_keys=OFF; INSERT INTO archives(account_id,period,url) VALUES(999,'2026/10','https://example.test/archive'); PRAGMA foreign_keys=ON").unwrap();
        let db = Database {
            path,
            connection: Mutex::new(legacy),
            position_searches: crate::engine::SearchFlights::default(),
            practice_candidates: crate::practice::CandidateCache::default(),
            insight_games: crate::insights::LoadedGamesCache::default(),
        };
        assert!(
            db.migrate()
                .unwrap_err()
                .to_string()
                .contains("violates foreign keys")
        );
        assert_eq!(db.scalar("PRAGMA foreign_keys", &[]).unwrap(), 1);
        // Main migration 010 commits before the accounts rebuild in 011 fails.
        assert_eq!(
            db.scalar("SELECT version FROM schema_version", &[])
                .unwrap(),
            10
        );
        assert_eq!(
            db.scalar("SELECT username FROM accounts WHERE id=7", &[])
                .unwrap(),
            "fixture"
        );
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM archives WHERE account_id=999", &[])
                .unwrap(),
            1
        );
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM pragma_table_info('accounts') WHERE name='platform'",
                &[]
            )
            .unwrap(),
            0
        );
        db.execute("DELETE FROM archives WHERE account_id=999", &[])
            .unwrap();
        db.migrate().unwrap();
        assert_eq!(
            db.scalar("SELECT version FROM schema_version", &[])
                .unwrap(),
            MIGRATIONS.len()
        );
        assert_eq!(db.scalar("PRAGMA foreign_keys", &[]).unwrap(), 1);
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
