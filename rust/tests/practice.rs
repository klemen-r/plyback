//! Practice queue, answers and scheduling against a small synthetic database.
use plyback::{Query, db::Database, practice};
use serde_json::{Value, json};

const MATE_FEN: &str = "r1bqkb1r/pppp1ppp/2n2n2/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR w KQkq - 4 4";

fn fixture() -> Database {
    let db = Database::open(":memory:").unwrap();
    db.transaction(|tx| {
        tx.execute_batch(
            "INSERT INTO accounts(id, username, added_at) VALUES (1, 'me', 0), (2, 'other', 0);
             INSERT INTO games(id, uuid, pgn, rules, end_time, white_username, black_username,
                               white_rating, black_rating, time_class, imported_at)
             VALUES (10, 'g10', '', 'chess', 2000, 'me', 'rival', 600, 640, 'blitz', 0),
                    (11, 'g11', '', 'chess', 1000, 'rival', 'me', 650, 610, 'blitz', 0),
                    (12, 'g12', '', 'chess960', 3000, 'me', 'rival', 600, 640, 'blitz', 0);
             INSERT INTO account_games(account_id, game_id, color)
             VALUES (1, 10, 'white'), (1, 11, 'black'), (1, 12, 'white');",
        )?;
        let mv = |game: i64,
                  ply: i64,
                  color: &str,
                  san: &str,
                  uci: &str,
                  fen: &str,
                  class: &str,
                  best: &str,
                  best_san: &str,
                  lines: Value,
                  phase: &str| {
            tx.execute(
                "INSERT INTO moves(game_id, ply, color, san, uci, fen_before, epd_before,
                                   classification, best_uci, best_san, lines_json, ep_loss, phase,
                                   explanation)
                 VALUES (?,?,?,?,?,?,'',?,?,?,?,0.4,?,'Missed mate.')",
                rusqlite::params![
                    game,
                    ply,
                    color,
                    san,
                    uci,
                    fen,
                    class,
                    best,
                    best_san,
                    lines.to_string(),
                    phase
                ],
            )
        };
        let mate =
            json!([{"cp":99999,"uci":"h5f7","san":["Qxf7#"]},{"cp":30,"uci":"d2d3","san":["d3"]}]);
        // The previous move, so the queue can highlight it.
        mv(
            10,
            6,
            "b",
            "Nf6",
            "g8f6",
            "r1bqkbnr/pppp1ppp/2n5/4p2Q/2B1P3/8/PPPP1PPP/RNB1K1NR b KQkq - 3 3",
            "blunder",
            "g7g6",
            "g6",
            json!([]),
            "opening",
        )?;
        // Qualifies: the account's own blunder with one clear answer.
        mv(
            10,
            7,
            "w",
            "Nc3",
            "b1c3",
            MATE_FEN,
            "blunder",
            "h5f7",
            "Qxf7#",
            mate.clone(),
            "opening",
        )?;
        // Two near-equal best moves: no single fair answer.
        mv(
            10,
            9,
            "w",
            "a3",
            "a2a3",
            MATE_FEN,
            "mistake",
            "h5f7",
            "Qxf7#",
            json!([{"cp":300,"uci":"h5f7","san":[]},{"cp":290,"uci":"d2d3","san":[]}]),
            "middlegame",
        )?;
        // Only an inaccuracy.
        mv(
            10,
            11,
            "w",
            "h3",
            "h2h3",
            MATE_FEN,
            "inaccuracy",
            "h5f7",
            "Qxf7#",
            mate.clone(),
            "middlegame",
        )?;
        // Black's move in game 11 is the account's own; White's blunder is the opponent's.
        mv(
            11,
            8,
            "b",
            "a6",
            "a7a6",
            "r1bqk2r/pppp1ppp/2n2n2/2b1p2Q/2B1P3/2N5/PPPP1PPP/R1B1K1NR b KQkq - 6 5",
            "miss",
            "f6h5",
            "Nxh5",
            json!([{"cp":-500,"uci":"f6h5","san":["Nxh5"]},{"cp":200,"uci":"e8g8","san":["O-O"]}]),
            "middlegame",
        )?;
        mv(
            11,
            7,
            "w",
            "Nc3",
            "b1c3",
            MATE_FEN,
            "blunder",
            "h5f7",
            "Qxf7#",
            mate.clone(),
            "opening",
        )?;
        // Chess960 games are left out.
        mv(
            12, 7, "w", "Nc3", "b1c3", MATE_FEN, "blunder", "h5f7", "Qxf7#", mate, "opening",
        )?;
        Ok(())
    })
    .unwrap();
    db
}

fn get(db: &Database, path: &str, q: &[(&str, &str)]) -> (u16, Value) {
    let q: Query = q
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    practice::handle(db, "GET", path, &q, &Value::Null)
        .unwrap()
        .unwrap()
}
fn post(db: &Database, path: &str, body: Value) -> anyhow::Result<(u16, Value)> {
    practice::handle(db, "POST", path, &Query::new(), &body).unwrap()
}
fn status(error: anyhow::Error) -> u16 {
    error.downcast_ref::<plyback::ApiError>().unwrap().0
}

#[test]
fn queue_offers_only_the_accounts_own_clear_mistakes_newest_first() {
    let db = fixture();
    let (code, body) = get(&db, "/api/accounts/1/practice", &[]);
    assert_eq!(code, 200);
    let items = body["items"].as_array().unwrap();
    let keys: Vec<_> = items
        .iter()
        .map(|i| (i["game_id"].as_i64().unwrap(), i["ply"].as_i64().unwrap()))
        .collect();
    assert_eq!(keys, [(10, 7), (11, 8)]);
    assert_eq!(body["stats"]["total"], 2);
    assert_eq!(body["stats"]["new"], 2);
    let first = &items[0];
    assert_eq!(first["color"], "white");
    assert_eq!(first["move_number"], 4);
    assert_eq!(first["last_uci"], "g8f6");
    assert_eq!(first["opponent"], "rival");
    assert_eq!(first["opponent_rating"], 640);
    assert_eq!(first["status"], "new");
    assert!(
        first["themes"]
            .as_array()
            .unwrap()
            .contains(&json!("mate-in-1"))
    );
    // The answer is only revealed after an attempt.
    assert!(first.get("best_uci").is_none() && first.get("best_san").is_none());
    assert_eq!(items[1]["color"], "black");
    // Filters.
    let (_, misses) = get(&db, "/api/accounts/1/practice", &[("kind", "miss")]);
    assert_eq!(misses["items"].as_array().unwrap().len(), 1);
    let (_, endgames) = get(&db, "/api/accounts/1/practice", &[("phase", "endgame")]);
    assert_eq!(endgames["stats"]["total"], 0);
    let (_, one) = get(&db, "/api/accounts/1/practice", &[("limit", "1")]);
    assert_eq!(one["items"].as_array().unwrap().len(), 1);
    // A range covers only recent games, and includes positions already answered.
    post(&db, "/api/accounts/1/practice/10/7", json!({"uci": "h5f7"})).unwrap();
    let (_, recent) = get(&db, "/api/accounts/1/practice", &[("since", "1500")]);
    assert_eq!(recent["stats"]["total"], 1);
    assert_eq!(recent["items"][0]["game_id"], 10);
    assert_eq!(recent["items"][0]["status"], "scheduled");
    let (_, all) = get(&db, "/api/accounts/1/practice", &[("since", "0")]);
    assert_eq!(all["items"].as_array().unwrap().len(), 2);
    let q: Query = [("since".to_string(), "today".to_string())].into();
    let error = practice::handle(&db, "GET", "/api/accounts/1/practice", &q, &Value::Null).unwrap();
    assert_eq!(status(error.unwrap_err()), 422);
    // Another account has nothing.
    assert_eq!(
        get(&db, "/api/accounts/2/practice", &[]).1["stats"]["total"],
        0
    );
}

#[test]
fn answers_are_checked_and_scheduled() {
    let db = fixture();
    let path = "/api/accounts/1/practice/10/7";
    let (_, wrong) = post(&db, path, json!({"uci": "d2d3"})).unwrap();
    assert_eq!(wrong["correct"], false);
    assert_eq!(wrong["san"], "d3");
    assert_eq!(wrong["best_san"], "Qxf7#");
    assert_eq!(wrong["game_san"], "Nc3");
    assert_eq!(wrong["line_san"], json!(["Qxf7#"]));
    assert!(
        wrong["themes"]
            .as_array()
            .unwrap()
            .contains(&json!("mate-in-1"))
    );
    assert_eq!(wrong["box"], 0);
    // Missed positions return shortly, so it is no longer due right now.
    let (_, queue) = get(&db, "/api/accounts/1/practice", &[]);
    assert_eq!(queue["stats"]["learning"], 1);
    assert_eq!(queue["stats"]["due"], 0);
    assert_eq!(queue["items"].as_array().unwrap().len(), 1);
    // Answering again before it is due (a second tab) changes nothing.
    let (_, again) = post(&db, path, json!({"uci": "d2d3"})).unwrap();
    assert_eq!(again["correct"], false);
    let attempts: i64 = db
        .read(|c| {
            Ok(c.query_row(
                "SELECT attempts FROM practice_items WHERE game_id=10 AND ply=7",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(attempts, 1);
    // Later, once it is due again, the next answer counts.
    db.execute("UPDATE practice_items SET due_at=0", &[])
        .unwrap();
    let (_, right) = post(&db, path, json!({"uci": "h5f7"})).unwrap();
    assert_eq!(right["correct"], true);
    assert_eq!(right["box"], 1);
    let (attempts, correct, lapses): (i64, i64, i64) = db
        .read(|c| Ok(c.query_row(
            "SELECT attempts, correct, lapses FROM practice_items WHERE account_id=1 AND game_id=10 AND ply=7",
            [], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?))
        .unwrap();
    assert_eq!((attempts, correct, lapses), (2, 1, 0));
    // Answering it correctly again before it is due, after the 10-minute guard (the Today range
    // lists positions whether due or not), is practice: it stays in its box.
    db.execute(
        "UPDATE practice_items SET last_seen_at=last_seen_at-700",
        &[],
    )
    .unwrap();
    let (_, early) = post(&db, path, json!({"uci": "h5f7"})).unwrap();
    assert_eq!(early["correct"], true);
    assert_eq!(early["box"], 1);
    // A due position comes before new ones.
    db.execute("UPDATE practice_items SET due_at=0", &[])
        .unwrap();
    let (_, queue) = get(&db, "/api/accounts/1/practice", &[]);
    assert_eq!(queue["items"][0]["status"], "due");
    assert_eq!(queue["items"][0]["ply"], 7);
    // Revealing the answer counts as a miss, and a second miss counts as a lapse.
    let (_, revealed) = post(&db, path, json!({"reveal": true})).unwrap();
    assert_eq!(revealed["correct"], false);
    assert_eq!(revealed["uci"], Value::Null);
    assert_eq!(revealed["best_san"], "Qxf7#");
    let lapses: i64 = db
        .read(|c| {
            Ok(c.query_row(
                "SELECT lapses FROM practice_items WHERE game_id=10 AND ply=7",
                [],
                |r| r.get(0),
            )?)
        })
        .unwrap();
    assert_eq!(lapses, 1);
}

#[test]
fn invalid_requests_are_rejected() {
    let db = fixture();
    for (path, body, expected) in [
        ("/api/accounts/1/practice/10/7", json!({"uci": "e1e5"}), 422),
        (
            "/api/accounts/1/practice/10/7",
            json!({"uci": "nonsense"}),
            422,
        ),
        ("/api/accounts/1/practice/10/7", json!({}), 422),
        (
            "/api/accounts/1/practice/10/7",
            json!({"reveal": "yes"}),
            422,
        ),
        ("/api/accounts/1/practice/10/9", json!({"uci": "h5f7"}), 404),
        ("/api/accounts/1/practice/11/7", json!({"uci": "h5f7"}), 404),
        ("/api/accounts/1/practice/12/7", json!({"uci": "h5f7"}), 404),
        ("/api/accounts/9/practice/10/7", json!({"uci": "h5f7"}), 404),
        ("/api/accounts/x/practice/10/7", json!({"uci": "h5f7"}), 422),
    ] {
        assert_eq!(
            status(post(&db, path, body).unwrap_err()),
            expected,
            "{path}"
        );
    }
    let q: Query = [("kind".to_string(), "brilliant".to_string())].into();
    let error = practice::handle(&db, "GET", "/api/accounts/1/practice", &q, &Value::Null).unwrap();
    assert_eq!(status(error.unwrap_err()), 422);
    let q: Query = [("theme".to_string(), "made-up-theme".to_string())].into();
    let error = practice::handle(&db, "GET", "/api/accounts/1/practice", &q, &Value::Null).unwrap();
    assert_eq!(status(error.unwrap_err()), 422);
    let error = practice::handle(
        &db,
        "DELETE",
        "/api/accounts/1/practice",
        &Query::new(),
        &Value::Null,
    )
    .unwrap();
    assert_eq!(status(error.unwrap_err()), 405);
    assert!(
        practice::handle(
            &db,
            "GET",
            "/api/accounts/1/insights",
            &Query::new(),
            &Value::Null
        )
        .is_none()
    );
}

#[test]
fn theme_filters_scope_statistics_and_counts_respect_the_other_filters() {
    let db = fixture();
    let path = "/api/accounts/1/practice";
    let (_, all) = get(&db, path, &[]);
    assert_eq!(all["theme_counts"]["mate-in-1"], 1);
    assert_eq!(
        all["theme_counts"].as_object().unwrap().len(),
        plyback::tactics::THEMES.len()
    );

    let (_, mates) = get(&db, path, &[("theme", "mate-in-1"), ("limit", "1")]);
    assert_eq!(mates["stats"]["total"], 1);
    assert_eq!(mates["stats"]["new"], 1);
    assert_eq!(mates["items"][0]["game_id"], 10);
    // The selected theme and item limit do not narrow facet counts.
    assert_eq!(mates["theme_counts"], all["theme_counts"]);

    let (_, misses) = get(&db, path, &[("kind", "miss"), ("theme", "mate-in-1")]);
    assert_eq!(misses["stats"]["total"], 0);
    assert_eq!(misses["theme_counts"]["mate-in-1"], 0);
    let (_, opening) = get(&db, path, &[("phase", "opening")]);
    assert_eq!(opening["theme_counts"]["mate-in-1"], 1);
    let (_, ending) = get(&db, path, &[("phase", "endgame")]);
    assert!(
        ending["theme_counts"]
            .as_object()
            .unwrap()
            .values()
            .all(|v| v == 0)
    );
    let (_, future) = get(&db, path, &[("since", "2001")]);
    assert!(
        future["theme_counts"]
            .as_object()
            .unwrap()
            .values()
            .all(|v| v == 0)
    );

    post(
        &db,
        "/api/accounts/1/practice/10/7",
        json!({"reveal": true}),
    )
    .unwrap();
    let (_, scheduled) = get(&db, path, &[("theme", "mate-in-1")]);
    assert_eq!(scheduled["stats"]["total"], 1);
    assert_eq!(scheduled["stats"]["learning"], 1);
    assert_eq!(scheduled["stats"]["new"], 0);
    assert_eq!(scheduled["theme_counts"]["mate-in-1"], 1);
    assert_eq!(scheduled["items"], json!([]));
    let (_, range) = get(&db, path, &[("theme", "mate-in-1"), ("since", "0")]);
    assert_eq!(range["items"][0]["game_id"], 10);
    for theme in ["", "all"] {
        assert_eq!(get(&db, path, &[("theme", theme)]).1["stats"]["total"], 2);
    }
}

#[test]
fn themes_follow_the_current_stored_position_and_line_without_changing_review_data() {
    let db = fixture();
    let path = "/api/accounts/1/practice";
    let before = db
        .read(|c| {
            Ok(c.query_row(
                "SELECT lines_json FROM moves WHERE game_id=10 AND ply=7",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })
        .unwrap();
    get(&db, path, &[]);
    assert_eq!(
        db.read(|c| {
            Ok(c.query_row(
                "SELECT lines_json FROM moves WHERE game_id=10 AND ply=7",
                [],
                |r| r.get::<_, String>(0),
            )?)
        })
        .unwrap(),
        before
    );
    db.transaction(|tx| {
        tx.execute("UPDATE moves SET fen_before=?1, best_uci='e2e4', best_san='e4', lines_json=?2 WHERE game_id=10 AND ply=7", rusqlite::params![
            plyback::pgn::STARTING_FEN,
            json!([{"cp":500,"uci":"e2e4","san":["e4"]},{"cp":-500,"uci":"g1f3","san":["Nf3"]}]).to_string()
        ])?;
        tx.execute("UPDATE data_revision SET revision=revision+1 WHERE id=1", [])?;
        Ok(())
    }).unwrap();
    let (_, changed) = get(&db, path, &[]);
    assert_eq!(changed["items"][0]["themes"], json!([]));
    assert_eq!(changed["theme_counts"]["mate-in-1"], 0);
    assert_eq!(
        get(&db, path, &[("theme", "mate-in-1")]).1["stats"]["total"],
        0
    );
    assert_eq!(get(&db, path, &[("theme", "all")]).1["stats"]["total"], 2);
}

/// Reproducible throughput check using only in-memory fixture data. Repeated
/// positions measure list/detection overhead, not coverage on real game data.
#[test]
#[ignore = "manual release-mode Practice throughput measurement"]
fn profile_three_thousand_theme_candidates() {
    let db = fixture();
    db.transaction(|tx| {
        tx.execute_batch(
            "WITH RECURSIVE ids(id) AS (SELECT 100 UNION ALL SELECT id+1 FROM ids WHERE id<3099)
             INSERT INTO games(id, uuid, pgn, rules, end_time, white_username, black_username, imported_at)
             SELECT id, 'theme-profile-' || id, '', 'chess', 2000, 'me', 'rival', 0 FROM ids;
             INSERT INTO account_games(account_id, game_id, color)
             SELECT 1, id, 'white' FROM games WHERE id>=100;
             INSERT INTO moves(game_id, ply, color, san, uci, fen_before, epd_before,
                               classification, best_uci, best_san, lines_json, ep_loss, phase)
             SELECT g.id, m.ply, m.color, m.san, m.uci, m.fen_before, m.epd_before,
                    m.classification, m.best_uci, m.best_san, m.lines_json, m.ep_loss, m.phase
             FROM games g CROSS JOIN moves m WHERE g.id>=100 AND m.game_id=10 AND m.ply=7;"
        )?;
        Ok(())
    }).unwrap();
    for iteration in 0..3 {
        let start = std::time::Instant::now();
        let (_, queue) = get(&db, "/api/accounts/1/practice", &[("theme", "mate-in-1")]);
        assert_eq!(queue["stats"]["total"], 3001);
        assert_eq!(queue["theme_counts"]["mate-in-1"], 3001);
        eprintln!(
            "Practice themes: 3002 fixture candidates, iteration {iteration}: {:?}",
            start.elapsed()
        );
    }
}

#[test]
fn deleting_a_game_or_account_removes_its_schedule() {
    let db = fixture();
    post(&db, "/api/accounts/1/practice/10/7", json!({"uci": "h5f7"})).unwrap();
    post(&db, "/api/accounts/1/practice/11/8", json!({"uci": "f6h5"})).unwrap();
    db.execute("DELETE FROM games WHERE id=10", &[]).unwrap();
    assert_eq!(
        db.scalar("SELECT COUNT(*) FROM practice_items", &[])
            .unwrap(),
        1
    );
    db.execute("DELETE FROM accounts WHERE id=1", &[]).unwrap();
    assert_eq!(
        db.scalar("SELECT COUNT(*) FROM practice_items", &[])
            .unwrap(),
        0
    );
}
