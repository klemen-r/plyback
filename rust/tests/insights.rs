//! Contract oracle generated from the preserved Python baseline and public test fixtures.
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
fn full_insights_match_preserved_python_oracle() {
    let db = fixture();
    let expected: Value =
        serde_json::from_str(include_str!("fixtures/insights-expected.json")).unwrap();
    for case in expected.as_array().unwrap() {
        let mut result = compute(&db, 1, &case["filters"]).unwrap();
        // Native-only extension that the preserved Python oracle does not produce.
        result["results"]
            .as_object_mut()
            .unwrap()
            .remove("by_quarter_hour");
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
    db.transaction(|tx|{tx.execute("INSERT INTO games(uuid,pgn,rules,source,white_username,black_username,white_result,black_result,imported_at,end_time) VALUES('after-report','1. e4 *','chess','pgn','klemen_r','new-opponent','win','resigned',0,1799000000)",[])?;let gid=tx.last_insert_rowid();tx.execute("INSERT INTO account_games(account_id,game_id,color) VALUES(1,?,'white')",[gid])?;Ok(())}).unwrap();
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
    db.transaction(|tx|{tx.execute("DELETE FROM games WHERE id=1",[])?;tx.execute("INSERT INTO games(id,uuid,pgn,rules,source,white_username,black_username,imported_at,end_time) VALUES(1,'different-uuid','1. e4 *','chess','pgn','klemen_r','new',0,0)",[])?;tx.execute("INSERT INTO account_games VALUES(1,1,'white')",[])?;Ok(())}).unwrap();
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
