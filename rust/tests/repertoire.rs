//! Opening repertoire tree against a small synthetic database.
use plyback::{Query, db::Database, repertoire};
use serde_json::Value;

/// (game id, account colour, white result, black result, end time, rules, moves)
type GameSpec<'a> = (
    i64,
    &'a str,
    &'a str,
    &'a str,
    i64,
    &'a str,
    &'a [(&'a str, bool, &'a str)],
);

fn fixture(games: &[GameSpec]) -> Database {
    let db = Database::open(":memory:").unwrap();
    db.transaction(|tx| {
        tx.execute(
            "INSERT INTO accounts(id, username, added_at) VALUES (1, 'me', 0)",
            [],
        )?;
        for (id, color, white, black, end, rules, moves) in games {
            tx.execute(
                "INSERT INTO games(id, uuid, pgn, rules, end_time, white_result, black_result,
                                   time_class, imported_at)
                 VALUES (?1, ?1, '', ?2, ?3, ?4, ?5, 'blitz', 0)",
                rusqlite::params![id, rules, end, white, black],
            )?;
            tx.execute(
                "INSERT INTO account_games(account_id, game_id, color) VALUES (1, ?, ?)",
                rusqlite::params![id, color],
            )?;
            for (i, (san, book, class)) in moves.iter().enumerate() {
                let ply = i as i64 + 1;
                tx.execute(
                    "INSERT INTO moves(game_id, ply, color, san, uci, fen_before, epd_before,
                                       is_book, classification, ep_loss)
                     VALUES (?, ?, ?, ?, '', '', '', ?, ?, ?)",
                    rusqlite::params![
                        id,
                        ply,
                        if ply % 2 == 1 { "w" } else { "b" },
                        san,
                        *book as i64,
                        class,
                        if *class == "blunder" { 0.3 } else { 0.0 }
                    ],
                )?;
            }
        }
        Ok(())
    })
    .unwrap();
    db
}

fn get(db: &Database, q: &[(&str, &str)]) -> Value {
    let q: Query = q
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
    repertoire::handle(db, "GET", "/api/accounts/1/repertoire", &q)
        .unwrap()
        .unwrap()
        .1
}
fn node<'a>(tree: &'a Value, path: &str) -> &'a Value {
    tree["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["path"] == path)
        .unwrap_or_else(|| panic!("{path}"))
}

const SCOTCH: &[(&str, bool, &str)] = &[
    ("e4", true, "book"),
    ("e5", true, "book"),
    ("Nf3", true, "book"),
    ("Nc6", true, "book"),
    ("Bc4", false, "blunder"),
];
const SICILIAN: &[(&str, bool, &str)] = &[
    ("e4", true, "book"),
    ("c5", true, "book"),
    ("Nf3", false, "best"),
];

#[test]
fn builds_a_move_tree_with_results_book_and_own_errors() {
    let db = fixture(&[
        (1, "white", "win", "resigned", 300, "chess", SCOTCH),
        (2, "white", "checkmated", "win", 200, "chess", SCOTCH),
        (3, "white", "agreed", "agreed", 100, "chess", SICILIAN),
        (4, "black", "win", "resigned", 400, "chess", SICILIAN),
        (5, "white", "win", "resigned", 500, "chess960", SCOTCH),
    ]);
    let tree = get(&db, &[]);
    assert_eq!(tree["color"], "white");
    assert_eq!(
        (tree["games"].as_i64(), tree["total_games"].as_i64()),
        (Some(3), Some(3))
    );
    assert_eq!(
        (
            tree["wins"].as_i64(),
            tree["draws"].as_i64(),
            tree["losses"].as_i64()
        ),
        (Some(1), Some(1), Some(1))
    );
    let e4 = node(&tree, "e4");
    assert_eq!(e4["games"], 3);
    assert_eq!(e4["score"], 0.5);
    assert_eq!(e4["mine"], true);
    assert_eq!(e4["book"], 3);
    assert_eq!(e4["errors"], 0);
    assert_eq!(e4["last_played"], 300);
    let e5 = node(&tree, "e4 e5");
    assert_eq!(e5["mine"], false);
    assert_eq!(e5["errors"], Value::Null);
    let bc4 = node(&tree, "e4 e5 Nf3 Nc6 Bc4");
    assert_eq!(
        (
            bc4["games"].as_i64(),
            bc4["errors"].as_i64(),
            bc4["blunders"].as_i64()
        ),
        (Some(2), Some(2), Some(2))
    );
    assert_eq!(bc4["left_book"], 2);
    assert!((bc4["avg_loss"].as_f64().unwrap() - 0.3).abs() < 1e-9);
    assert_eq!(node(&tree, "e4 c5 Nf3")["left_book"], 1);
    // Positions are named from the bundled opening dataset, and moves link to recent games.
    assert!(
        node(&tree, "e4 e5 Nf3 Nc6 Bc4")["opening"]
            .as_str()
            .unwrap()
            .starts_with("Italian Game")
    );
    assert!(
        node(&tree, "e4 c5")["opening"]
            .as_str()
            .unwrap()
            .starts_with("Sicilian Defense")
    );
    assert_eq!(
        node(&tree, "e4")["recent_games"],
        serde_json::json!([1, 2, 3])
    );
    assert_eq!(bc4["recent_games"], serde_json::json!([1, 2]));
    // Two games are not enough to list a problem.
    assert_eq!(tree["problems"].as_array().unwrap().len(), 0);
    // Black's tree only holds the game played as Black.
    let black = get(&db, &[("color", "black")]);
    assert_eq!(black["games"], 1);
    assert_eq!(node(&black, "e4 c5")["mine"], true);
    assert_eq!(node(&black, "e4")["mine"], false);
    // Depth and filters.
    let short = get(&db, &[("plies", "2")]);
    assert!(
        short["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|n| n["ply"].as_i64().unwrap() <= 2)
    );
    assert_eq!(get(&db, &[("since", "250")])["games"], 1);
    assert_eq!(get(&db, &[("time_class", "bullet")])["games"], 0);
}

#[test]
fn repeated_errors_are_listed_as_problems() {
    let db = fixture(&[
        (1, "white", "checkmated", "win", 100, "chess", SCOTCH),
        (2, "white", "resigned", "win", 200, "chess", SCOTCH),
        (3, "white", "win", "resigned", 300, "chess", SCOTCH),
        (4, "white", "win", "resigned", 400, "chess", SICILIAN),
    ]);
    let tree = get(&db, &[]);
    let problems = tree["problems"].as_array().unwrap();
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0]["path"], "e4 e5 Nf3 Nc6 Bc4");
    assert_eq!(problems[0]["errors"], 3);
}

#[test]
fn invalid_requests_are_rejected() {
    let db = fixture(&[]);
    for q in [
        [("color", "red")],
        [("plies", "99")],
        [("time_class", "classical")],
        [("since", "soon")],
    ] {
        let q: Query = q
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let error = repertoire::handle(&db, "GET", "/api/accounts/1/repertoire", &q)
            .unwrap()
            .unwrap_err();
        assert_eq!(error.downcast_ref::<plyback::ApiError>().unwrap().0, 422);
    }
    let missing =
        repertoire::handle(&db, "GET", "/api/accounts/7/repertoire", &Query::new()).unwrap();
    assert_eq!(
        missing
            .unwrap_err()
            .downcast_ref::<plyback::ApiError>()
            .unwrap()
            .0,
        404
    );
    let post =
        repertoire::handle(&db, "POST", "/api/accounts/1/repertoire", &Query::new()).unwrap();
    assert_eq!(
        post.unwrap_err()
            .downcast_ref::<plyback::ApiError>()
            .unwrap()
            .0,
        405
    );
    assert!(repertoire::handle(&db, "GET", "/api/accounts/1/insights", &Query::new()).is_none());
}
