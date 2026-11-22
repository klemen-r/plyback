//! Preserved insight contract snapshots over public game fixtures.
//! Contains no live account data. Numerical JSON representation is compared by value.
use plyback::{
    Query,
    db::Database,
    insights::{Insights, compute},
    services::Services,
};
use serde_json::{Value, json};
use std::sync::Arc;

fn fixture() -> Arc<Database> {
    let db = Arc::new(Database::open(":memory:").unwrap());
    db.transaction(|tx| {
        tx.execute_batch(include_str!("fixtures/insights.sql"))?;
        Ok(())
    })
    .unwrap();
    db
}
fn equal(actual: &Value, expected: &Value, path: &str) {
    match (actual, expected) {
        (Value::Number(a), Value::Number(b)) => assert!(
            (a.as_f64().unwrap() - b.as_f64().unwrap()).abs() < 1e-8,
            "{path}: {a} != {b}"
        ),
        (Value::Array(a), Value::Array(b)) => {
            assert_eq!(a.len(), b.len(), "{path}: array length");
            for (i, (a, b)) in a.iter().zip(b).enumerate() {
                equal(a, b, &format!("{path}/{i}"));
            }
        }
        (Value::Object(a), Value::Object(b)) => {
            assert_eq!(
                a.keys().collect::<Vec<_>>(),
                b.keys().collect::<Vec<_>>(),
                "{path}: object keys"
            );
            for (k, v) in a {
                equal(v, &b[k], &format!("{path}/{k}"));
            }
        }
        _ => assert_eq!(actual, expected, "{path}"),
    }
}
#[test]
fn full_insights_match_preserved_contract_snapshot() {
    let db = fixture();
    let expected: Value =
        serde_json::from_str(include_str!("fixtures/insights-expected.json")).unwrap();
    for case in expected.as_array().unwrap() {
        let mut result = compute(&db, 1, &case["filters"]).unwrap();
        // Additional fields outside the preserved contract snapshot.
        result["results"]
            .as_object_mut()
            .unwrap()
            .remove("by_quarter_hour");
        for item in result["recurring"].as_array_mut().unwrap() {
            // The total counts every game; `games` is only the latest few as examples.
            let examples = item["games"].as_array().unwrap().len();
            let total = item["game_count"].as_u64().unwrap() as usize;
            assert!(total >= 2 && total >= examples && examples <= 8, "{item}");
            item.as_object_mut().unwrap().remove("game_count");
        }
        equal(
            &result,
            &case["result"],
            &format!("filters={}", case["filters"]),
        );
    }
}
#[test]
fn independent_databases_and_reanalysis_do_not_share_move_rows() {
    let a = fixture();
    let b = fixture();
    let first = compute(&a, 1, &json!({})).unwrap();
    b.transaction(|tx| {
        tx.execute(
            "UPDATE moves SET classification='best',ep_loss=0,tags='{}'",
            [],
        )?;
        tx.execute("UPDATE game_analysis SET revision=revision+1", [])?;
        Ok(())
    })
    .unwrap();
    let second = compute(&b, 1, &json!({})).unwrap();
    assert_ne!(
        first["overview"]["per_game"],
        second["overview"]["per_game"]
    );
    assert_eq!(second["overview"]["per_game"]["blunder"], json!(0.0));
    assert_eq!(
        compute(&a, 1, &json!({})).unwrap()["overview"],
        first["overview"]
    );
}
fn runtime(db: Arc<Database>) -> Arc<Insights> {
    Insights::new(db.clone(), Services::new(db).unwrap())
}
fn call(service: &Insights, method: &str, path: &str, body: Value) -> Value {
    service
        .handle(method, path, &Query::new(), &body)
        .unwrap()
        .unwrap()
        .1
}

fn repeated_errors() -> Arc<Database> {
    let db = Arc::new(Database::open(":memory:").unwrap());
    db.execute(
        "INSERT INTO accounts(id,username,added_at,sync_enabled) VALUES(1,'me',0,0)",
        &[],
    )
    .unwrap();
    for id in 1..=2 {
        let pgn = format!(
            "[White \"me\"]\n[Black \"opponent{id}\"]\n[Result \"1-0\"]\n\n1. e4 e5 2. Nf3 Nc6 3. d4 Qh4 4. Nxh4 1-0"
        );
        db.execute("INSERT INTO games(id,uuid,pgn,rules,time_control,time_class,white_username,black_username,white_result,black_result,end_time,imported_at) VALUES(?,?,?,'chess','180','blitz','me',?,'win','resigned',?,0)", &[json!(id),json!(format!("repeat-{id}")),json!(pgn),json!(format!("opponent{id}")),json!(1790000000 + id * 3600)]).unwrap();
        db.execute(
            "INSERT INTO account_games VALUES(1,?,'white')",
            &[json!(id)],
        )
        .unwrap();
        save_repeated_analysis(&db, id, false);
    }
    db
}

fn save_repeated_analysis(db: &Database, id: i64, clean: bool) {
    use plyback::services::store::{AnalysisClaim, save_analysis};
    use std::sync::atomic::AtomicBool;
    let game = db
        .one("SELECT uuid,pgn FROM games WHERE id=?", &[json!(id)])
        .unwrap()
        .unwrap();
    let parsed = plyback::pgn::parse(game["pgn"].as_str().unwrap()).unwrap();
    let moves = parsed.moves.iter().map(|m| {
        let (class, loss) = match (clean, m.san.as_str()) {
            (false, "Nf3") => ("mistake", 0.1),
            (false, "Qh4") => ("blunder", 0.8),
            _ => ("best", 0.0),
        };
        json!({"ply":m.ply,"color":m.color,"san":m.san,"uci":m.uci,"fen_before":m.fen_before,"eval_before":0,"eval_after":0,"win_before":50.0,"win_after":50.0,"ep_loss":loss,"cp_loss":0,"accuracy":100.0,"best_uci":m.uci,"best_san":m.san,"classification":class,"phase":"opening","clock":100.0,"time_spent":2.0,"is_book":0,"tags":{},"explanation":"Fixture","lines":[],"reply":null,"endgame_type":"mixed"})
    }).collect::<Vec<_>>();
    let result = json!({"moves":moves,"duration_ms":0,"accuracy":{"white":100.0,"black":100.0},"acpl":{"white":0.0,"black":0.0},"opening":null,"book_plies":0,"counts":{},"key_moments":[],"evals":[]});
    db.execute("INSERT INTO jobs(game_id,status,created_at,claim_token) VALUES(?,'running',0,'fixture-claim')", &[json!(id)]).unwrap();
    let job = db
        .scalar("SELECT MAX(id) FROM jobs", &[])
        .unwrap()
        .as_i64()
        .unwrap();
    let cancel = AtomicBool::new(false);
    let claim = AnalysisClaim {
        game_id: id,
        job_id: job,
        token: "fixture-claim",
        uuid: game["uuid"].as_str().unwrap(),
        pgn: game["pgn"].as_str().unwrap(),
        cancel: &cancel,
    };
    assert!(save_analysis(db, &claim, &result, 8, 2, "Fixture engine").unwrap());
}

#[test]
fn recurring_lists_only_the_users_repeated_errors() {
    let db = repeated_errors();
    let insights = compute(&db, 1, &json!({})).unwrap();
    assert_eq!(insights["overview"]["opp_per_game"]["blunder"], 1.0);
    let recurring = insights["recurring"].as_array().unwrap();
    assert_eq!(recurring.len(), 1);
    assert_eq!(recurring[0]["occurrences"], 2);
    assert_eq!(recurring[0]["played"], json!([{"san":"Nf3","count":2}]));
    for row in recurring {
        assert!(
            row["played"]
                .as_array()
                .unwrap()
                .iter()
                .all(|played| played["san"] != "Qh4")
        );
    }
}

#[test]
fn saved_reanalysis_refreshes_insights_without_a_manual_revision_bump() {
    let db = repeated_errors();
    // Freeze persisted timestamps so the revision, rather than elapsed wall time,
    // must distinguish both saved analyses even when the test crosses a second.
    db.execute("UPDATE game_analysis SET analyzed_at=1234567890", &[])
        .unwrap();
    db.execute("CREATE TEMP TRIGGER fixed_analysis_update AFTER UPDATE OF analyzed_at ON game_analysis WHEN NEW.analyzed_at!=1234567890 BEGIN UPDATE game_analysis SET analyzed_at=1234567890 WHERE game_id=NEW.game_id; END", &[]).unwrap();
    let original_rows = db
        .query(
            "SELECT game_id,analyzed_at,revision FROM game_analysis ORDER BY game_id",
            &[],
        )
        .unwrap();
    let service = runtime(db.clone());
    let first = call(&service, "GET", "/api/accounts/1/insights", Value::Null);
    assert_eq!(first["overview"]["per_game"]["mistake"], 1.0);
    assert_eq!(first["recurring"].as_array().unwrap().len(), 1);
    for id in 1..=2 {
        save_repeated_analysis(&db, id, true);
    }
    let updated_rows = db
        .query(
            "SELECT game_id,analyzed_at,revision FROM game_analysis ORDER BY game_id",
            &[],
        )
        .unwrap();
    assert_eq!(original_rows.len(), 2);
    assert_eq!(updated_rows.len(), 2);
    for (original, updated) in original_rows.iter().zip(&updated_rows) {
        assert_eq!(original["game_id"], updated["game_id"]);
        assert_eq!(original["analyzed_at"], updated["analyzed_at"]);
        assert_eq!(
            updated["revision"].as_i64().unwrap(),
            original["revision"].as_i64().unwrap() + 1
        );
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let updated = call(&service, "GET", "/api/accounts/1/insights", Value::Null);
        if updated["stale"] != true {
            assert_eq!(updated["overview"]["per_game"]["mistake"], 0.0);
            assert_eq!(updated["overview"]["opp_per_game"]["blunder"], 0.0);
            assert!(updated["recurring"].as_array().unwrap().is_empty());
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "saved analysis never reached the response cache"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    service.stop();
}
#[test]
fn completed_reports_are_immutable_and_membership_is_fixed() {
    let db = fixture();
    let service = runtime(db.clone());
    let rep = call(
        &service,
        "POST",
        "/api/accounts/1/reports",
        json!({"title":"Fixture report"}),
    );
    let id = rep["id"].as_i64().unwrap();
    let original_games = compute(&db, 1, &json!({})).unwrap()["overview"]["games"].clone();
    db.transaction(|tx|{tx.execute("INSERT INTO games(uuid,pgn,rules,source,white_username,black_username,white_result,black_result,imported_at,end_time) VALUES('after-report','1. e4 *','chess','pgn','fixture_player','new-opponent','win','resigned',0,1799000000)",[])?;let gid=tx.last_insert_rowid();tx.execute("INSERT INTO account_games(account_id,game_id,color) VALUES(1,?,'white')",[gid])?;Ok(())}).unwrap();
    call(
        &service,
        "POST",
        &format!("/api/reports/{id}/finalize"),
        json!({}),
    );
    let first = call(&service, "GET", &format!("/api/reports/{id}"), Value::Null);
    assert_eq!(first["status"], "ready");
    assert_eq!(first["report"]["overview"]["games"], original_games);
    db.execute(
        "UPDATE games SET white_result='resigned',black_result='win'",
        &[],
    )
    .unwrap();
    call(
        &service,
        "POST",
        &format!("/api/reports/{id}/finalize"),
        json!({}),
    );
    let second = call(&service, "GET", &format!("/api/reports/{id}"), Value::Null);
    assert_eq!(first, second);
    assert!(
        first["report"]["data_quality"]
            .get("unanalyzed_ids")
            .is_none()
    );
    service.stop();
}
#[test]
fn report_coverage_counts_reviews_already_included_in_the_snapshot() {
    let db = fixture();
    let service = runtime(db.clone());
    let report = call(&service, "POST", "/api/accounts/1/reports", json!({}));
    let id = report["id"].as_i64().unwrap();
    assert!(
        db.scalar("SELECT COUNT(*) FROM jobs WHERE status='queued'", &[])
            .unwrap()
            .as_i64()
            .unwrap()
            > 0
    );
    call(
        &service,
        "POST",
        &format!("/api/reports/{id}/finalize"),
        json!({}),
    );
    let ready = call(&service, "GET", &format!("/api/reports/{id}"), Value::Null);
    assert_eq!(ready["status"], "ready");
    let included = &ready["report"]["data_quality"]["analyzed"];
    assert!(included.as_u64().unwrap() > 0);
    assert_eq!(&ready["analyzed_games"], included);
    service.stop();
}

#[test]
fn empty_report_scope_stays_empty_and_deleted_ids_cannot_join_it() {
    let db = fixture();
    let service = runtime(db.clone());
    let empty = call(
        &service,
        "POST",
        "/api/accounts/1/reports",
        json!({"filters":{"until":0}}),
    );
    let empty_id = empty["id"].as_i64().unwrap();
    let rep = call(&service, "POST", "/api/accounts/1/reports", json!({}));
    let rid = rep["id"].as_i64().unwrap();
    let original = rep["total_games"].as_u64().unwrap();
    db.transaction(|tx|{tx.execute("DELETE FROM games WHERE id=1",[])?;tx.execute("INSERT INTO games(id,uuid,pgn,rules,source,white_username,black_username,imported_at,end_time) VALUES(1,'different-uuid','1. e4 *','chess','pgn','fixture_player','new',0,0)",[])?;tx.execute("INSERT INTO account_games VALUES(1,1,'white')",[])?;Ok(())}).unwrap();
    call(
        &service,
        "POST",
        &format!("/api/reports/{empty_id}/finalize"),
        json!({}),
    );
    assert_eq!(
        call(
            &service,
            "GET",
            &format!("/api/reports/{empty_id}"),
            Value::Null
        )["report"]["overview"]["games"],
        json!(0)
    );
    call(
        &service,
        "POST",
        &format!("/api/reports/{rid}/finalize"),
        json!({}),
    );
    let frozen = call(&service, "GET", &format!("/api/reports/{rid}"), Value::Null);
    assert_eq!(frozen["total_games"], json!(original - 1));
    service.stop();
}

#[test]
fn pending_report_keeps_membership_after_filter_fields_are_corrected() {
    let db = fixture();
    let service = runtime(db.clone());
    let filter = json!({"time_classes":"blitz","color":"white"});
    let original = compute(&db, 1, &filter).unwrap()["overview"]["games"].clone();
    assert!(original.as_u64().unwrap() > 0);
    let report = call(
        &service,
        "POST",
        "/api/accounts/1/reports",
        json!({"filters":filter}),
    );
    let id = report["id"].as_i64().unwrap();
    db.execute("UPDATE games SET time_class='rapid'", &[])
        .unwrap();
    assert_eq!(
        compute(&db, 1, &filter).unwrap()["overview"]["games"],
        json!(0)
    );
    call(
        &service,
        "POST",
        &format!("/api/reports/{id}/finalize"),
        json!({}),
    );
    let frozen = call(&service, "GET", &format!("/api/reports/{id}"), Value::Null);
    assert_eq!(frozen["report"]["overview"]["games"], original);
    service.stop();
}
#[test]
fn old_ready_report_is_not_rebuilt_and_old_pending_report_freezes_once() {
    let db = fixture();
    db.execute("INSERT INTO reports(account_id,title,filters_json,status,total_games,created_at,report_json) VALUES(1,'Legacy','{}','ready',42,0,'{\"legacy\":true}')",&[]).unwrap();
    let service = runtime(db.clone());
    call(&service, "POST", "/api/reports/1/finalize", json!({}));
    assert_eq!(
        call(&service, "GET", "/api/reports/1", Value::Null)["report"],
        json!({"legacy":true})
    );
    db.execute("INSERT INTO reports(account_id,title,filters_json,status,total_games,created_at) VALUES(1,'Pending','{}','analyzing',0,0)",&[]).unwrap();
    service.tick().unwrap();
    assert_eq!(
        db.scalar("SELECT scope_frozen FROM reports WHERE id=2", &[])
            .unwrap(),
        json!(1)
    );
    assert_eq!(
        db.scalar("SELECT status FROM reports WHERE id=2", &[])
            .unwrap(),
        json!("ready")
    );
    service.stop();
}

#[test]
fn a_report_that_cannot_be_frozen_does_not_hold_back_later_reports() {
    let db = fixture();
    // Stored filters that no longer validate make freezing this report fail on every tick.
    db.execute("INSERT INTO reports(account_id,title,filters_json,status,total_games,created_at) VALUES(1,'Broken','{\"time_classes\":[\"nope\"]}','analyzing',0,0)",&[]).unwrap();
    db.execute("INSERT INTO reports(account_id,title,filters_json,status,total_games,created_at) VALUES(1,'Fine','{}','analyzing',0,0)",&[]).unwrap();
    let service = runtime(db.clone());
    service.tick().unwrap();
    let status = |id: i64| {
        db.scalar("SELECT status FROM reports WHERE id=?", &[json!(id)])
            .unwrap()
    };
    assert_eq!(status(1), json!("analyzing"));
    assert_eq!(status(2), json!("ready"));
    service.stop();
}

#[test]
fn response_cache_is_owned_by_database_and_refreshes_same_second_changes() {
    let a = fixture();
    let b = fixture();
    let first = runtime(a.clone());
    let second = runtime(b.clone());
    let original = call(&first, "GET", "/api/accounts/1/insights", Value::Null);
    b.transaction(|tx| {
        tx.execute(
            "UPDATE moves SET classification='best',ep_loss=0,tags='{}'",
            [],
        )?;
        tx.execute("UPDATE game_analysis SET revision=revision+1", [])?;
        Ok(())
    })
    .unwrap();
    let other = call(&second, "GET", "/api/accounts/1/insights", Value::Null);
    assert_ne!(
        original["overview"]["per_game"],
        other["overview"]["per_game"]
    );
    a.transaction(|tx| {
        tx.execute(
            "UPDATE moves SET classification='best',ep_loss=0,tags='{}'",
            [],
        )?;
        tx.execute("UPDATE game_analysis SET revision=revision+1", [])?;
        Ok(())
    })
    .unwrap();
    let stale = call(&first, "GET", "/api/accounts/1/insights", Value::Null);
    assert_eq!(stale["stale"], json!(true));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let refreshed = call(&first, "GET", "/api/accounts/1/insights", Value::Null);
        if refreshed["stale"] != true {
            assert_eq!(
                refreshed["overview"]["per_game"],
                other["overview"]["per_game"]
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "cache refresh did not finish"
        );
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    first.stop();
    second.stop();
}

#[test]
#[ignore = "Run explicitly with PLYBACK_INSIGHT_BENCH_SOURCE pointing at a prepared synthetic archive"]
fn benchmark_insights_on_readonly_snapshot() {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_test_writer()
        .try_init();
    let source = std::env::var_os("PLYBACK_INSIGHT_BENCH_SOURCE")
        .expect("a synthetic benchmark source is required");
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("benchmark.db");
    Database::backup(std::path::Path::new(&source), &path).unwrap();
    let db = Database::open(&path).unwrap();
    let account = db
        .scalar("SELECT MIN(id) FROM accounts", &[])
        .unwrap()
        .as_i64()
        .unwrap();
    let start = std::time::Instant::now();
    let result = compute(&db, account, &json!({})).unwrap();
    let cold = start.elapsed().as_secs_f64() * 1000.0;
    let mut all = Vec::new();
    let mut filtered = Vec::new();
    for _ in 0..5 {
        let start = std::time::Instant::now();
        let run = compute(&db, account, &json!({})).unwrap();
        assert_eq!(run, result);
        all.push(start.elapsed().as_secs_f64() * 1000.0);
        let start = std::time::Instant::now();
        compute(
            &db,
            account,
            &json!({"time_classes":"blitz","color":"white"}),
        )
        .unwrap();
        filtered.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    let median = |samples: &[f64]| {
        let mut sorted = samples.to_vec();
        sorted.sort_by(f64::total_cmp);
        sorted[sorted.len() / 2]
    };
    let all_median = median(&all);
    let filtered_median = median(&filtered);
    let metrics = json!({
        "games": db.scalar("SELECT COUNT(*) FROM games", &[]).unwrap(),
        "moves": db.scalar("SELECT COUNT(*) FROM moves", &[]).unwrap(),
        "insights_cold_ms": cold,
        "insights_all_ms": all,
        "insights_all_median_ms": all_median,
        "insights_blitz_white_ms": filtered,
        "insights_blitz_white_median_ms": filtered_median,
        "output_equal": true,
    });
    println!("{metrics}");
    if let Some(output) = std::env::var_os("PLYBACK_INSIGHT_BENCH_OUTPUT") {
        std::fs::write(
            output,
            format!("{}\n", serde_json::to_string_pretty(&metrics).unwrap()),
        )
        .expect("could not write insight benchmark metrics");
    }
}
#[test]
fn quarter_hour_buckets_agree_with_utc_hours() {
    let db = fixture();
    let result = compute(&db, 1, &json!({})).unwrap();
    let quarters = result["results"]["by_quarter_hour"].as_array().unwrap();
    let mut games = [0_u64; 24];
    let mut points = [0_f64; 24];
    let mut last = i64::MIN;
    for q in quarters {
        let index = q[0].as_i64().unwrap();
        assert!(index > last, "buckets are sorted and distinct");
        last = index;
        let hour = (index * 900).div_euclid(3600).rem_euclid(24) as usize;
        games[hour] += q[1].as_u64().unwrap();
        points[hour] += q[2].as_f64().unwrap();
    }
    for bucket in result["results"]["by_hour_utc"].as_array().unwrap() {
        let hour = bucket["hour_utc"].as_u64().unwrap() as usize;
        assert_eq!(
            games[hour],
            bucket["games"].as_u64().unwrap(),
            "hour {hour}"
        );
        if games[hour] > 0 {
            let score = (1000.0 * points[hour] / games[hour] as f64).round() / 10.0;
            assert!(
                (score - bucket["score"].as_f64().unwrap()).abs() < 0.051,
                "hour {hour}"
            );
        }
    }
}
fn status(service: &Insights, method: &str, path: &str, body: Value) -> u16 {
    match service.handle(method, path, &Query::new(), &body).unwrap() {
        Ok((status, _)) => status,
        Err(error) => error.downcast_ref::<plyback::ApiError>().unwrap().0,
    }
}
#[test]
fn report_titles_are_trimmed_and_missing_reports_cannot_be_deleted() {
    let db = fixture();
    let service = runtime(db.clone());
    let blank = call(
        &service,
        "POST",
        "/api/accounts/1/reports",
        json!({"title":"   "}),
    );
    let default = call(&service, "POST", "/api/accounts/1/reports", json!({}));
    assert_eq!(blank["title"], default["title"]);
    assert!(!blank["title"].as_str().unwrap().trim().is_empty());
    let padded = call(
        &service,
        "POST",
        "/api/accounts/1/reports",
        json!({"title":"  Blitz form  "}),
    );
    assert_eq!(padded["title"], "Blitz form");
    // The limit applies to the trimmed title.
    let long = format!(" {} ", "x".repeat(120));
    assert_eq!(
        status(
            &service,
            "POST",
            "/api/accounts/1/reports",
            json!({ "title": long })
        ),
        201
    );
    assert_eq!(
        status(
            &service,
            "POST",
            "/api/accounts/1/reports",
            json!({"title":"x".repeat(121)})
        ),
        422
    );
    let id = padded["id"].as_i64().unwrap();
    let path = format!("/api/reports/{id}");
    assert_eq!(status(&service, "DELETE", &path, Value::Null), 200);
    assert_eq!(status(&service, "DELETE", &path, Value::Null), 404);
    assert_eq!(
        status(&service, "DELETE", "/api/reports/999999", Value::Null),
        404
    );
    service.stop();
}
#[test]
fn games_without_a_result_are_left_out_of_scores() {
    let db = fixture();
    let before = compute(&db, 1, &json!({})).unwrap();
    // Game 11 (a black win right after the black loss in game 10) stored as PGN `*`.
    db.execute(
        "UPDATE games SET white_result=NULL, black_result=NULL, result='*' WHERE id=11",
        &[],
    )
    .unwrap();
    let after = compute(&db, 1, &json!({})).unwrap();
    let (o, was) = (&after["overview"], &before["overview"]);
    let less = |v: &Value, by: i64| json!(v.as_i64().unwrap() - by);
    assert_eq!(after["data_quality"]["games"], 11);
    assert_eq!(o["games"], less(&was["games"], 1));
    assert_eq!(o["wins"], less(&was["wins"], 1));
    assert_eq!(o["draws"], was["draws"]);
    assert_eq!(o["losses"], was["losses"]);
    assert_eq!(o["black"]["games"], less(&was["black"]["games"], 1));
    assert_eq!(o["black"]["losses"], was["black"]["losses"]);
    assert!(
        after["data_quality"]["notes"]
            .as_array()
            .unwrap()
            .iter()
            .any(|n| n
                .as_str()
                .unwrap()
                .starts_with("1 game(s) in this selection have no recorded result"))
    );
    // Neither a game after a loss nor a loss itself.
    let s = &after["sessions"]["after"];
    assert_eq!(
        s["loss"]["games"],
        less(&before["sessions"]["after"]["loss"]["games"], 1)
    );
    let losses_by: i64 = after["results"]["losses_by"]
        .as_object()
        .unwrap()
        .values()
        .map(|v| v.as_i64().unwrap())
        .sum();
    assert_eq!(json!(losses_by), o["losses"]);
}
#[test]
fn sessions_include_games_hidden_by_filters() {
    let db = fixture();
    // As black: game 2 follows the white loss in game 1 (467 s later) and game 10 the white
    // loss in game 9; both are games after a loss although the filter hides that loss.
    let black = compute(&db, 1, &json!({"color":"black"})).unwrap();
    let s = &black["sessions"];
    assert_eq!(s["after"]["loss"]["games"], 3);
    assert_eq!(s["after"]["loss"]["score"], 66.7);
    assert_eq!(s["after"]["win"]["games"], 0);
    assert_eq!(s["sessions"], 5);
    // Game 10 is the third game of its sitting and game 11 the fourth.
    assert_eq!(s["by_game_in_session"]["4–6"]["games"], 1);
    // Unfiltered, the same sittings.
    let all = compute(&db, 1, &json!({})).unwrap();
    assert_eq!(all["sessions"]["sessions"], 5);
}
#[test]
fn trend_rating_is_null_when_a_period_mixes_time_classes() {
    let db = fixture();
    // All eleven games share one ISO week; the daily game 7 is in another rating pool.
    let all = compute(&db, 1, &json!({})).unwrap();
    let week = &all["trend"][0];
    assert_eq!(week["games"], 11);
    assert!(week["rating"].is_null(), "{week}");
    let blitz = compute(&db, 1, &json!({"time_classes":"blitz"})).unwrap();
    assert_eq!(blitz["trend"][0]["rating"].as_f64(), Some(597.0));
}
#[test]
fn chess960_is_its_own_rating_pool() {
    let db = fixture();
    // The last rated blitz game of the week becomes a live 960 game with a 960 rating.
    let last = db
        .scalar(
            "SELECT id FROM games WHERE time_class='blitz' AND rated=1 ORDER BY end_time DESC,id DESC LIMIT 1",
            &[],
        )
        .unwrap();
    let before = compute(&db, 1, &json!({"time_classes":"blitz"})).unwrap();
    let games = before["ratings"]["blitz"]["games"].as_i64().unwrap();
    db.execute(
        "UPDATE games SET rules='chess960',white_rating=1500,black_rating=1500 WHERE id=?",
        std::slice::from_ref(&last),
    )
    .unwrap();
    let blitz = compute(&db, 1, &json!({"time_classes":"blitz"})).unwrap();
    assert!(blitz["trend"][0]["rating"].is_null(), "{}", blitz["trend"]);
    assert_eq!(blitz["ratings"]["blitz"]["games"], games - 1);
    assert_ne!(blitz["ratings"]["blitz"]["end"].as_f64(), Some(1500.0));
    assert_eq!(blitz["ratings"]["blitz 960"]["games"], 1);
    assert_eq!(blitz["ratings"]["blitz 960"]["end"].as_f64(), Some(1500.0));
    assert_eq!(blitz["ratings"]["blitz 960"]["points"][0]["game_id"], last);
}
#[test]
fn sessions_are_the_same_with_and_without_filters_when_the_account_has_variant_games() {
    let db = fixture();
    // Game 1, the loss game 2 follows 467 s later, was a variant game: not in any statistic,
    // but still a sitting at the board.
    db.execute("UPDATE games SET rules='crazyhouse' WHERE id=1", &[])
        .unwrap();
    let all = compute(&db, 1, &json!({})).unwrap();
    let dated = compute(&db, 1, &json!({"since":0,"until":253402300799_i64})).unwrap();
    assert_eq!(all["overview"]["games"], dated["overview"]["games"]);
    assert_eq!(all["sessions"], dated["sessions"]);
    assert_eq!(all["sessions"]["sessions"], 5);
    let black = compute(&db, 1, &json!({"color":"black"})).unwrap();
    assert_eq!(black["sessions"]["after"]["loss"]["games"], 3);
}
#[test]
fn missed_mates_count_alike_for_both_sides() {
    let db = fixture();
    // Game 1: the account is white, so ply 8 is the opponent's. An inaccuracy that misses a
    // mate counts for the account (see the contract fixture) and so must count for them too.
    db.execute(
        "UPDATE moves SET classification='inaccuracy', tags='{\"missed_mate\":{\"mate\":3}}' WHERE game_id=1 AND ply=8",
        &[],
    )
    .unwrap();
    let r = compute(&db, 1, &json!({})).unwrap();
    let row = r["errors"]["tags"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["tag"] == "missed_mate")
        .unwrap()
        .clone();
    assert!(row["opp_per_game"].as_f64().unwrap() > 0.0, "{row}");
}
