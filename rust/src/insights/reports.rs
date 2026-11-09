use super::compute::{compute, compute_scope, normalize_filters, where_clause};
use crate::{ApiError, ApiResult, Query, api_error, db::Database, now, services::Services};
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::sync::{
    Arc, Condvar, Mutex, OnceLock, Weak,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const CACHE_ENTRIES: usize = 64;
const CACHE_BYTES: usize = 32 * 1024 * 1024;
const REFRESH_WORKERS: usize = 2;
const STALE_FOR: i64 = 120;

struct Entry {
    revision: i64,
    data: Value,
    used: u64,
    bytes: usize,
}
#[derive(Default)]
struct Cache {
    entries: HashMap<String, Entry>,
    pending: HashSet<String>,
    clock: u64,
    bytes: usize,
}

/// All caches and work queues belong to one application/database instance.
pub struct Insights {
    db: Arc<Database>,
    services: Arc<Services>,
    cache: Mutex<Cache>,
    cache_changed: Condvar,
    stop: AtomicBool,
    wake: (Mutex<bool>, Condvar),
    worker: Mutex<Option<JoinHandle<()>>>,
    refreshes: Mutex<Vec<JoinHandle<()>>>,
    build: Mutex<()>,
    self_ref: OnceLock<Weak<Self>>,
}

impl Insights {
    pub fn new(db: Arc<Database>, services: Arc<Services>) -> Arc<Self> {
        let out = Arc::new(Self {
            db,
            services,
            cache: Mutex::new(Cache::default()),
            cache_changed: Condvar::new(),
            stop: AtomicBool::new(false),
            wake: (Mutex::new(false), Condvar::new()),
            worker: Mutex::new(None),
            refreshes: Mutex::new(Vec::new()),
            build: Mutex::new(()),
            self_ref: OnceLock::new(),
        });
        let _ = out.self_ref.set(Arc::downgrade(&out));
        out
    }
    pub fn start(self: &Arc<Self>) {
        let mut worker = self.worker.lock().unwrap_or_else(|e| e.into_inner());
        if worker.is_some() {
            return;
        }
        self.stop.store(false, Ordering::Release);
        let this = Arc::clone(self);
        *worker = Some(std::thread::spawn(move || {
            while !this.stop.load(Ordering::Acquire) {
                if let Err(e) = this.tick() {
                    tracing::error!(error=%e,"Report worker failed");
                }
                let flag = this.wake.0.lock().unwrap_or_else(|e| e.into_inner());
                let (mut flag, _) = this
                    .wake
                    .1
                    .wait_timeout_while(flag, Duration::from_secs(4), |flag| {
                        !*flag && !this.stop.load(Ordering::Acquire)
                    })
                    .unwrap_or_else(|e| e.into_inner());
                *flag = false;
            }
        }));
    }
    /// Signal shutdown without waiting for report or cache work to finish.
    /// The HTTP server calls this before draining in-flight requests.
    pub fn request_stop(&self) {
        self.stop.store(true, Ordering::Release);
        *self.wake.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.wake.1.notify_all();
        self.cache_changed.notify_all();
    }
    pub fn stop(&self) {
        self.request_stop();
        if let Some(worker) = self.worker.lock().unwrap_or_else(|e| e.into_inner()).take() {
            let _ = worker.join();
        }
        for worker in self
            .refreshes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
        {
            let _ = worker.join();
        }
    }
    fn notify(&self) {
        *self.wake.0.lock().unwrap_or_else(|e| e.into_inner()) = true;
        self.wake.1.notify_one();
    }
    fn account(&self, id: i64) -> Result<()> {
        if self
            .db
            .one("SELECT id FROM accounts WHERE id=?", &[json!(id)])?
            .is_none()
        {
            Err(api_error(404, "Account not found"))
        } else {
            Ok(())
        }
    }
    fn get_report(&self, id: i64) -> Result<Value> {
        self.db
            .one("SELECT * FROM reports WHERE id=?", &[json!(id)])?
            .ok_or_else(|| api_error(404, "Report not found"))
    }

    fn cached(&self, account: i64, filters: &Value) -> Result<Value> {
        let key = format!("{account}:{}", serde_json::to_string(filters)?);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            if self.stop.load(Ordering::Acquire) {
                return Err(api_error(503, "Plyback is shutting down."));
            }
            let revision = self
                .db
                .scalar("SELECT revision FROM data_revision WHERE id=1", &[])?
                .as_i64()
                .unwrap_or(0);
            let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
            cache.clock += 1;
            let clock = cache.clock;
            let mut stale = None;
            if let Some(entry) = cache.entries.get_mut(&key) {
                entry.used = clock;
                if entry.revision == revision {
                    return Ok(entry.data.clone());
                }
                if now() - entry.data["generated_at"].as_i64().unwrap_or(0) < STALE_FOR {
                    let mut data = entry.data.clone();
                    data["stale"] = json!(true);
                    stale = Some(data);
                }
            }
            if cache.pending.contains(&key) || cache.pending.len() >= REFRESH_WORKERS {
                if let Some(data) = stale {
                    return Ok(data);
                }
                if Instant::now() >= deadline {
                    return Err(api_error(503, "Insights are busy. Try again shortly."));
                }
                let _guard = self
                    .cache_changed
                    .wait_timeout(cache, Duration::from_millis(250))
                    .unwrap_or_else(|e| e.into_inner());
                continue;
            }
            cache.pending.insert(key.clone());
            drop(cache);
            if let Some(data) = stale
                && let Some(this) = self.self_ref.get().and_then(Weak::upgrade)
            {
                let f = filters.clone();
                let key = key.clone();
                let mut workers = self.refreshes.lock().unwrap_or_else(|e| e.into_inner());
                let mut i = 0;
                while i < workers.len() {
                    if workers[i].is_finished() {
                        let _ = workers.swap_remove(i).join();
                    } else {
                        i += 1;
                    }
                }
                workers.push(std::thread::spawn(move || {
                    if let Err(e) = this.refresh(key, revision, account, &f) {
                        tracing::warn!(error=%e,"Insight refresh failed");
                    }
                }));
                return Ok(data);
            }
            return self.refresh(key, revision, account, filters);
        }
    }
    fn refresh(&self, key: String, revision: i64, account: i64, filters: &Value) -> Result<Value> {
        let result = compute(&self.db, account, filters).map(|mut data| {
            data["generated_at"] = json!(now());
            data
        });
        let mut cache = self.cache.lock().unwrap_or_else(|e| e.into_inner());
        cache.pending.remove(&key);
        if let Ok(data) = &result {
            let bytes = serde_json::to_vec(data)
                .map(|v| v.len())
                .unwrap_or(CACHE_BYTES + 1);
            if bytes <= CACHE_BYTES {
                if let Some(old) = cache.entries.remove(&key) {
                    cache.bytes = cache.bytes.saturating_sub(old.bytes);
                }
                cache.clock += 1;
                let used = cache.clock;
                cache.bytes += bytes;
                cache.entries.insert(
                    key,
                    Entry {
                        revision,
                        data: data.clone(),
                        used,
                        bytes,
                    },
                );
                while cache.entries.len() > CACHE_ENTRIES || cache.bytes > CACHE_BYTES {
                    let victim = cache
                        .entries
                        .iter()
                        .min_by_key(|(_, e)| e.used)
                        .map(|(k, _)| k.clone());
                    if let Some(k) = victim {
                        if let Some(old) = cache.entries.remove(&k) {
                            cache.bytes = cache.bytes.saturating_sub(old.bytes);
                        }
                    } else {
                        break;
                    }
                }
            }
        }
        self.cache_changed.notify_all();
        result
    }

    /// Freeze membership once; ready historical reports are never recalculated.
    fn freeze(&self, report: &Value) -> Result<()> {
        if report["status"] == "ready" || report["scope_frozen"].as_i64() == Some(1) {
            return Ok(());
        }
        let rid = report["id"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("Invalid report ID"))?;
        let account = report["account_id"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("Invalid account ID"))?;
        let filters = normalize_filters(&decode(&report["filters_json"], json!({})))?;
        let (clauses, params) = where_clause(account, &filters);
        self.db.transaction(|tx|{
            let freeze:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM reports WHERE id=? AND status!='ready' AND scope_frozen=0)",[rid],|r|r.get(0))?;
            if !freeze{return Ok(())}
            let mut bind=vec![rusqlite::types::Value::Integer(rid)];bind.extend(params.iter().map(sql_value));
            tx.execute(&format!("INSERT OR IGNORE INTO report_games (report_id,game_id,game_uuid) SELECT ?,g.id,g.uuid FROM account_games ag JOIN games g ON g.id=ag.game_id WHERE {}",clauses.join(" AND ")),rusqlite::params_from_iter(bind))?;
            tx.execute("UPDATE reports SET scope_frozen=1 WHERE id=? AND status!='ready'",[rid])?;Ok(())
        })
    }
    // A game still queued or running (e.g. re-analysed at the report's depth) is not done yet,
    // even if an older, shallower analysis exists; otherwise the bar reads complete too early.
    fn progress(&self, rid: i64) -> Result<(usize, usize, usize)> {
        let row=self.db.one("SELECT COUNT(*) AS total, COALESCE(SUM(EXISTS(SELECT 1 FROM game_analysis ga WHERE ga.game_id=g.id) AND NOT EXISTS(SELECT 1 FROM jobs j WHERE j.game_id=g.id AND j.status IN ('queued','running'))),0) AS analyzed,COALESCE(SUM(EXISTS(SELECT 1 FROM jobs j WHERE j.game_id=g.id AND j.status IN ('queued','running'))),0) AS pending FROM report_games rg JOIN games g ON g.id=rg.game_id AND g.uuid=rg.game_uuid WHERE rg.report_id=? AND g.rules IN ('chess','chess960')",&[json!(rid)])?.unwrap_or_else(||json!({}));
        Ok((
            row["total"].as_u64().unwrap_or(0) as usize,
            row["analyzed"].as_u64().unwrap_or(0) as usize,
            row["pending"].as_u64().unwrap_or(0) as usize,
        ))
    }
    fn create(&self, account: i64, filters: Value, title: String) -> Result<Value> {
        let _guard = self.build.lock().unwrap_or_else(|e| e.into_inner());
        let (clauses, params) = where_clause(account, &filters);
        let rid=self.db.transaction(|tx|{
            tx.execute("INSERT INTO reports (account_id,title,filters_json,status,total_games,created_at,scope_frozen) VALUES (?,?,?,'analyzing',0,?,1)",rusqlite::params![account,title,filters.to_string(),now()])?;
            let rid=tx.last_insert_rowid();let mut bind=vec![rusqlite::types::Value::Integer(rid)];bind.extend(params.iter().map(sql_value));
            tx.execute(&format!("INSERT INTO report_games (report_id,game_id,game_uuid) SELECT ?,g.id,g.uuid FROM account_games ag JOIN games g ON g.id=ag.game_id WHERE {}",clauses.join(" AND ")),rusqlite::params_from_iter(bind))?;
            tx.execute("UPDATE reports SET total_games=(SELECT COUNT(*) FROM report_games rg JOIN games g ON g.id=rg.game_id AND g.uuid=rg.game_uuid WHERE rg.report_id=? AND g.rules IN ('chess','chess960')) WHERE id=?",[rid,rid])?;Ok(rid)
        })?;
        let ids=self.db.query("SELECT g.id FROM report_games rg JOIN games g ON g.id=rg.game_id AND g.uuid=rg.game_uuid WHERE rg.report_id=? AND g.rules IN ('chess','chess960')",&[json!(rid)])?.iter().filter_map(|r|r["id"].as_i64()).collect::<Vec<_>>();
        if let Err(e) = self
            .services
            .enqueue(&ids, crate::services::PRIORITY_REPORT, None, false)
        {
            self.db.execute(
                "UPDATE reports SET status='failed',error=? WHERE id=?",
                &[
                    json!("Could not queue report analysis. Retry the report."),
                    json!(rid),
                ],
            )?;
            return Err(e);
        }
        self.notify();
        Ok(public(&self.get_report(rid)?, false))
    }
    fn build_snapshot(&self, rid: i64) -> Result<()> {
        let rep = self.get_report(rid)?;
        if rep["status"] == "ready" {
            return Ok(());
        }
        self.freeze(&rep)?;
        let account = rep["account_id"]
            .as_i64()
            .ok_or_else(|| anyhow::anyhow!("Invalid report account"))?;
        for _ in 0..5 {
            let revision = self
                .db
                .scalar("SELECT revision FROM data_revision WHERE id=1", &[])?
                .as_i64()
                .unwrap_or(0);
            let mut data = compute_scope(
                &self.db,
                account,
                &decode(&rep["filters_json"], json!({})),
                Some(rid),
            )?;
            if let Some(quality) = data["data_quality"].as_object_mut() {
                quality.remove("unanalyzed_ids");
            }
            let (total, analyzed, _) = self.progress(rid)?;
            // Publish the result and its counters only if every read came from
            // the same committed analysis generation. Computation holds no lock.
            let published=self.db.transaction(|tx|{
                let current:i64=tx.query_row("SELECT revision FROM data_revision WHERE id=1",[],|r|r.get(0))?;
                if current!=revision{return Ok(false)}
                let n=tx.execute("UPDATE reports SET status='ready',completed_at=?,report_json=?,total_games=?,analyzed_games=?,error=NULL WHERE id=? AND status!='ready'",rusqlite::params![now(),data.to_string(),total as i64,analyzed as i64,rid])?;
                if n>0{tx.execute("INSERT INTO events (ts,level,source,message,account_id) VALUES (?,'info','reports',?,?)",rusqlite::params![now(),format!("Report #{rid} is ready ({analyzed}/{total} games analysed)"),account])?;}
                Ok(true)
            })?;
            if published {
                return Ok(());
            }
        }
        Err(api_error(
            503,
            "Analysis data is changing; retry report finalization shortly.",
        ))
    }
    pub fn tick(&self) -> Result<()> {
        let _guard = self.build.lock().unwrap_or_else(|e| e.into_inner());
        for rep in self
            .db
            .query("SELECT * FROM reports WHERE status='analyzing'", &[])?
        {
            let rid = rep["id"].as_i64().unwrap_or(0);
            // An error on one report must not hold back the others; it is retried next tick.
            if let Err(e) = self.tick_one(&rep, rid) {
                tracing::warn!(report_id=rid,error=%e,"Report progress update failed");
            }
        }
        Ok(())
    }
    fn tick_one(&self, rep: &Value, rid: i64) -> Result<()> {
        self.freeze(rep)?;
        let (total, analyzed, pending) = self.progress(rid)?;
        self.db.execute(
            "UPDATE reports SET total_games=?,analyzed_games=? WHERE id=? AND status='analyzing'",
            &[json!(total), json!(analyzed), json!(rid)],
        )?;
        if pending == 0
            && let Err(e) = self.build_snapshot(rid)
        {
            // Analysis rows are still changing: try again on the next tick.
            if e.downcast_ref::<ApiError>().is_some_and(|e| e.0 == 503) {
                return Ok(());
            }
            tracing::error!(report_id=rid,error=%e,"Report snapshot failed");
            self.db.execute(
                "UPDATE reports SET status='failed',error=? WHERE id=? AND status!='ready'",
                &[
                    json!("Could not build this report. Check the server log and try again."),
                    json!(rid),
                ],
            )?;
        }
        Ok(())
    }

    pub fn handle(&self, method: &str, path: &str, q: &Query, body: &Value) -> Option<ApiResult> {
        let parts = path.trim_matches('/').split('/').collect::<Vec<_>>();
        if parts.len() == 4
            && parts[0] == "api"
            && parts[1] == "accounts"
            && ["insights", "reports"].contains(&parts[3])
        {
            let result = (|| {
                let id = parts[2]
                    .parse::<i64>()
                    .map_err(|_| api_error(422, "account_id must be an integer"))?;
                self.account(id)?;
                match (method,parts[3]) {
                    ("GET","insights")=>{let raw=q.iter().map(|(k,v)|(k.clone(),json!(v))).collect::<serde_json::Map<_,_>>();let f=normalize_filters(&json!(raw))?;Ok((200,self.cached(id,&f)?))},
                    ("GET","reports")=>Ok((200,json!(self.db.query("SELECT * FROM reports WHERE account_id=? ORDER BY created_at DESC,id DESC",&[json!(id)])?.iter().map(|r|public(r,false)).collect::<Vec<_>>()))),
                    ("POST","reports")=>{if !body.is_object(){return Err(api_error(422,"Expected a report object"))}let f=normalize_filters(&body["filters"])?;let title=report_title(&body["title"],&f)?;Ok((201,self.create(id,f,title)?))},
                    _=>Err(api_error(405,"Method not allowed")),
                }
            })();
            return Some(result);
        }
        if (parts.len() == 3 || parts.len() == 4) && parts[0] == "api" && parts[1] == "reports" {
            let result = (|| {
                let id = parts[2]
                    .parse::<i64>()
                    .map_err(|_| api_error(422, "report_id must be an integer"))?;
                match (method, parts.get(3).copied()) {
                    ("GET", None) => Ok((200, public(&self.get_report(id)?, true))),
                    ("DELETE", None) => {
                        if self
                            .db
                            .execute("DELETE FROM reports WHERE id=?", &[json!(id)])?
                            == 0
                        {
                            return Err(api_error(404, "Report not found"));
                        }
                        Ok((200, json!({"deleted":true})))
                    }
                    ("POST", Some("finalize")) => {
                        let _guard = self.build.lock().unwrap_or_else(|e| e.into_inner());
                        self.build_snapshot(id)?;
                        Ok((200, public(&self.get_report(id)?, false)))
                    }
                    _ => Err(api_error(404, "Not found")),
                }
            })();
            return Some(result);
        }
        None
    }
}

fn sql_value(v: &Value) -> rusqlite::types::Value {
    match v {
        Value::Null => rusqlite::types::Value::Null,
        Value::Bool(v) => rusqlite::types::Value::Integer(i64::from(*v)),
        Value::Number(n) => n
            .as_i64()
            .map(rusqlite::types::Value::Integer)
            .unwrap_or_else(|| rusqlite::types::Value::Real(n.as_f64().unwrap_or(0.0))),
        Value::String(s) => rusqlite::types::Value::Text(s.clone()),
        _ => rusqlite::types::Value::Text(v.to_string()),
    }
}
fn decode(v: &Value, default: Value) -> Value {
    v.as_str()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(default)
}
fn public(r: &Value, full: bool) -> Value {
    let mut out = json!({});
    for k in [
        "id",
        "account_id",
        "title",
        "status",
        "total_games",
        "analyzed_games",
        "created_at",
        "completed_at",
        "error",
    ] {
        out[k] = r[k].clone();
    }
    out["filters"] = decode(&r["filters_json"], json!({}));
    if full {
        out["report"] = decode(&r["report_json"], Value::Null);
    }
    out
}
/// Longest report title, in characters (the Reports page input uses the same limit).
const MAX_TITLE_CHARS: usize = 120;

/// The title to save: trimmed, and the default for a missing or blank one.
fn report_title(title: &Value, f: &Value) -> Result<String> {
    if title.is_null() {
        return Ok(default_title(f));
    }
    let title = title
        .as_str()
        .map(str::trim)
        .filter(|s| s.chars().count() <= MAX_TITLE_CHARS)
        .ok_or_else(|| {
            api_error(
                422,
                format!("title must be text of at most {MAX_TITLE_CHARS} characters"),
            )
        })?;
    Ok(if title.is_empty() {
        default_title(f)
    } else {
        title.to_string()
    })
}

fn default_title(f: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(a) = f["time_classes"].as_array() {
        parts.push(
            a.iter()
                .filter_map(Value::as_str)
                .map(|s| {
                    let mut c = s.chars();
                    c.next()
                        .map(|a| a.to_uppercase().collect::<String>() + c.as_str())
                        .unwrap_or_default()
                })
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    if let Some(since) = f["since"].as_i64() {
        let date = chrono::DateTime::from_timestamp(since, 0)
            .map(|d| d.format("%b %Y").to_string())
            .unwrap_or_default();
        parts.push(format!("since {date}"));
    } else {
        parts.push("all time".into());
    }
    if let Some(c) = f["color"].as_str() {
        parts.push(format!("as {c}"));
    }
    format!("Account analysis — {}", parts.join(" "))
}
