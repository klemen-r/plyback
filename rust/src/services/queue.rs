use super::*;
use crate::{
    ANALYSIS_VERSION,
    analysis::{self, AnalysisOptions},
    db::query_connection,
    engine::Engine,
    now,
};
use rusqlite::OptionalExtension;
use std::time::Instant;

pub(super) struct PendingRelease {
    id: i64,
    token: String,
    error: Option<String>,
}

impl Services {
    pub fn enqueue(
        &self,
        ids: &[i64],
        priority: i64,
        depth: Option<i64>,
        force: bool,
    ) -> Result<usize> {
        self.enqueue_inner(ids, priority, depth, force, None)
    }
    pub(super) fn enqueue_for_account(
        &self,
        account_id: i64,
        sync_generation: &str,
        ids: &[i64],
        priority: i64,
    ) -> Result<usize> {
        self.enqueue_inner(
            ids,
            priority,
            None,
            false,
            Some((account_id, sync_generation)),
        )
    }
    fn enqueue_inner(
        &self,
        ids: &[i64],
        priority: i64,
        depth: Option<i64>,
        force: bool,
        account: Option<(i64, &str)>,
    ) -> Result<usize> {
        let settings = if account.is_some() {
            let Some(settings) = settings::get_settings_until(
                &self.db,
                Instant::now() + Duration::from_secs(2),
                || self.stop.load(Ordering::SeqCst),
            )?
            else {
                return Err(super::sync::SyncDbBusy.into());
            };
            settings
        } else {
            get_settings(&self.db)?
        };
        let depth = depth.unwrap_or(settings["analysis_depth"].as_i64().unwrap_or(16));
        if !(6..=30).contains(&depth) {
            return Err(crate::api_error(
                422,
                "Analysis depth must be between 6 and 30",
            ));
        }
        let enqueue = |tx: &rusqlite::Transaction<'_>| {
            if let Some((account, generation)) = account
                && !tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM accounts WHERE id=? AND sync_generation=?)",
                    rusqlite::params![account, generation],
                    |r| r.get::<_, bool>(0),
                )?
            {
                return Ok(0);
            }
            let mut seen = HashSet::new();
            let mut added = 0;
            for &id in ids {
                if account.is_some() && self.stop.load(Ordering::SeqCst) {
                    anyhow::bail!("Sync cancelled");
                }
                if !seen.insert(id) {
                    continue;
                }
                if let Some((account, _)) = account
                    && !tx.query_row("SELECT EXISTS(SELECT 1 FROM account_games WHERE account_id=? AND game_id=?)",rusqlite::params![account,id],|r|r.get::<_,bool>(0))? {
                    continue;
                }
                let game = tx
                    .query_row("SELECT end_time,rules FROM games WHERE id=?", [id], |r| {
                        Ok((r.get::<_, Option<i64>>(0)?, r.get::<_, String>(1)?))
                    })
                    .optional()?;
                let Some((end_time, rules)) = game else {
                    continue;
                };
                if !matches!(rules.as_str(), "chess" | "chess960") {
                    continue;
                }
                if !force {
                    let done:bool=tx.query_row("SELECT EXISTS(SELECT 1 FROM game_analysis WHERE game_id=? AND depth>=? AND version>=?)",rusqlite::params![id,depth,ANALYSIS_VERSION],|r|r.get(0))?;
                    if done {
                        continue;
                    }
                }
                let active=tx.query_row("SELECT id,status FROM jobs WHERE game_id=? AND status IN ('queued','running')",[id],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?))).optional()?;
                if let Some((job, status)) = active {
                    if status == "queued" {
                        tx.execute("UPDATE jobs SET priority=MAX(priority,?),depth=MAX(COALESCE(depth,0),?) WHERE id=?",rusqlite::params![priority,depth,job])?;
                    }
                    continue;
                }
                tx.execute("DELETE FROM jobs WHERE game_id=? AND status='failed'", [id])?;
                tx.execute("INSERT INTO jobs(game_id,status,priority,depth,created_at,sort_key) VALUES(?,'queued',?,?,?,?)",rusqlite::params![id,priority,depth,now(),end_time.unwrap_or(0)])?;
                added += 1;
            }
            Ok(added)
        };
        let added = if account.is_some() {
            self.db
                .transaction_until(
                    Instant::now() + Duration::from_secs(2),
                    || self.stop.load(Ordering::SeqCst),
                    enqueue,
                )?
                .ok_or(super::sync::SyncDbBusy)?
        } else {
            self.db.transaction(enqueue)?
        };
        if added > 0 {
            self.notify();
        }
        Ok(added)
    }
    pub fn stats(&self) -> Value {
        let mut counts = HashMap::new();
        if let Ok(rows) = self
            .db
            // "done" means completed in the last 7 days: older rows are only pruned after
            // the next review, so an idle week would otherwise show them as recent.
            .query(
                "SELECT status,COUNT(*) AS n FROM jobs WHERE status<>'done' OR finished_at>=? GROUP BY status",
                &[json!(crate::now() - 7 * 86_400)],
            )
        {
            for row in rows {
                counts.insert(
                    row["status"].as_str().unwrap_or("").to_string(),
                    row["n"].as_i64().unwrap_or(0),
                );
            }
        }
        let active = self
            .progress
            .lock()
            .unwrap()
            .values()
            .map(|(v, _)| v.clone())
            .collect::<Vec<_>>();
        let avg=self.db.scalar("SELECT AVG(duration_ms) FROM (SELECT duration_ms FROM game_analysis ORDER BY analyzed_at DESC LIMIT 30)",&[]).ok().and_then(|v|v.as_f64()).filter(|v|*v>0.);
        let workers = self.worker_count.load(Ordering::SeqCst);
        let queued = *counts.get("queued").unwrap_or(&0);
        json!({"paused":self.paused.load(Ordering::SeqCst),"workers":workers,"engine":*self.engine_name.lock().unwrap(),"engine_error":*self.engine_error.lock().unwrap(),"queued":queued,"running":counts.get("running").copied().unwrap_or(0),"failed":counts.get("failed").copied().unwrap_or(0),"done":counts.get("done").copied().unwrap_or(0),"active":active,"avg_game_seconds":avg.map(|v|(v/100.).round()/10.),"eta_seconds":avg.filter(|_|!self.paused.load(Ordering::SeqCst)).map(|v|(queued as f64*v/1000./workers.max(1) as f64)as i64)})
    }
    pub(super) fn supervise_workers(self: Arc<Self>) {
        let mut workers: Vec<(Arc<AtomicBool>, JoinHandle<()>)> = Vec::new();
        let mut generation = u64::MAX;
        while !self.stop.load(Ordering::SeqCst) {
            self.flush_releases(Duration::from_millis(25));
            let desired_generation = self.generation.load(Ordering::SeqCst);
            if generation != desired_generation || workers.iter().any(|(_, h)| h.is_finished()) {
                for (cancel, _) in &workers {
                    cancel.store(true, Ordering::SeqCst);
                }
                self.cancel_running_jobs();
                self.notify();
                for (_, h) in workers.drain(..) {
                    let _ = h.join();
                }
                let settings = match settings::get_settings_until(
                    &self.db,
                    Instant::now() + Duration::from_secs(2),
                    || self.stop.load(Ordering::SeqCst),
                ) {
                    Ok(Some(settings)) => settings,
                    Ok(None) => continue,
                    Err(error) => {
                        tracing::error!(%error,"Could not read worker settings");
                        self.wait(Duration::from_millis(250));
                        continue;
                    }
                };
                if let Err(error) = settings::validate_resources(&settings) {
                    *self.engine_error.lock().unwrap() = Some(error.to_string());
                    self.worker_count.store(0, Ordering::SeqCst);
                    generation = desired_generation;
                    continue;
                }
                let threads = settings["threads_per_worker"].as_u64().unwrap_or(1) as usize;
                let hash = settings["hash_mb"].as_u64().unwrap_or(64) as usize;
                *self.engine_error.lock().unwrap() = None;
                let count = settings["workers"].as_u64().unwrap_or(1).clamp(1, 32);
                self.worker_count.store(count, Ordering::SeqCst);
                for id in 0..count {
                    let service = Arc::clone(&self);
                    let cancel = Arc::new(AtomicBool::new(false));
                    let worker_cancel = Arc::clone(&cancel);
                    workers.push((
                        cancel,
                        std::thread::spawn(move || {
                            service.worker_loop(id, worker_cancel, threads, hash)
                        }),
                    ));
                }
                generation = desired_generation;
            }
            self.wait(Duration::from_millis(250));
        }
        for (cancel, _) in &workers {
            cancel.store(true, Ordering::SeqCst);
        }
        self.cancel_running_jobs();
        self.notify();
        for (_, h) in workers {
            let _ = h.join();
        }
        self.flush_releases(Duration::from_millis(250));
        self.worker_count.store(0, Ordering::SeqCst);
    }
    fn claim(&self, cancel: &AtomicBool) -> Result<Option<Value>> {
        Ok(self.db.transaction_until(Instant::now()+Duration::from_secs(2),||cancel.load(Ordering::SeqCst)||self.stop.load(Ordering::SeqCst),|tx|{
            let mut job=query_connection(tx,"SELECT j.*,g.uuid AS game_uuid,g.pgn AS game_pgn FROM jobs j JOIN games g ON g.id=j.game_id WHERE j.status='queued' ORDER BY j.priority DESC,j.sort_key DESC,j.id LIMIT 1",&[])?.into_iter().next();
            if let Some(job)=&mut job {
                let token=format!("{:032x}",rand::random::<u128>());
                tx.execute("UPDATE jobs SET status='running',started_at=?,attempts=attempts+1,claim_token=? WHERE id=?",rusqlite::params![now(),token,job["id"].as_i64()])?;
                job["claim_token"]=json!(token);
            }
            Ok(job)
        })?.flatten())
    }
    fn release_job(&self, job: &Value, error: Option<String>) {
        self.pending_releases.lock().unwrap().push(PendingRelease {
            id: job["id"].as_i64().unwrap(),
            token: job["claim_token"].as_str().unwrap().to_string(),
            error,
        });
        self.notify();
    }
    fn flush_releases(&self, budget: Duration) {
        let releases = std::mem::take(&mut *self.pending_releases.lock().unwrap());
        if releases.is_empty() {
            return;
        }
        // Cleanup deliberately remains allowed after cancellation, but only for
        // this short budget. If a writer still holds SQLite, retain the tokens
        // for the supervisor's next pass (or startup recovery after shutdown).
        let result=self.db.transaction_until(Instant::now()+budget,||false,|tx|{
            for release in &releases {
                if let Some(message)=&release.error {
                    let changed=tx.execute("UPDATE jobs SET status=CASE WHEN attempts>=3 THEN 'failed' ELSE 'queued' END,priority=priority-1,error=?,finished_at=CASE WHEN attempts>=3 THEN ? ELSE NULL END WHERE id=? AND claim_token=? AND status='running'",rusqlite::params![message,now(),release.id,release.token])?;
                    if changed>0 {tx.execute("INSERT INTO events(ts,level,source,message) VALUES(?,'error','analysis',?)",rusqlite::params![now(),message])?;}
                }else{
                    tx.execute("UPDATE jobs SET status='queued',attempts=MAX(0,attempts-1),error=NULL WHERE id=? AND claim_token=? AND status='running'",rusqlite::params![release.id,release.token])?;
                }
            }
            tx.execute("DELETE FROM events WHERE id < (SELECT MAX(id)-5000 FROM events)",[])?;
            Ok(())
        });
        if !matches!(result, Ok(Some(()))) {
            if let Err(error) = result {
                tracing::warn!(%error,"Could not persist worker cleanup; retaining claims for retry");
            }
            self.pending_releases.lock().unwrap().extend(releases);
        }
    }
    /// Stops every analysis in progress; the workers set their own flags.
    fn cancel_running_jobs(&self) {
        for (_, job_cancel) in self.progress.lock().unwrap().values() {
            job_cancel.store(true, Ordering::SeqCst);
        }
    }
    fn worker_loop(&self, worker: u64, cancel: Arc<AtomicBool>, threads: usize, hash: usize) {
        let mut engine: Option<Engine> = None;
        while !cancel.load(Ordering::SeqCst) && !self.stop.load(Ordering::SeqCst) {
            let observed = self.wake_generation();
            if self.paused.load(Ordering::SeqCst) {
                self.wait(Duration::from_secs(1));
                continue;
            }
            if engine.is_none() {
                // Avoid launching engines on an empty queue.
                if !self
                    .db
                    .read_until(
                        Instant::now() + Duration::from_secs(2),
                        || cancel.load(Ordering::SeqCst) || self.stop.load(Ordering::SeqCst),
                        |c| {
                            Ok(c.query_row(
                                "SELECT EXISTS(SELECT 1 FROM jobs WHERE status='queued')",
                                [],
                                |r| r.get::<_, bool>(0),
                            )?)
                        },
                    )
                    .ok()
                    .flatten()
                    .unwrap_or(false)
                {
                    self.wait_since(observed, Duration::from_millis(500));
                    continue;
                }
                match Engine::new(threads, hash) {
                    Ok(mut e) => {
                        *self.engine_name.lock().unwrap() = Some(e.name.clone());
                        *self.engine_error.lock().unwrap() = None;
                        e.attach_database(Arc::clone(&self.db));
                        engine = Some(e);
                    }
                    Err(error) => {
                        *self.engine_error.lock().unwrap() =
                            Some(format!("Could not start Stockfish: {error}"));
                        self.wait(Duration::from_secs(5));
                        continue;
                    }
                }
            }
            let job = match self.claim(&cancel) {
                Ok(Some(job)) => job,
                Ok(None) => {
                    self.wait_since(observed, Duration::from_millis(500));
                    continue;
                }
                Err(error) => {
                    tracing::error!(%error,"Could not claim analysis job");
                    self.wait(Duration::from_secs(1));
                    continue;
                }
            };
            let game_id = job["game_id"].as_i64().unwrap();
            // Each job has its own flag, so cancelling one job (its account was deleted) does not
            // also end this worker, which would make the supervisor restart every worker.
            let job_cancel = Arc::new(AtomicBool::new(false));
            let result = self.run_job(&job, engine.as_mut().unwrap(), worker, &cancel, &job_cancel);
            self.progress
                .lock()
                .unwrap()
                .remove(job["claim_token"].as_str().unwrap());
            match result {
                Ok(true) => {}
                Ok(false) => self.release_job(&job, None),
                Err(_)
                    if cancel.load(Ordering::SeqCst)
                        || job_cancel.load(Ordering::SeqCst)
                        || self.stop.load(Ordering::SeqCst) =>
                {
                    self.release_job(&job, None)
                }
                Err(error) => {
                    let message = format!("Game {game_id}: analysis failed: {error}");
                    tracing::warn!(%message);
                    self.release_job(&job, Some(message));
                    engine = None;
                }
            }
        }
    }
    fn run_job(
        &self,
        job: &Value,
        engine: &mut Engine,
        worker: u64,
        cancel: &Arc<AtomicBool>,
        job_cancel: &Arc<AtomicBool>,
    ) -> Result<bool> {
        let id = job["id"].as_i64().unwrap();
        let game_id = job["game_id"].as_i64().unwrap();
        let token = job["claim_token"].as_str().unwrap();
        let Some(game) = self
            .db
            .read_until(
                Instant::now() + Duration::from_secs(2),
                || cancel.load(Ordering::SeqCst) || self.stop.load(Ordering::SeqCst),
                |c| {
                    Ok(query_connection(
                        c,
                        "SELECT * FROM games WHERE id=? AND uuid=? AND pgn=?",
                        &[
                            json!(game_id),
                            job["game_uuid"].clone(),
                            job["game_pgn"].clone(),
                        ],
                    )?
                    .into_iter()
                    .next())
                },
            )?
            .flatten()
        else {
            return Ok(false);
        };
        let Some(settings) = settings::get_settings_until(
            &self.db,
            Instant::now() + Duration::from_secs(2),
            || cancel.load(Ordering::SeqCst) || self.stop.load(Ordering::SeqCst),
        )?
        else {
            return Ok(false);
        };
        let depth = job["depth"]
            .as_u64()
            .unwrap_or(settings["analysis_depth"].as_u64().unwrap_or(16))
            as u32;
        let multipv = settings["analysis_multipv"].as_u64().unwrap_or(2) as u32;
        let opts = AnalysisOptions {
            depth,
            multipv,
            time_cap: settings["max_seconds_per_position"].as_f64().unwrap_or(5.),
            ..Default::default()
        };
        let started = Instant::now();
        self.progress.lock().unwrap().insert(
            token.to_string(),
            (
                json!({"job_id":id,"game_id":game_id,"done":0,"total":game["plies"],"worker":worker}),
                Arc::clone(job_cancel),
            ),
        );
        let progress = |done: usize, total: usize| {
            self.progress.lock().unwrap().insert(
                token.to_string(),
                (
                    json!({"job_id":id,"game_id":game_id,"done":done,"total":total,"worker":worker}),
                    Arc::clone(job_cancel),
                ),
            );
            if done.is_multiple_of(10) || done == total {
                let _ = self.db.try_transaction(|tx|Ok(tx.execute(
                    "UPDATE jobs SET progress=?,total=? WHERE id=? AND claim_token=? AND status='running'",
                    rusqlite::params![done as i64, total as i64, id,token],
                )?));
            }
        };
        // The job is in the progress map now, so a worker cancelled before this point is seen
        // here and one cancelled later reaches the job through cancel_running_jobs.
        if cancel.load(Ordering::SeqCst) {
            job_cancel.store(true, Ordering::SeqCst);
        }
        let mut result = analysis::analyze(
            engine,
            game["pgn"].as_str().unwrap_or(""),
            &opts,
            job_cancel,
            progress,
        )?;
        if job_cancel.load(Ordering::SeqCst) {
            anyhow::bail!("Analysis cancelled");
        }
        result["duration_ms"] = json!(started.elapsed().as_millis() as u64);
        let analysis_us = started.elapsed().as_micros() as u64;
        let storing = Instant::now();
        let saved = store::save_analysis(
            &self.db,
            &store::AnalysisClaim {
                game_id,
                job_id: id,
                token,
                uuid: job["game_uuid"].as_str().unwrap(),
                pgn: job["game_pgn"].as_str().unwrap(),
                cancel: job_cancel,
            },
            &result,
            depth as i64,
            multipv as i64,
            &engine.name,
        )?;
        tracing::debug!(target: "plyback::review_timing", event = "stored", game_id, worker, analysis_us, storage_us = storing.elapsed().as_micros() as u64, saved);
        if !saved {
            return Ok(false);
        }
        let _ = self.db.try_transaction(|tx| {
            Ok(tx.execute(
                "DELETE FROM jobs WHERE status='done' AND finished_at<?",
                [now() - 7 * 86400],
            )?)
        });
        Ok(true)
    }
    pub(super) fn backfill(&self, account: i64) -> Result<usize> {
        let rows=self.db.query("SELECT ag.game_id FROM account_games ag JOIN games g ON g.id=ag.game_id LEFT JOIN game_analysis ga ON ga.game_id=g.id LEFT JOIN jobs j ON j.game_id=g.id WHERE ag.account_id=? AND ga.game_id IS NULL AND j.id IS NULL AND g.rules IN ('chess','chess960')",&[json!(account)])?;
        self.enqueue(
            &rows
                .iter()
                .filter_map(|r| r["game_id"].as_i64())
                .collect::<Vec<_>>(),
            PRIORITY_HISTORY,
            None,
            false,
        )
    }
    pub(super) fn retry_failed(&self) -> Result<usize> {
        let count=self.db.transaction(|tx|{tx.execute("DELETE FROM jobs WHERE status='failed' AND id NOT IN(SELECT MAX(id) FROM jobs WHERE status='failed' GROUP BY game_id)",[])?;Ok(tx.execute("UPDATE jobs SET status='queued',attempts=0,error=NULL WHERE status='failed' AND game_id NOT IN(SELECT game_id FROM jobs WHERE status IN('queued','running'))",[])?)})?;
        self.notify();
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stop_during_external_write_lock_is_bounded_and_claims_remain_recoverable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("contention.db");
        let db = Arc::new(Database::open(&path).unwrap());
        db.execute("INSERT INTO games(uuid,pgn,imported_at) VALUES('one','1. e4 e5 *',0),('two','1. d4 d5 *',0)",&[]).unwrap();
        let service = Services::new(db.clone()).unwrap();
        service.enqueue(&[1, 2], 100, Some(6), false).unwrap();
        let old = service.claim(&AtomicBool::new(false)).unwrap().unwrap();
        service.release_job(&old, None);
        let blocker = rusqlite::Connection::open(&path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<()>();
        let worker = service.clone();
        service
            .handles
            .lock()
            .unwrap()
            .push(std::thread::spawn(move || {
                ready_tx.send(()).unwrap();
                assert!(worker.claim(&AtomicBool::new(false)).unwrap().is_none());
                worker.flush_releases(Duration::from_millis(250));
            }));
        ready_rx.recv().unwrap();
        let start = Instant::now();
        service.stop();
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "Database contention delayed shutdown"
        );
        assert_eq!(service.pending_releases.lock().unwrap().len(), 1);
        assert_eq!(
            db.scalar("SELECT status FROM jobs WHERE id=?", &[old["id"].clone()])
                .unwrap(),
            "running"
        );
        blocker.execute_batch("ROLLBACK").unwrap();
        // The same process can retry cleanup after a settings restart; a new
        // process also recovers any durable running row during Services::start.
        service.flush_releases(Duration::from_secs(1));
        assert!(service.pending_releases.lock().unwrap().is_empty());
        assert_eq!(
            db.scalar("SELECT status FROM jobs WHERE id=?", &[old["id"].clone()])
                .unwrap(),
            "queued"
        );
        assert_eq!(
            db.scalar("SELECT attempts FROM jobs WHERE id=?", &[old["id"].clone()])
                .unwrap(),
            0
        );
        service.stop.store(false, Ordering::SeqCst);
        let new = service.claim(&AtomicBool::new(false)).unwrap().unwrap();
        assert_eq!(new["id"], old["id"]);
        assert_ne!(new["claim_token"], old["claim_token"]);
        service.release_job(&old, None);
        service.flush_releases(Duration::from_secs(1));
        assert_eq!(
            db.scalar("SELECT status FROM jobs WHERE id=?", &[new["id"].clone()])
                .unwrap(),
            "running"
        );
    }
    #[test]
    fn account_enqueue_checks_generation_and_membership_inside_the_claim_transaction() {
        let db = Arc::new(Database::open(":memory:").unwrap());
        let service = Services::new(db.clone()).unwrap();
        db.execute(
            "INSERT INTO accounts(id,username,added_at) VALUES(1,'old',0)",
            &[],
        )
        .unwrap();
        let old = db
            .scalar("SELECT sync_generation FROM accounts WHERE id=1", &[])
            .unwrap();
        db.execute("DELETE FROM accounts WHERE id=1", &[]).unwrap();
        db.execute(
            "INSERT INTO accounts(id,username,added_at) VALUES(1,'new',0)",
            &[],
        )
        .unwrap();
        let new = db
            .scalar("SELECT sync_generation FROM accounts WHERE id=1", &[])
            .unwrap();
        db.execute("INSERT INTO games(id,uuid,pgn,imported_at) VALUES(1,'one','1. e4 e5 *',0),(2,'two','1. d4 d5 *',0)",&[]).unwrap();
        db.execute("INSERT INTO account_games VALUES(1,2,'white')", &[])
            .unwrap();
        assert_eq!(
            service
                .enqueue_for_account(1, old.as_str().unwrap(), &[1, 2], 50)
                .unwrap(),
            0
        );
        assert_eq!(
            service
                .enqueue_for_account(1, new.as_str().unwrap(), &[1, 2], 50)
                .unwrap(),
            1
        );
        assert_eq!(db.scalar("SELECT game_id FROM jobs", &[]).unwrap(), 2);
    }
    fn wait_until(mut check: impl FnMut() -> bool, seconds: u64) -> bool {
        let until = Instant::now() + Duration::from_secs(seconds);
        while Instant::now() < until {
            if check() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        check()
    }
    #[test]
    fn real_engine_queue_cancels_requeues_and_resumes_without_live_data() {
        if crate::engine::locate_stockfish().is_err() {
            eprintln!("Stockfish unavailable; real engine queue test skipped");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let db = Arc::new(Database::open(dir.path().join("queue.db")).unwrap());
        save_settings(
            &db,
            &json!({"workers":1,"analysis_depth":30,"max_seconds_per_position":60.0,"hash_mb":16}),
        )
        .unwrap();
        let service = Services::new(Arc::clone(&db)).unwrap();
        let (_, imported) = service
            .handle(
                "POST",
                "/api/games/import",
                &Query::new(),
                &json!({"pgn":"1. e4 e5 2. Nf3 Nc6 3. Bb5 a6 *","analyze":true}),
            )
            .unwrap()
            .unwrap();
        let game = imported["game_ids"][0].as_i64().unwrap();
        service.start();
        let active = wait_until(|| service.stats()["running"] == 1, 10);
        let start = Instant::now();
        service.stop();
        assert!(active, "Engine did not start: {}", service.stats());
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "Cancellation exceeded cleanup budget"
        );
        assert_eq!(service.stats()["workers"], 0);
        assert_eq!(service.stats()["queued"], 1);
        assert_eq!(db.scalar("SELECT attempts FROM jobs", &[]).unwrap(), 0);
        save_settings(
            &db,
            &json!({"analysis_depth":6,"max_seconds_per_position":0.2}),
        )
        .unwrap();
        db.execute("UPDATE jobs SET depth=6", &[]).unwrap();
        service.start();
        let completed = wait_until(
            || {
                db.scalar("SELECT COUNT(*) FROM game_analysis", &[])
                    .unwrap()
                    == 1
            },
            20,
        );
        service.stop();
        assert!(completed, "Job did not resume: {}", service.stats());
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM moves WHERE game_id=?", &[json!(game)])
                .unwrap(),
            6
        );
        assert_eq!(service.stats()["done"], 1);
        assert_eq!(service.enqueue(&[game], 100, Some(6), false).unwrap(), 0);
        assert!(
            db.scalar("SELECT COUNT(*) FROM position_cache", &[])
                .unwrap()
                .as_i64()
                .unwrap()
                > 0
        );
    }
}
