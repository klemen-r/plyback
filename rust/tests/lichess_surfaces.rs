//! Platform-independent views over reviewed fixture games, never a live database.
use plyback::{
    Query,
    db::Database,
    insights::{Insights, compute},
    practice, repertoire,
    services::Services,
};
use serde_json::{Value, json};
use std::sync::Arc;

fn call(service: &Services, path: &str, query: &Query) -> Value {
    service
        .handle("GET", path, query, &Value::Null)
        .unwrap()
        .unwrap()
        .1
}

#[test]
fn lichess_reviews_reach_games_insights_openings_practice_compare_and_reports() {
    let db = Arc::new(Database::open(":memory:").unwrap());
    db.transaction(|tx| {
        tx.execute_batch(include_str!("fixtures/insights.sql"))?;
        Ok(())
    })
    .unwrap();
    let original = compute(&db, 1, &json!({})).unwrap();
    let original_tree = repertoire::compute(&db, 1, "white", 16, None, None, false).unwrap();
    let original_practice = practice::handle(
        &db,
        "GET",
        "/api/accounts/1/practice",
        &Query::new(),
        &Value::Null,
    )
    .unwrap()
    .unwrap()
    .1;
    // Use the same stored engine reviews to isolate platform and timestamp handling.
    db.transaction(|tx| {
        tx.execute("UPDATE accounts SET platform='lichess'", [])?;
        tx.execute(
            "UPDATE games SET source='lichess',uuid='lichess:'||id,
             url='https://lichess.org/'||id,
             chesscom_accuracy_white=NULL,chesscom_accuracy_black=NULL",
            [],
        )?;
        Ok(())
    })
    .unwrap();
    let services = Services::new(db.clone()).unwrap();
    let q = Query::from([("account_id".into(), "1".into())]);
    let games = call(&services, "/api/games", &q);
    assert_eq!(games["total"], original["overview"]["games"]);
    assert!(games["items"].as_array().unwrap().iter().all(|game| {
        game["source"] == "lichess"
            && game["url"]
                .as_str()
                .unwrap()
                .starts_with("https://lichess.org/")
            && game["analyzed"] == true
    }));
    let review = call(&services, "/api/games/1", &q);
    assert!(review["analysis"].is_object());
    assert!(!review["moves"].as_array().unwrap().is_empty());
    assert_eq!(review["game"]["source"], "lichess");
    let queued = services
        .handle(
            "POST",
            "/api/games/1/analyze",
            &Query::new(),
            &json!({"force":true}),
        )
        .unwrap()
        .unwrap()
        .1;
    assert_eq!(queued["queued"], true);
    // The pending refresh must not hold the immutable report below open.
    db.execute("DELETE FROM jobs WHERE game_id=1", &[]).unwrap();
    assert!(
        !call(&services, "/api/games/openings", &q)
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        repertoire::compute(&db, 1, "white", 16, None, None, false).unwrap(),
        original_tree
    );
    let practice = practice::handle(
        &db,
        "GET",
        "/api/accounts/1/practice",
        &Query::new(),
        &Value::Null,
    )
    .unwrap()
    .unwrap()
    .1;
    assert_eq!(practice["stats"], original_practice["stats"]);
    // Compare consumes these same metrics; imported platform scores cannot affect them.
    let current = compute(&db, 1, &json!({})).unwrap();
    for metric in [
        "games",
        "score",
        "accuracy",
        "acpl",
        "first_game",
        "last_game",
    ] {
        assert_eq!(
            current["overview"][metric], original["overview"][metric],
            "{metric}"
        );
    }
    assert_eq!(current["overview"]["chesscom_accuracy_games"], 0);
    let insights = Insights::new(db.clone(), services);
    let report = insights
        .handle("POST", "/api/accounts/1/reports", &Query::new(), &json!({}))
        .unwrap()
        .unwrap()
        .1;
    let id = report["id"].as_i64().unwrap();
    insights
        .handle(
            "POST",
            &format!("/api/reports/{id}/finalize"),
            &Query::new(),
            &json!({}),
        )
        .unwrap()
        .unwrap();
    let frozen = insights
        .handle(
            "GET",
            &format!("/api/reports/{id}"),
            &Query::new(),
            &Value::Null,
        )
        .unwrap()
        .unwrap()
        .1;
    assert_eq!(frozen["status"], "ready");
    assert_eq!(frozen["report"]["overview"], current["overview"]);
    db.execute(
        "UPDATE games SET white_result='agreed',black_result='agreed'",
        &[],
    )
    .unwrap();
    let after = insights
        .handle(
            "GET",
            &format!("/api/reports/{id}"),
            &Query::new(),
            &Value::Null,
        )
        .unwrap()
        .unwrap()
        .1;
    assert_eq!(frozen, after);
    insights.stop();
}
