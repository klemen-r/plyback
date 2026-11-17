use plyback::{
    analysis::{self, AnalysisOptions},
    engine::{Engine, locate_stockfish},
    pgn,
};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

fn real_engine() -> Option<Engine> {
    match locate_stockfish() {
        Ok(path) => Some(Engine::with_path(path, 1, 64).expect("Stockfish starts")),
        Err(e) => {
            assert!(
                std::env::var_os("PLYBACK_REQUIRE_STOCKFISH").is_none(),
                "Stockfish required: {e}"
            );
            eprintln!("Stockfish unavailable; set PLYBACK_REQUIRE_STOCKFISH=1 in acceptance CI");
            None
        }
    }
}

#[test]
fn real_stockfish_review_mates_sacrifice_and_hanging_queen() {
    let Some(mut engine) = real_engine() else {
        return;
    };
    let cancel = AtomicBool::new(false);
    let options = AnalysisOptions {
        depth: 14,
        time_cap: 2.0,
        ..Default::default()
    };
    let start = engine
        .analyse(pgn::STARTING_FEN, 12, 2, 2.0, &cancel)
        .unwrap();
    assert_eq!(start.len(), 2);
    assert!(start[0]["cp"].as_i64().unwrap().abs() < 150);
    let white = engine
        .analyse("6k1/5ppp/8/8/8/8/5PPP/3R2K1 w - - 0 1", 12, 1, 2.0, &cancel)
        .unwrap();
    assert_eq!(white[0]["pv"][0], "d1d8");
    assert_eq!(white[0]["mate"], 1);
    let black = engine
        .analyse("3r2k1/5ppp/8/8/8/8/5PPP/6K1 b - - 0 1", 12, 1, 2.0, &cancel)
        .unwrap();
    assert_eq!(black[0]["pv"][0], "d8d1");
    assert_eq!(black[0]["mate"], -1);
    let scholars = analysis::analyze(
        &mut engine,
        "1. e4 e5 2. Bc4 Nc6 3. Qh5 Nf6 4. Qxf7# 1-0",
        &options,
        &cancel,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(scholars["moves"][5]["classification"], "blunder");
    assert!(scholars["moves"][5]["tags"].get("allowed_mate").is_some());
    assert!(
        scholars["moves"][5]["explanation"]
            .as_str()
            .unwrap()
            .to_lowercase()
            .contains("mate")
    );
    assert_eq!(scholars["moves"][6]["classification"], "best");
    assert!(scholars["moves"][6]["tags"].get("missed_mate").is_none());
    assert_eq!(scholars["moves"][0]["classification"], "book");
    assert!(!scholars["opening"].is_null());
    assert!(
        scholars["accuracy_white"].as_f64().unwrap() > scholars["accuracy_black"].as_f64().unwrap()
    );
    let legal = analysis::analyze(
        &mut engine,
        "1. e4 e5 2. Nf3 d6 3. Bc4 Bg4 4. Nc3 g6 5. Nxe5 Bxd1 6. Bxf7+ Ke7 7. Nd5# 1-0",
        &options,
        &cancel,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(legal["moves"][8]["classification"], "brilliant");
    assert!(
        legal["moves"][8]["explanation"]
            .as_str()
            .unwrap()
            .contains("sacrifice")
    );
    assert_eq!(legal["moves"][9]["classification"], "blunder");
    assert_eq!(legal["moves"][11]["classification"], "forced");
    assert_eq!(legal["counts"]["white"]["brilliant"], 1);
    let queen = analysis::analyze(
        &mut engine,
        "1. e4 e5 2. Nf3 Nc6 3. d4 Qh4 4. Nxh4 *",
        &options,
        &cancel,
        |_, _| {},
    )
    .unwrap();
    assert_eq!(queen["moves"][5]["classification"], "blunder");
    assert!(queen["moves"][5]["tags"].get("hanging_piece").is_some());
    let best_san = queen["moves"][5]["best_san"].as_str().unwrap();
    assert!(!best_san.is_empty());
    assert_ne!(best_san, "Qh4");
    assert!(
        queen["moves"][5]["explanation"]
            .as_str()
            .unwrap()
            .contains("queen")
    );
    assert!(matches!(
        queen["moves"][6]["classification"].as_str(),
        Some("best" | "excellent")
    ));
    let shilling = analysis::analyze(
        &mut engine,
        "1. e4 e5 2. Nf3 Nc6 3. Bc4 Nd4 4. Nxe5 Qg5 5. Nxf7 Qxg2 6. Rf1 Qxe4+ 7. Be2 Nf3# 0-1",
        &options,
        &cancel,
        |_, _| {},
    )
    .unwrap();
    assert!(matches!(
        shilling["moves"][8]["classification"].as_str(),
        Some("mistake" | "blunder")
    ));
    assert!(matches!(
        shilling["moves"][9]["classification"].as_str(),
        Some("best" | "great")
    ));
    assert!(shilling["moves"][9]["tags"].get("sacrifice").is_none());
    assert!(
        shilling["accuracy_black"].as_f64().unwrap() > shilling["accuracy_white"].as_f64().unwrap()
    );
}

#[test]
fn real_stockfish_cancel_reaps_and_next_request_recovers() {
    let Some(mut engine) = real_engine() else {
        return;
    };
    let cancel = Arc::new(AtomicBool::new(false));
    let setter = cancel.clone();
    let thread = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(80));
        setter.store(true, Ordering::SeqCst);
    });
    let start = Instant::now();
    let error = engine
        .analyse(pgn::STARTING_FEN, 32, 3, 60.0, &cancel)
        .unwrap_err();
    thread.join().unwrap();
    assert_eq!(error.downcast_ref::<plyback::ApiError>().unwrap().0, 499);
    assert!(start.elapsed() < Duration::from_secs(2));
    let result = engine
        .analyse(pgn::STARTING_FEN, 8, 1, 1.0, &AtomicBool::new(false))
        .unwrap();
    assert_eq!(result.len(), 1);
}

#[test]
fn durable_cache_is_database_owned_and_history_sensitive() {
    let Some(mut engine) = real_engine() else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(plyback::db::Database::open(dir.path().join("test.db")).unwrap());
    engine.attach_database(db.clone());
    let cancel = AtomicBool::new(false);
    let request = plyback::engine::Search::position(pgn::STARTING_FEN, 8, 2, 1.0);
    let first = engine.search(&request, &cancel).unwrap();
    let rows = db
        .scalar("SELECT COUNT(*) FROM position_cache", &[])
        .unwrap();
    assert_eq!(rows, 1);
    let second = engine.search(&request, &cancel).unwrap();
    assert_eq!(first, second);
    let mut history = request.clone();
    history.moves = ["g1f3", "g8f6", "f3g1", "f6g8"].map(String::from).to_vec();
    engine.search(&history, &cancel).unwrap();
    assert_eq!(
        db.scalar("SELECT COUNT(*) FROM position_cache", &[])
            .unwrap(),
        2
    );
    let other = Arc::new(plyback::db::Database::open(dir.path().join("other.db")).unwrap());
    engine.attach_database(other.clone());
    engine.search(&request, &cancel).unwrap();
    assert_eq!(
        other
            .scalar("SELECT COUNT(*) FROM position_cache", &[])
            .unwrap(),
        1
    );
}

/// Explicit local benchmark; no live data and no reduced search budgets.
/// PLYBACK_BENCH_INPUT selects a preserved recording with games and UCI evaluations.
/// PLYBACK_BENCH_OUTPUT selects the JSON metrics file. Run with --release --ignored.
#[test]
#[ignore = "real-engine benchmark; run explicitly with fixture capture and output paths"]
fn benchmark_reference_fixture_cold_and_warm() {
    use plyback::engine::{Line, Search};
    use serde_json::{Value, json};
    let input = std::env::var_os("PLYBACK_BENCH_INPUT")
        .expect("Set PLYBACK_BENCH_INPUT to a fixture-only capture");
    let source: Value = serde_json::from_slice(&std::fs::read(input).unwrap()).unwrap();
    let games: Vec<String> = source["games"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["pgn"].as_str().unwrap().to_owned())
        .collect();
    let options = AnalysisOptions {
        depth: 16,
        multipv: 2,
        threat_depth: 10,
        time_cap: 5.0,
    };
    assert_eq!(source["options"]["depth"], 16);
    assert_eq!(source["options"]["multipv"], 2);
    assert_eq!(source["options"]["threat_depth"], 10);
    assert_eq!(source["threads"], 1);
    assert_eq!(source["hash_mb"], 64);
    assert_eq!(source["time_cap"], 5.0);
    let cancel = AtomicBool::new(false);
    let startup = Instant::now();
    let mut engine = Engine::with_path(locate_stockfish().unwrap(), 1, 64).unwrap();
    let startup_ms = startup.elapsed().as_secs_f64() * 1000.;
    let name = engine.name.clone();
    assert_eq!(source["engine"], name);
    let mut recordings: Vec<Vec<(Search, Vec<Line>)>> = Vec::new();
    let mut outputs = Vec::new();
    let mut cold = Vec::new();
    let validate = |result: &Value, game: &str| {
        let parsed = pgn::parse(game).unwrap();
        let moves = result["moves"].as_array().unwrap();
        assert_eq!(moves.len(), parsed.moves.len());
        assert_eq!(result["evals"].as_array().unwrap().len(), moves.len() + 1);
        for m in moves {
            let white = m["color"] == "w";
            let best = m["eval_before"].as_i64().unwrap() as i32;
            let played = m["eval_after"].as_i64().unwrap() as i32;
            let loss = (analysis::expected_points(best, white)
                - analysis::expected_points(played, white))
            .max(0.0);
            assert!((m["ep_loss"].as_f64().unwrap() - loss).abs() <= 0.000051);
            assert_eq!(
                m["tags"]["evaluation"]["base_classification"],
                analysis::base_classification(m["best_uci"] == m["uci"], loss)
            );
            assert!(!m["explanation"].as_str().unwrap().is_empty());
            assert!(
                m["tags"]["evaluation"]["comparison_depth"]
                    .as_u64()
                    .unwrap()
                    <= 16
            );
        }
    };
    let normalized = |mut v: Value| {
        v.as_object_mut().unwrap().remove("duration_ms");
        v
    };
    for game in &games {
        let mut calls = Vec::new();
        let mut search_ms = 0.;
        let before = engine.metrics();
        let started = Instant::now();
        let result = analysis::analyze_with(
            game,
            &options,
            &cancel,
            |_, _| {},
            &name,
            |request| {
                let start = Instant::now();
                let lines = engine.search(request, &cancel)?;
                search_ms += start.elapsed().as_secs_f64() * 1000.;
                calls.push((request.clone(), lines.clone()));
                Ok(lines)
            },
        )
        .unwrap();
        let wall_ms = started.elapsed().as_secs_f64() * 1000.;
        validate(&result, game);
        let stats = engine.metrics();
        let mut depths = calls
            .iter()
            .flat_map(|(_, ls)| ls.iter().map(|l| l.depth))
            .collect::<Vec<_>>();
        depths.sort_unstable();
        depths.dedup();
        cold.push(json!({"plies":pgn::parse(game).unwrap().moves.len(),"wall_ms":wall_ms,"search_wall_ms":search_ms,"non_search_ms":wall_ms-search_ms,
            "evaluation_calls":calls.len(),"uci_searches":stats.searches-before.searches,"paired_root_searches":calls.iter().filter(|(r,_)|!r.root_moves.is_empty()).count(),
            "threat_calls":calls.iter().filter(|(r,_)|r.depth==10).count(),"returned_iteration_nodes":calls.iter().map(|(_,ls)|ls.iter().map(|l|l.nodes).max().unwrap_or(0)).sum::<u64>(),"achieved_depths":depths}));
        recordings.push(calls);
        outputs.push(result);
    }
    let mut replay_ms = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        for (i, game) in games.iter().enumerate() {
            let mut expected = recordings[i].iter();
            let result = analysis::analyze_with(
                game,
                &options,
                &cancel,
                |_, _| {},
                &name,
                |request| {
                    let (stored, lines) = expected.next().expect("no extra search");
                    assert_eq!(stored, request, "search options/history changed");
                    Ok(lines.clone())
                },
            )
            .unwrap();
            assert!(expected.next().is_none(), "no search dropped");
            assert_eq!(normalized(result), normalized(outputs[i].clone()));
        }
        replay_ms.push(start.elapsed().as_secs_f64() * 1000.);
    }
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(plyback::db::Database::open(dir.path().join("cache.db")).unwrap());
    let mut cached = Engine::with_path(locate_stockfish().unwrap(), 1, 64).unwrap();
    cached.attach_database(db.clone());
    let mut seeded = Vec::new();
    let seed_start = Instant::now();
    for game in &games {
        seeded.push(analysis::analyze(&mut cached, game, &options, &cancel, |_, _| {}).unwrap());
    }
    let seed_ms = seed_start.elapsed().as_secs_f64() * 1000.;
    let seed_stats = cached.metrics();
    cached.close();
    let mut warm_ms = Vec::new();
    for _ in 0..5 {
        let start = Instant::now();
        for (i, game) in games.iter().enumerate() {
            let result =
                analysis::analyze(&mut cached, game, &options, &cancel, |_, _| {}).unwrap();
            validate(&result, game);
            assert_eq!(normalized(result), normalized(seeded[i].clone()));
        }
        warm_ms.push(start.elapsed().as_secs_f64() * 1000.);
    }
    let warm_stats = cached.metrics();
    assert_eq!(warm_stats.searches, seed_stats.searches);
    assert_eq!(warm_stats.process_starts, seed_stats.process_starts);
    let median = |values: &[f64]| {
        let mut sorted = values.to_vec();
        sorted.sort_by(f64::total_cmp);
        sorted[sorted.len() / 2]
    };
    let metrics = json!({"engine":name,"options":options,"threads":1,"hash_mb":64,"node_cap":null,"release":!cfg!(debug_assertions),
        "platform":std::env::consts::OS,"startup_ms":startup_ms,"cold_no_database_cache":cold,"replay_ms":replay_ms,"replay_median_ms":median(&replay_ms),
        "warm_cache":{"seed_ms":seed_ms,"repeats":warm_ms,"median_ms":median(&warm_ms),"uci_searches":warm_stats.searches-seed_stats.searches,
            "process_starts":warm_stats.process_starts-seed_stats.process_starts,"hits":warm_stats.cache_hits-seed_stats.cache_hits,"entries":db.scalar("SELECT COUNT(*) FROM position_cache",&[]).unwrap()},
        "validation":{"legal_games":true,"consistent_root_losses":true,"replay_output_equal":true,"warm_output_equal":true},
        "limitations":"One cold run per fixture; CPU load and Stockfish hash state affect timing. Comparison intentionally changes search workload through paired root evaluation; search budgets unchanged. Returned-iteration nodes exclude any discarded partial iteration. Warm cache measures repeated identical review, not new game analysis."});
    if let Some(output) = std::env::var_os("PLYBACK_BENCH_OUTPUT") {
        std::fs::write(output, serde_json::to_vec_pretty(&metrics).unwrap()).unwrap();
    }
    println!("{}", serde_json::to_string_pretty(&metrics).unwrap());
}
