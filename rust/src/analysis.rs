//! Transparent move-review heuristics; these do not reproduce Chess.com's model.
//!
//! Every loss compares two scores at the *same root*, from the mover's view.
//! If the played move is absent from the unrestricted MultiPV, a paired search
//! evaluates the selected best and played moves together under identical limits.
//! Only a complete exact MultiPV iteration is accepted by the UCI transport.
use crate::{
    ANALYSIS_VERSION, api_error,
    engine::{Engine, Line, MATE_CP, MATE_THRESHOLD, Search, parse_move},
    pgn::{self, ParsedGame},
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use shakmaty::{Chess, Color, Move, Position, Role, Square, san::SanPlus};
use std::{
    collections::{HashMap, HashSet},
    sync::{
        OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Instant,
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct AnalysisOptions {
    pub depth: u32,
    pub multipv: u32,
    pub threat_depth: u32,
    pub time_cap: f64,
}
impl Default for AnalysisOptions {
    fn default() -> Self {
        Self {
            depth: 16,
            multipv: 2,
            threat_depth: 10,
            time_cap: 5.0,
        }
    }
}

/// Replace this trait implementation when a calibrated expected-points model is
/// available. The default is a probability-like public logistic heuristic, not
/// a claim that the values are empirically calibrated win/draw/loss frequencies.
pub trait ExpectedPointsModel {
    fn id(&self) -> &'static str;
    fn expected_points(&self, cp_white: i32, white: bool) -> f64;
}
pub struct LogisticModel;
struct ReviewContext<'a> {
    options: &'a AnalysisOptions,
    cancel: &'a AtomicBool,
    model: &'a dyn ExpectedPointsModel,
}
impl ExpectedPointsModel for LogisticModel {
    fn id(&self) -> &'static str {
        "lichess-logistic-cp-v1"
    }
    fn expected_points(&self, cp: i32, white: bool) -> f64 {
        let probability = if cp.abs() >= MATE_THRESHOLD {
            if cp > 0 { 1.0 } else { 0.0 }
        } else {
            1.0 / (1.0 + (-0.003_682_08 * f64::from(cp.clamp(-1000, 1000))).exp())
        };
        if white {
            probability
        } else {
            1.0 - probability
        }
    }
}
pub fn expected_points(cp: i32, white: bool) -> f64 {
    LogisticModel.expected_points(cp, white)
}
pub fn win_pct_white(cp: i32) -> f64 {
    expected_points(cp, true) * 100.0
}
pub fn format_eval(cp: i32) -> String {
    if cp.abs() >= MATE_THRESHOLD {
        let distance = (MATE_CP - cp.abs()).max(0);
        if distance == 0 {
            "#".into()
        } else {
            format!("{}M{distance}", if cp < 0 { "-" } else { "" })
        }
    } else {
        format!("{:+.2}", f64::from(cp) / 100.0)
    }
}
pub fn base_classification(best: bool, loss: f64) -> &'static str {
    if best {
        return "best";
    }
    for (limit, label) in [
        (0.02, "excellent"),
        (0.05, "good"),
        (0.10, "inaccuracy"),
        (0.20, "mistake"),
    ] {
        // Subtraction such as 0.52 - 0.50 may land a few ULPs above an exact
        // boundary. This tolerance is far below the stored 0.0001 resolution.
        if loss <= limit + 1e-12 {
            return label;
        }
    }
    "blunder"
}
fn rounded(number: f64, digits: i32) -> f64 {
    let scale = 10_f64.powi(digits);
    (number * scale).round() / scale
}
fn accuracy(loss: f64) -> f64 {
    if loss <= 0.0 {
        100.0
    } else {
        (103.166_810_071_164_9 * (-0.043_544_153_867_539_51 * loss * 100.0).exp()
            - 3.166_924_740_191_411
            + 1.0)
            .clamp(0.0, 100.0)
    }
}
fn cp_loss(best: i32, played: i32, white: bool) -> i32 {
    let sign = if white { 1 } else { -1 };
    ((best * sign).clamp(-1000, 1000) - (played * sign).clamp(-1000, 1000)).max(0)
}
fn mate_for(cp: i32, white: bool) -> Option<i32> {
    if cp.abs() < MATE_THRESHOLD {
        None
    } else {
        Some((MATE_CP - cp.abs()) * if (cp > 0) == white { 1 } else { -1 })
    }
}

#[derive(Deserialize)]
struct OpeningIndex {
    named: HashMap<String, (String, String, usize)>,
    book: HashSet<String>,
}
static OPENINGS: OnceLock<OpeningIndex> = OnceLock::new();
fn openings() -> &'static OpeningIndex {
    OPENINGS.get_or_init(|| {
        serde_json::from_str(include_str!("../../backend/plyback/data/openings.json"))
            .expect("checked bundled opening index")
    })
}

/// The named opening (ECO code, name) for a position given as EPD (placement,
/// side to move, castling, en passant), from the same CC0 Lichess dataset.
pub fn opening_at(epd: &str) -> Option<(String, String)> {
    openings()
        .named
        .get(epd)
        .map(|(eco, name, _)| (eco.clone(), name.clone()))
}

/// Matches verified positions from the preserved CC0 Lichess opening dataset.
/// Leading book coverage stops at the first unknown position; transpositions
/// can still identify a later named opening but never relabel the intervening play.
pub fn identify_opening(game: &ParsedGame) -> (Option<Value>, usize) {
    if game.chess960 || game.initial_fen != pgn::STARTING_FEN {
        return (None, 0);
    }
    let index = openings();
    let mut best = None;
    let mut book_plies = 0;
    let mut continuous = true;
    for (i, mv) in game.moves.iter().enumerate() {
        let key = mv
            .fen_after
            .split_whitespace()
            .take(4)
            .collect::<Vec<_>>()
            .join(" ");
        if continuous && index.book.contains(&key) {
            book_plies = i + 1;
        } else {
            continuous = false;
        }
        if let Some((eco, name, _)) = index.named.get(&key) {
            let (family, variation) = name.split_once(':').unwrap_or((name.as_str(), ""));
            best = Some(
                json!({"eco":eco,"name":name,"family":family.trim(),"variation":variation.trim()}),
            );
        }
        if i > 40 && !continuous {
            break;
        }
    }
    (best, book_plies)
}

pub fn pv_to_san(board: &Chess, moves: &[String], limit: usize) -> Vec<String> {
    let mut board = board.clone();
    let mut out = Vec::new();
    for text in moves.iter().take(limit) {
        let Ok(mv) = parse_move(text, &board) else {
            break;
        };
        out.push(SanPlus::from_move_and_play_unchecked(&mut board, mv).to_string());
    }
    out
}

pub fn interactive_lines(fen: &str, lines: &[Value]) -> Result<Vec<Value>> {
    let board = pgn::board(fen, false).or_else(|_| pgn::board(fen, true))?;
    lines.iter().map(|value| {
        let line: Line = serde_json::from_value(value.clone())?;
        Ok(json!({"cp":line.cp,"eval":format_eval(line.cp),"win_white":rounded(win_pct_white(line.cp),1),
            "uci":line.pv.iter().take(12).collect::<Vec<_>>(),"san":pv_to_san(&board,&line.pv,12),"depth":line.depth,"mate":line.mate}))
    }).collect()
}

pub fn analyze(
    engine: &mut Engine,
    text: &str,
    options: &AnalysisOptions,
    cancel: &AtomicBool,
    progress: impl Fn(usize, usize),
) -> Result<Value> {
    let name = engine.name.clone();
    analyze_with(
        text,
        options,
        cancel,
        progress,
        &LogisticModel,
        &name,
        |request| {
            let before = engine.metrics();
            let started = Instant::now();
            let result = engine.search(request, cancel);
            let after = engine.metrics();
            tracing::debug!(
                target: "plyback::review_timing",
                event = "search",
                kind = if !request.root_moves.is_empty() { "paired" } else if request.depth == options.threat_depth && request.multipv == 1 { "threat" } else { "main" },
                elapsed_us = started.elapsed().as_micros() as u64,
                searches = after.searches - before.searches,
                cache_hits = after.cache_hits - before.cache_hits,
                shared_search_hits = after.shared_search_hits - before.shared_search_hits,
                depth = result.as_ref().ok().and_then(|lines| lines.first()).map(|line| line.depth),
                nodes = result.as_ref().ok().and_then(|lines| lines.last()).map(|line| line.nodes),
            );
            result
        },
    )
}

/// Engine-agnostic path used by deterministic tactical and score-perspective tests.
pub fn analyze_with(
    text: &str,
    options: &AnalysisOptions,
    cancel: &AtomicBool,
    progress: impl Fn(usize, usize),
    model: &dyn ExpectedPointsModel,
    engine_name: &str,
    mut evaluate: impl FnMut(&Search) -> Result<Vec<Line>>,
) -> Result<Value> {
    if !(1..=32).contains(&options.depth)
        || !(1..=5).contains(&options.multipv)
        || !(1..=32).contains(&options.threat_depth)
        || !options.time_cap.is_finite()
        || !(0.01..=60.0).contains(&options.time_cap)
    {
        return Err(api_error(400, "Invalid analysis options"));
    }
    let started = Instant::now();
    let game = pgn::parse(text)?;
    tracing::debug!(target: "plyback::review_timing", event = "pgn", elapsed_us = started.elapsed().as_micros() as u64);
    if game.moves.is_empty() {
        return Err(api_error(400, "Game has no moves"));
    }
    let (opening, book_plies) = identify_opening(&game);
    let mut boards = vec![pgn::board(&game.initial_fen, game.chess960)?];
    boards.extend(
        game.moves
            .iter()
            .map(|m| pgn::board(&m.fen_after, game.chess960))
            .collect::<Result<Vec<_>>>()?,
    );
    tracing::debug!(target: "plyback::review_timing", event = "setup", elapsed_us = started.elapsed().as_micros() as u64);
    let mut searches = Vec::with_capacity(boards.len());
    let mut evaluations = Vec::with_capacity(boards.len());
    let mut history = Vec::new();
    for i in 0..boards.len() {
        if cancel.load(Ordering::Relaxed) {
            return Err(api_error(499, "Analysis request was cancelled"));
        }
        let search = Search {
            initial_fen: game.initial_fen.clone(),
            moves: history.clone(),
            chess960: game.chess960,
            depth: options.depth,
            multipv: options.multipv,
            time_cap: options.time_cap,
            nodes: None,
            root_moves: vec![],
        };
        let lines = evaluate(&search)?;
        if lines.is_empty() {
            return Err(api_error(503, "Engine returned no analysis"));
        }
        evaluations.push(lines);
        searches.push(search);
        if i < game.moves.len() {
            history.push(game.moves[i].uci.clone());
        }
        progress(i + 1, boards.len() + game.moves.len());
    }
    let spent = pgn::time_spent(&game);
    let tc = pgn::parse_time_control(
        game.headers
            .get("TimeControl")
            .map(String::as_str)
            .unwrap_or(""),
    );
    let mut out: Vec<Value> = Vec::with_capacity(game.moves.len());
    let mut counts = json!({"white":{},"black":{}});
    for (i, played) in game.moves.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            return Err(api_error(499, "Analysis request was cancelled"));
        }
        let before = &boards[i];
        let after = &boards[i + 1];
        let white = before.turn().is_white();
        let mv = parse_move(&played.uci, before)?;
        let full_best = &evaluations[i][0];
        let best_uci = full_best.pv.first().cloned();
        let is_best = best_uci.as_deref() == Some(&played.uci);
        let mut best = full_best.clone();
        let mut played_score = evaluations[i]
            .iter()
            .find(|l| l.pv.first() == Some(&played.uci))
            .cloned();
        let mut source = "unrestricted-multipv";
        if played_score.is_none() {
            if let Some(best_move) = &best_uci {
                let mut pair = searches[i].clone();
                pair.multipv = 2;
                pair.root_moves = vec![best_move.clone(), played.uci.clone()];
                let candidates = evaluate(&pair)?;
                let b = candidates.iter().find(|l| l.pv.first() == Some(best_move));
                let p = candidates
                    .iter()
                    .find(|l| l.pv.first() == Some(&played.uci));
                match (b, p) {
                    (Some(b), Some(p)) if b.depth == p.depth => {
                        best = b.clone();
                        played_score = Some(p.clone());
                    }
                    _ => {
                        return Err(api_error(
                            503,
                            "Engine did not evaluate both candidate moves at the same depth",
                        ));
                    }
                }
                source = "paired-root-multipv";
            } else {
                // Continuation after an automatic draw has no competitive loss.
                played_score = Some(full_best.clone());
                source = "terminal-position";
            }
        }
        let played_line = played_score.expect("score resolved above");
        let best_cp = best.cp;
        let played_cp = played_line.cp;
        let ep_best = model.expected_points(best_cp, white);
        let ep_played = model.expected_points(played_cp, white);
        let loss = (ep_best - ep_played).max(0.0);
        let base = base_classification(is_best, loss);
        let second = evaluations[i].get(1);
        let ep_second = second.map(|l| model.expected_points(l.cp, white));
        let n_legal = before.legal_moves().len();
        let in_book = i < book_plies;
        let abs_ply = 2 * (before.fullmoves().get() - 1) + u32::from(!white) + 1;
        let phase = phase(before, abs_ply, in_book);
        let mut tags = Map::new();
        tags.insert("evaluation".into(), json!({"model":model.id(),"comparison_depth":best.depth,
            "source":source,"best_mate":best.mate,"played_mate":played_line.mate,"base_classification":base}));
        let mut class = base;
        if n_legal == 1 {
            class = "forced";
        } else if in_book {
            class = "book";
        } else if is_best {
            let best_mates = mate_for(best_cp, white).is_some_and(|n| n > 0);
            let second_mates = second.is_some_and(|s| mate_for(s.cp, white).is_some_and(|n| n > 0));
            let undecided = ep_second.is_some_and(|p| p < 0.80);
            if best.depth >= 10
                && second.is_some_and(|s| s.depth == best.depth)
                && ep_played >= 0.50
                && (undecided || (best_mates && !second_mates))
                && let Some((square, role)) = offered(before, mv)
                && (best_mates || !quickly_regained(before, &best.pv, square))
            {
                class = "brilliant";
                tags.insert(
                    "sacrifice".into(),
                    json!({"piece":format!("{} on {square}", role_name(role))}),
                );
            }
            if class != "brilliant"
                && full_best.depth >= 10
                && second.is_some_and(|s| s.depth == full_best.depth)
                && ep_second.is_some_and(|s| {
                    model.expected_points(full_best.cp, white) - s >= 0.15 && s < 0.9
                })
                && ep_played >= 0.35
            {
                let recapture = i > 0
                    && parse_move(&game.moves[i - 1].uci, &boards[i - 1])
                        .is_ok_and(|prev| prev.is_capture() && prev.to() == mv.to());
                let trivial = recapture
                    || after.is_checkmate()
                    || (before.is_check() && n_legal <= 2)
                    || mv.promotion() == Some(Role::Queen)
                    || (mv.is_capture() && see(before, mv.to(), 0) >= 2);
                if !trivial {
                    class = "great";
                }
            }
        } else if matches!(base, "mistake" | "blunder") && i > 0 {
            let previous_loss = out[i - 1]["ep_loss"].as_f64().unwrap_or(0.0);
            let previous_best = model.expected_points(
                out[i - 1]["eval_before"].as_i64().unwrap_or(0) as i32,
                white,
            );
            if previous_loss >= 0.10
                && ep_best - previous_best >= 0.10
                && ep_played >= previous_best - 0.05
            {
                class = "miss";
                tags.insert("missed_punish".into(), json!({}));
            }
        }
        let reply = &evaluations[i + 1][0];
        let best_sans = pv_to_san(before, &best.pv, 12);
        let reply_sans = pv_to_san(after, &reply.pv, 12);
        if !is_best
            && !after.is_checkmate()
            && let Some(n) = mate_for(best_cp, white).filter(|n| *n > 0 && *n <= 3)
            && !mate_for(played_cp, white).is_some_and(|n| n > 0)
        {
            tags.insert("missed_mate".into(), json!({"n":n}));
        }
        if is_error(class) || tags.contains_key("missed_mate") {
            tactical_tags(before, after, &best, reply, best_cp, played_cp, &mut tags);
            if matches!(class, "mistake" | "blunder" | "miss")
                && !before.is_check()
                && !tags.contains_key("allowed_mate")
            {
                threat_test(
                    before,
                    reply,
                    ep_best,
                    &ReviewContext {
                        options,
                        cancel,
                        model,
                    },
                    &mut evaluate,
                    &mut tags,
                )?;
            }
            if before.is_check() {
                tags.insert("in_check".into(), json!({}));
            }
            if matches!(class, "mistake" | "blunder")
                && !tags.contains_key("threat_ignored")
                && !tags.contains_key("missed_punish")
            {
                tags.insert("self_inflicted".into(), json!({}));
            }
            let tactical = [
                "hanging_piece",
                "allows_fork",
                "missed_fork",
                "missed_material",
                "loses_material",
                "allowed_mate",
                "missed_mate",
                "threat_ignored",
            ]
            .iter()
            .any(|k| tags.contains_key(*k));
            tags.insert(
                if tactical { "tactical" } else { "positional" }.into(),
                json!({}),
            );
        }
        if played
            .clock
            .zip(tc.base)
            .is_some_and(|(clock, base)| !tc.daily && clock <= 10.0_f64.max(base * 0.1))
        {
            tags.insert("time_pressure".into(), json!({}));
        }
        let explanation = explain(
            class,
            &best_sans,
            &reply_sans,
            best_cp,
            played_cp,
            &tags,
            opening.as_ref(),
        );
        let alternatives:Vec<Value>=evaluations[i].iter().map(|l|json!({"uci":l.pv.first(),"cp":l.cp,"eval":format_eval(l.cp),"san":pv_to_san(before,&l.pv,12),"depth":l.depth,"mate":l.mate})).collect();
        let color = if white { "white" } else { "black" };
        let count = counts[color][class].as_u64().unwrap_or(0) + 1;
        counts[color][class] = json!(count);
        out.push(json!({"ply":i+1,"move_number":before.fullmoves().get(),"color":played.color,
            "san":played.san,"uci":played.uci,"fen_before":played.fen_before,"fen_after":played.fen_after,
            "eval_before":best_cp,"eval_after":played_cp,"win_before":rounded(ep_best*100.0,2),"win_after":rounded(ep_played*100.0,2),
            "ep_loss":rounded(loss,4),"cp_loss":cp_loss(best_cp,played_cp,white),"accuracy":rounded(accuracy(loss),1),
            "best_uci":best_uci,"best_san":best_sans.first(),"lines":alternatives,
            "reply":{"uci":reply.pv.first(),"san":reply_sans,"cp":reply.cp,"mate":reply.mate},"classification":class,
            "phase":phase,"endgame_type":if phase=="endgame" {Some(endgame_type(&played.fen_before))} else {None},
            "clock":played.clock,"time_spent":spent[i],"is_book":in_book,"tags":tags,"explanation":explanation}));
        progress(boards.len() + i + 1, boards.len() + game.moves.len());
    }
    let evals: Vec<i32> = evaluations.iter().map(|e| e[0].cp).collect();
    let (aw, ab) = game_accuracy(&out, &evals);
    let acpl = |color: &str| {
        let losses: Vec<f64> = out
            .iter()
            .filter(|m| m["color"] == color && m["is_book"] == false)
            .filter_map(|m| m["cp_loss"].as_f64())
            .collect();
        if losses.is_empty() {
            None
        } else {
            Some(rounded(losses.iter().sum::<f64>() / losses.len() as f64, 1))
        }
    };
    let acpl_w = acpl("w");
    let acpl_b = acpl("b");
    let moments = key_moments(&out);
    Ok(
        json!({"depth":options.depth,"multipv":options.multipv,"engine":engine_name,"version":ANALYSIS_VERSION,
        "model":model.id(),"evals":evals,"moves":out,"accuracy_white":aw,"accuracy_black":ab,"accuracy":{"white":aw,"black":ab},
        "acpl_white":acpl_w,"acpl_black":acpl_b,"acpl":{"white":acpl_w,"black":acpl_b},"counts":counts,"opening":opening,
        "book_plies":book_plies,"key_moments":moments,"duration_ms":started.elapsed().as_millis()}),
    )
}

pub fn role_value(role: Role) -> i32 {
    match role {
        Role::Pawn => 1,
        Role::Knight | Role::Bishop => 3,
        Role::Rook => 5,
        Role::Queen => 9,
        Role::King => 0,
    }
}
fn role_name(role: Role) -> &'static str {
    match role {
        Role::Pawn => "pawn",
        Role::Knight => "knight",
        Role::Bishop => "bishop",
        Role::Rook => "rook",
        Role::Queen => "queen",
        Role::King => "king",
    }
}
fn material(board: &Chess, color: Color) -> i32 {
    board
        .board()
        .clone()
        .into_iter()
        .filter(|(_, p)| p.color == color)
        .map(|(_, p)| role_value(p.role))
        .sum()
}
fn balance(board: &Chess, color: Color) -> i32 {
    material(board, color) - material(board, !color)
}
pub fn phase(board: &Chess, ply: u32, in_book: bool) -> &'static str {
    let mut npm = 0;
    let mut queens = false;
    for (_, p) in board.board().clone() {
        if p.role != Role::Pawn {
            npm += role_value(p.role);
        }
        if p.role == Role::Queen {
            queens = true;
        }
    }
    if npm <= 26 || (!queens && npm <= 32) {
        "endgame"
    } else if ply <= 20 || in_book {
        "opening"
    } else {
        "middlegame"
    }
}
pub fn endgame_type(fen: &str) -> &'static str {
    let pieces: HashSet<char> = fen
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_ascii_lowercase()
        .chars()
        .filter(|c| "nbrq".contains(*c))
        .collect();
    if pieces.is_empty() {
        "pawn"
    } else if pieces.len() == 1 && pieces.contains(&'r') {
        "rook"
    } else if pieces.iter().all(|p| "nb".contains(*p)) {
        "minor piece"
    } else if pieces.len() == 1 && pieces.contains(&'q') {
        "queen"
    } else {
        "mixed"
    }
}
fn captured_value(m: Move) -> i32 {
    m.capture().map(role_value).unwrap_or(0) + m.promotion().map(|p| role_value(p) - 1).unwrap_or(0)
}
/// Legal least-attacker static exchange evaluation, including pins, x-rays and
/// en passant. Material is only a conservative tactical clue, never an evaluation.
pub fn see(board: &Chess, square: Square, depth: usize) -> i32 {
    if depth > 12
        || board
            .board()
            .piece_at(square)
            .is_none_or(|p| p.color == board.turn() || p.role == Role::King)
    {
        return 0;
    }
    let mv = board
        .legal_moves()
        .into_iter()
        .filter(|m| m.to() == square && m.is_capture())
        .min_by_key(|m| {
            (
                if m.role() == Role::King {
                    100
                } else {
                    role_value(m.role())
                },
                m.promotion().is_none(),
            )
        });
    let Some(mv) = mv else {
        return 0;
    };
    let mut after = board.clone();
    after.play_unchecked(mv);
    (captured_value(mv) - see(&after, square, depth + 1)).max(0)
}
fn hanging(board: &Chess, color: Color, min: i32) -> Vec<(Square, i32)> {
    if board.turn() == color {
        return vec![];
    }
    let mut result: Vec<_> = board
        .board()
        .clone()
        .into_iter()
        .filter(|(_, p)| p.color == color && p.role != Role::King && role_value(p.role) >= min)
        .filter_map(|(sq, _)| {
            let gain = see(board, sq, 0);
            (gain > 0).then_some((sq, gain))
        })
        .collect();
    result.sort_by_key(|(_, gain)| -*gain);
    result
}
pub fn sacrifice(before: &Chess, mv: Move) -> Option<String> {
    offered(before, mv).map(|(square, role)| format!("{} on {square}", role_name(role)))
}
fn offered(before: &Chess, mv: Move) -> Option<(Square, Role)> {
    let mover = before.turn();
    let captured = captured_value(mv);
    let mut after = before.clone();
    after.play_unchecked(mv);
    if after.is_checkmate() {
        return None;
    }
    let already: HashMap<Square, i32> = before
        .clone()
        .swap_turn()
        .ok()
        .map(|b| hanging(&b, mover, 3).into_iter().collect())
        .unwrap_or_default();
    let (square, gain) = hanging(&after, mover, 3)
        .into_iter()
        .find(|(sq, g)| *sq == mv.to() || already.get(sq).copied().unwrap_or(0) < *g)?;
    let net = gain - captured;
    if net < 2 {
        return None;
    }
    let take = after
        .legal_moves()
        .into_iter()
        .filter(|m| m.to() == square && m.is_capture())
        .min_by_key(|m| {
            if m.role() == Role::King {
                100
            } else {
                role_value(m.role())
            }
        });
    if let Some(take) = take {
        let mut accepted = after.clone();
        accepted.play_unchecked(take);
        if !accepted.is_game_over() {
            let regain = accepted
                .board()
                .clone()
                .into_iter()
                .filter(|(_, p)| p.color != mover && p.role != Role::King)
                .map(|(sq, _)| see(&accepted, sq, 0))
                .max()
                .unwrap_or(0);
            if regain >= net {
                return None;
            }
        }
    }
    Some((square, after.board().piece_at(square)?.role))
}
/// A temporary offer such as the fork trick (Nxe5 Nxe5 d4) is a combination,
/// not a sacrifice. Following the engine's best line from the move itself, the
/// offer counts as regained when the opponent takes the offered piece and the
/// mover is back within 1 point after four more plies, plus any capture sequence
/// still in progress there. Declined offers and longer combinations are kept.
fn quickly_regained(before: &Chess, pv: &[String], square: Square) -> bool {
    const HORIZON: usize = 5;
    let mover = before.turn();
    let start = balance(before, mover);
    let mut board = before.clone();
    let mut accepted = false;
    for (i, text) in pv.iter().enumerate() {
        let Ok(mv) = parse_move(text, &board) else {
            break;
        };
        if i >= HORIZON && (!mv.is_capture() || i >= HORIZON + 2) {
            break;
        }
        accepted |= board.turn() != mover && mv.is_capture() && mv.to() == square;
        board.play_unchecked(mv);
    }
    accepted && balance(&board, mover) >= start - 1
}
fn play_line(board: &Chess, line: &[String], limit: usize) -> (Chess, usize) {
    let mut b = board.clone();
    let mut count = 0;
    for (i, s) in line.iter().enumerate() {
        let Ok(m) = parse_move(s, &b) else {
            break;
        };
        if i >= limit && (!m.is_capture() || i >= limit + 2) {
            break;
        }
        b.play_unchecked(m);
        count += 1;
    }
    (b, count)
}
fn fork_targets(board: &Chess, mv: Move) -> Vec<String> {
    let Some(piece) = board.board().piece_at(mv.to()) else {
        return vec![];
    };
    let value = if piece.role == Role::King {
        100
    } else {
        role_value(piece.role)
    };
    if see(board, mv.to(), 0) >= value {
        return vec![];
    }
    let mut targets = Vec::new();
    for square in board.board().attacks_from(mv.to()) {
        if let Some(target) = board.board().piece_at(square)
            && target.color != piece.color
            && (target.role == Role::King
                || role_value(target.role) > value
                || (role_value(target.role) >= 3
                    && board
                        .board()
                        .attacks_to(square, target.color, board.board().occupied())
                        .is_empty()))
        {
            targets.push(role_name(target.role).into());
        }
    }
    if targets.len() >= 2 { targets } else { vec![] }
}
fn material_words(points: i32) -> String {
    match points.abs() {
        1 => "a pawn".into(),
        2 => "two pawns".into(),
        3 => "a piece".into(),
        4 => "a piece and a pawn".into(),
        5 => "a rook".into(),
        6 => "a rook and a pawn".into(),
        9 | 10 => "the queen".into(),
        n => format!("{n} points of material"),
    }
}
fn tactical_tags(
    before: &Chess,
    after: &Chess,
    best: &Line,
    reply: &Line,
    best_cp: i32,
    played_cp: i32,
    tags: &mut Map<String, Value>,
) {
    let mover = before.turn();
    let white = mover.is_white();
    let start = balance(before, mover);
    let (reply_end, reply_plies) = play_line(after, &reply.pv, 6);
    let played = balance(&reply_end, mover);
    let (best_end, _) = play_line(before, &best.pv, 7);
    let best_balance = balance(&best_end, mover);
    if let Some(n) = mate_for(played_cp, !white).filter(|n| *n > 0)
        && !mate_for(best_cp, !white).is_some_and(|n| n > 0)
    {
        tags.insert("allowed_mate".into(), json!({"n":n}));
        let (mate_board, _) = play_line(after, &reply.pv, (2 * n + 1) as usize);
        if mate_board.is_checkmate()
            && let Some(king) = mate_board.board().king_of(mover)
            && king.rank()
                == if white {
                    shakmaty::Rank::First
                } else {
                    shakmaty::Rank::Eighth
                }
            && mate_board.checkers().into_iter().any(|sq| {
                sq.rank() == king.rank()
                    && mate_board
                        .board()
                        .piece_at(sq)
                        .is_some_and(|p| matches!(p.role, Role::Rook | Role::Queen))
            })
        {
            tags.insert("back_rank".into(), json!({}));
        }
    }
    let lost = start - played;
    if lost >= 2 {
        if let Some(reply_mv) = reply.pv.first().and_then(|s| parse_move(s, after).ok()) {
            if let Some(victim) = after
                .board()
                .piece_at(reply_mv.to())
                .filter(|p| p.color == mover && role_value(p.role) >= 2)
            {
                if reply_mv.is_capture() {
                    tags.insert("hanging_piece".into(),json!({"piece":role_name(victim.role),"square":reply_mv.to().to_string(),"capture":SanPlus::from_move(after.clone(),reply_mv).to_string(),"value":role_value(victim.role)}));
                }
            } else {
                let mut b = after.clone();
                b.play_unchecked(reply_mv);
                let targets = fork_targets(&b, reply_mv);
                if !targets.is_empty() {
                    tags.insert("allows_fork".into(),json!({"move":SanPlus::from_move(after.clone(),reply_mv).to_string(),"targets":targets}));
                }
            }
        }
        tags.insert(
            "loses_material".into(),
            json!({"points":lost,"words":material_words(lost),"plies":reply_plies}),
        );
    }
    if best_balance - played >= 2 && best_balance - start >= 2 {
        tags.insert(
            "missed_material".into(),
            json!({"points":best_balance-start,"words":material_words(best_balance-start)}),
        );
        if let Some(best_move) = best.pv.first().and_then(|s| parse_move(s, before).ok()) {
            let mut b = before.clone();
            b.play_unchecked(best_move);
            let targets = fork_targets(&b, best_move);
            if !targets.is_empty() {
                tags.insert("missed_fork".into(),json!({"move":SanPlus::from_move(before.clone(),best_move).to_string(),"targets":targets}));
            }
        }
    }
}
fn threat_test(
    before: &Chess,
    reply: &Line,
    ep_best: f64,
    context: &ReviewContext<'_>,
    evaluate: &mut impl FnMut(&Search) -> Result<Vec<Line>>,
    tags: &mut Map<String, Value>,
) -> Result<()> {
    let ReviewContext {
        options,
        cancel,
        model,
    } = context;
    let Ok(null) = before.clone().swap_turn() else {
        return Ok(());
    };
    if null.is_check() || null.legal_moves().is_empty() {
        return Ok(());
    }
    let mut search = Search::position(&pgn::fen(&null), options.threat_depth, 1, options.time_cap);
    search.chess960 = before.castles().mode() == shakmaty::CastlingMode::Chess960;
    let lines = match evaluate(&search) {
        Ok(l) => l,
        Err(e) => {
            if cancel.load(Ordering::Relaxed) {
                return Err(e);
            }
            return Ok(());
        }
    };
    if let Some(line) = lines.first()
        && ep_best - model.expected_points(line.cp, before.turn().is_white()) >= 0.15
        && let (Some(threat), Some(refutation)) = (
            line.pv.first().and_then(|s| parse_move(s, &null).ok()),
            reply.pv.first(),
        )
    {
        // Compare legal source and target squares instead of descriptions.
        if let Ok(reply_uci) = refutation.parse::<shakmaty::uci::UciMove>()
            && reply_uci.to() == Some(threat.to())
            && (reply_uci.from() == threat.from() || threat.is_capture())
        {
            tags.insert(
                "threat_ignored".into(),
                json!({"move":SanPlus::from_move(null,threat).to_string()}),
            );
        }
    }
    Ok(())
}
fn is_error(class: &str) -> bool {
    matches!(class, "inaccuracy" | "mistake" | "blunder" | "miss")
}
fn explain(
    class: &str,
    best: &[String],
    reply: &[String],
    best_cp: i32,
    played_cp: i32,
    tags: &Map<String, Value>,
    opening: Option<&Value>,
) -> String {
    let best_san = best.first().map(String::as_str);
    let lead = match class {
        "brilliant" => "Brilliant!",
        "great" => "Great move!",
        "best" => "Best move.",
        "excellent" => "Excellent move.",
        "good" => "Good move.",
        "book" => "Book move.",
        "forced" => "Forced — the only legal move.",
        "inaccuracy" => "Inaccuracy.",
        "mistake" => "Mistake.",
        "miss" => "Missed opportunity.",
        _ => "Blunder.",
    };
    let mut parts = vec![lead.to_string()];
    if class == "forced" {
        return parts.join(" ");
    }
    if class == "book" {
        if let Some(name) = opening.and_then(|v| v["name"].as_str()) {
            parts.push(format!("Verified opening theory ({name})."));
        } else {
            parts.push("The position is in the verified opening database.".into());
        }
        return parts.join(" ");
    }
    if class == "brilliant" {
        parts.push(format!(
            "A sound sacrifice of the {}: it remains the engine's strongest continuation ({}).",
            tags["sacrifice"]["piece"].as_str().unwrap_or("material"),
            format_eval(played_cp)
        ));
        parts.push(format!(
            "Main line: {}.",
            best.iter().take(6).cloned().collect::<Vec<_>>().join(" ")
        ));
        return parts.join(" ");
    }
    if class == "great" {
        parts.push("The only tested move that preserves the balance or advantage; the next best alternative loses at least 15 estimated percentage points.".into());
        return parts.join(" ");
    }
    if let Some(mate) = tags.get("allowed_mate") {
        parts.push(format!(
            "This allows a forced mate in {}: {}.",
            mate["n"],
            reply.iter().take(7).cloned().collect::<Vec<_>>().join(" ")
        ));
    }
    if let Some(mate) = tags.get("missed_mate") {
        parts.push(format!(
            "{} would force mate in {}.",
            best_san.unwrap_or("The engine's choice"),
            mate["n"]
        ));
    }
    if let Some(piece) = tags.get("hanging_piece") {
        parts.push(format!(
            "It leaves your {} on {} vulnerable: {} wins material.",
            piece["piece"].as_str().unwrap_or("piece"),
            piece["square"].as_str().unwrap_or("?"),
            piece["capture"].as_str().unwrap_or("the reply")
        ));
    } else if let Some(fork) = tags.get("allows_fork") {
        parts.push(format!(
            "It allows {}, forking valuable targets.",
            fork["move"].as_str().unwrap_or("a move")
        ));
    } else if let Some(loss) = tags.get("loses_material") {
        parts.push(format!(
            "The engine's continuation loses {}.",
            loss["words"].as_str().unwrap_or("material")
        ));
    }
    if class == "miss" {
        parts.push(
            "Your opponent's previous error offered an opportunity, but this move gives it back."
                .into(),
        );
    }
    if let Some(threat) = tags.get("threat_ignored") {
        parts.push(format!(
            "The existing threat {} is not addressed.",
            threat["move"].as_str().unwrap_or("")
        ));
    }
    if let Some(fork) = tags.get("missed_fork") {
        parts.push(format!(
            "{} would have forked valuable targets.",
            fork["move"].as_str().unwrap_or("The best move")
        ));
    } else if let Some(gain) = tags.get("missed_material") {
        parts.push(format!(
            "The best continuation would win {}.",
            gain["words"].as_str().unwrap_or("material")
        ));
    }
    if is_error(class) && parts.len() == 1 {
        parts.push(format!(
            "Compared from the same position, the evaluation changes from {} to {}.",
            format_eval(best_cp),
            format_eval(played_cp)
        ));
    }
    if let Some(best_san) = best_san
        && class != "best"
        && !parts.iter().any(|p| p.contains(best_san))
    {
        parts.push(format!(
            "{} was stronger ({}), for example {}.",
            best_san,
            format_eval(best_cp),
            best.iter().take(4).cloned().collect::<Vec<_>>().join(" ")
        ));
    }
    parts.join(" ")
}
fn game_accuracy(moves: &[Value], evals: &[i32]) -> (Option<f64>, Option<f64>) {
    let wins: Vec<f64> = evals.iter().map(|cp| win_pct_white(*cp)).collect();
    let window = (moves.len() / 10).clamp(2, 8).min(wins.len());
    let mut weights = Vec::new();
    let volatility = |values: &[f64]| {
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / values.len() as f64)
            .sqrt()
            .clamp(0.5, 12.0)
    };
    for _ in 0..window.saturating_sub(2) {
        weights.push(volatility(&wins[..window]));
    }
    for sample in wins.windows(window) {
        weights.push(volatility(sample));
    }
    let color_accuracy = |color: &str| {
        let values: Vec<(f64, f64)> = moves
            .iter()
            .enumerate()
            .filter(|(_, m)| m["color"] == color)
            .map(|(i, m)| {
                (
                    m["accuracy"].as_f64().unwrap_or(100.0),
                    weights
                        .get(i)
                        .copied()
                        .unwrap_or_else(|| *weights.last().unwrap_or(&0.5)),
                )
            })
            .collect();
        if values.is_empty() {
            None
        } else {
            let weighted = values.iter().map(|(a, w)| a * w).sum::<f64>()
                / values.iter().map(|(_, w)| w).sum::<f64>();
            let harmonic =
                values.len() as f64 / values.iter().map(|(a, _)| 1.0 / a.max(0.001)).sum::<f64>();
            Some(rounded((weighted + harmonic) / 2.0, 1))
        }
    };
    (color_accuracy("w"), color_accuracy("b"))
}
fn key_moments(moves: &[Value]) -> Vec<usize> {
    let mut scores: Vec<(f64, usize)> = moves
        .iter()
        .filter_map(|m| {
            let c = m["classification"].as_str()?;
            let score = if c == "brilliant" {
                1.5
            } else if c == "great" {
                0.5
            } else if is_error(c) {
                m["ep_loss"].as_f64()?
            } else {
                return None;
            };
            Some((score, m["ply"].as_u64()? as usize))
        })
        .collect();
    scores.sort_by(|a, b| b.0.total_cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    let mut plies: Vec<_> = scores.into_iter().take(6).map(|(_, ply)| ply).collect();
    plies.sort_unstable();
    plies
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn inclusive_boundaries_and_mover_perspective() {
        assert_eq!(base_classification(false, 0.02), "excellent");
        assert_eq!(base_classification(false, 0.52 - 0.50), "excellent");
        assert_eq!(base_classification(false, 0.05), "good");
        assert_eq!(base_classification(false, 0.10), "inaccuracy");
        assert_eq!(base_classification(false, 0.20), "mistake");
        assert_eq!(base_classification(false, 0.200001), "blunder");
        assert_eq!(base_classification(true, 0.9), "best");
        assert_eq!(expected_points(99_997, true), 1.0);
        assert_eq!(expected_points(99_997, false), 0.0);
        assert!((expected_points(300, true) - expected_points(-300, false)).abs() < 1e-12);
    }
    #[test]
    fn verified_book_and_arbitrary_fen_are_distinct() {
        let game = pgn::parse("1. e4 e5 2. Nf3 Nc6 3. Bb5 *").unwrap();
        let (opening, plies) = identify_opening(&game);
        assert_eq!(plies, 5);
        assert!(
            opening.unwrap()["name"]
                .as_str()
                .unwrap()
                .contains("Ruy Lopez")
        );
        let game = pgn::parse(
            "[FEN \"rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR b KQkq - 0 1\"]\n\n1... e5 *",
        )
        .unwrap();
        assert_eq!(identify_opening(&game), (None, 0));
    }
    #[test]
    fn legal_sacrifice_is_meaningful_and_static_exchange_respects_pins() {
        let game = pgn::parse("1. e4 e5 2. Nf3 d6 3. Bc4 Bg4 4. Nc3 g6 5. Nxe5 *").unwrap();
        let before = pgn::board(&game.moves[8].fen_before, false).unwrap();
        assert!(
            sacrifice(&before, parse_move("f3e5", &before).unwrap())
                .unwrap()
                .contains("queen")
        );
        let game = pgn::parse("1. e4 e5 2. Nf3 Nc6 3. d4 Qh4 4. Nxh4 *").unwrap();
        let before = pgn::board(&game.moves[6].fen_before, false).unwrap();
        assert!(sacrifice(&before, parse_move("f3h4", &before).unwrap()).is_none());
    }
    #[test]
    fn played_move_loss_uses_paired_root_not_next_ply_evaluation() {
        let text = "[FEN \"4k3/7p/8/8/8/8/P7/4K3 w - - 0 20\"]\n\n20. a3 *";
        let mut paired = false;
        let result = analyze_with(
            text,
            &AnalysisOptions {
                depth: 12,
                ..Default::default()
            },
            &AtomicBool::new(false),
            |_, _| {},
            &LogisticModel,
            "Fake",
            |request| {
                if !request.root_moves.is_empty() {
                    paired = true;
                    assert!(request.moves.is_empty());
                    return Ok(vec![
                        Line {
                            cp: 300,
                            mate: None,
                            pv: vec!["a2a4".into()],
                            depth: 12,
                            nodes: 100,
                        },
                        Line {
                            cp: -300,
                            mate: None,
                            pv: vec!["a2a3".into()],
                            depth: 12,
                            nodes: 100,
                        },
                    ]);
                }
                if request.moves.is_empty() {
                    Ok(vec![
                        Line {
                            cp: 300,
                            mate: None,
                            pv: vec!["a2a4".into()],
                            depth: 12,
                            nodes: 100,
                        },
                        Line {
                            cp: 200,
                            mate: None,
                            pv: vec!["e1d2".into()],
                            depth: 12,
                            nodes: 100,
                        },
                    ])
                } else {
                    Ok(vec![Line {
                        cp: 99999,
                        mate: Some(1),
                        pv: vec!["h7h6".into()],
                        depth: 12,
                        nodes: 100,
                    }])
                }
            },
        )
        .unwrap();
        assert!(paired);
        assert_eq!(result["moves"][0]["eval_after"], -300);
        assert_eq!(result["moves"][0]["classification"], "blunder");
    }

    fn fake_line(cp: i32, pv: &[&str]) -> Line {
        Line {
            cp,
            mate: None,
            pv: pv.iter().map(|s| (*s).into()).collect(),
            depth: 12,
            nodes: 100,
        }
    }

    #[test]
    fn brilliant_requires_completed_alternative_evidence_even_for_mate() {
        let setup = pgn::parse("1. e4 e5 2. Nf3 d6 3. Bc4 Bg4 4. Nc3 g6 5. Nxe5 *").unwrap();
        let text = format!("[FEN \"{}\"]\n\n5. Nxe5 *", setup.moves[8].fen_before);
        // The position offers the queen; incomplete evidence should preserve Best.
        for (best_cp, depth, alternative, expected) in [
            (0, 12, None, "best"),
            (0, 12, Some((0, 11)), "best"),
            (0, 8, Some((0, 8)), "best"),
            (0, 12, Some((0, 12)), "brilliant"),
            (99_997, 12, None, "best"),
            (99_997, 12, Some((500, 12)), "brilliant"),
            (99_997, 12, Some((99_999, 12)), "best"),
        ] {
            let result = analyze_with(
                &text,
                &AnalysisOptions::default(),
                &AtomicBool::new(false),
                |_, _| {},
                &LogisticModel,
                "Fake",
                |request| {
                    if !request.moves.is_empty() {
                        return Ok(vec![fake_line(0, &["g4d1"])]);
                    }
                    let mut best = fake_line(best_cp, &["f3e5"]);
                    best.depth = depth;
                    let mut lines = vec![best];
                    if let Some((cp, second_depth)) = alternative {
                        let mut second = fake_line(cp, &["h2h3"]);
                        second.depth = second_depth;
                        lines.push(second);
                    }
                    Ok(lines)
                },
            )
            .unwrap();
            assert_eq!(result["moves"][0]["classification"], expected);
            assert_eq!(
                result["moves"][0]["tags"]["evaluation"]["base_classification"],
                "best"
            );
        }
    }

    #[test]
    fn brilliant_rejects_material_the_best_line_wins_straight_back() {
        let setup = pgn::parse("1. e4 e5 2. Nf3 Nc6 3. Nc3 Bc5 4. Nxe5 *").unwrap();
        let text = format!("[FEN \"{}\"]\n\n4. Nxe5 *", setup.moves[6].fen_before);
        // The fork trick regains the knight with d4; a line that stops after the
        // knight is taken leaves a genuine offer and keeps the overlay.
        for (pv, expected) in [
            (
                &["f3e5", "c6e5", "d2d4", "c5d6", "d4e5", "d6e5"][..],
                "best",
            ),
            (&["f3e5", "c6e5"][..], "brilliant"),
        ] {
            let result = analyze_with(
                &text,
                &AnalysisOptions::default(),
                &AtomicBool::new(false),
                |_, _| {},
                &LogisticModel,
                "Fake",
                |request| {
                    if !request.moves.is_empty() {
                        return Ok(vec![fake_line(0, &["c6e5"])]);
                    }
                    Ok(vec![fake_line(76, pv), fake_line(17, &["h2h3"])])
                },
            )
            .unwrap();
            assert_eq!(result["moves"][0]["classification"], expected);
        }
        // Légal: declining the queen and taking the knight is not regaining the offer.
        let setup = pgn::parse("1. e4 e5 2. Nf3 d6 3. Bc4 Bg4 4. Nc3 g6 5. Nxe5 *").unwrap();
        let text = format!("[FEN \"{}\"]\n\n5. Nxe5 *", setup.moves[8].fen_before);
        let result = analyze_with(
            &text,
            &AnalysisOptions::default(),
            &AtomicBool::new(false),
            |_, _| {},
            &LogisticModel,
            "Fake",
            |request| {
                if !request.moves.is_empty() {
                    return Ok(vec![fake_line(0, &["d6e5"])]);
                }
                Ok(vec![
                    fake_line(150, &["f3e5", "d6e5", "d1g4"]),
                    fake_line(0, &["h2h3"]),
                ])
            },
        )
        .unwrap();
        assert_eq!(result["moves"][0]["classification"], "brilliant");
    }

    #[test]
    fn great_requires_a_nontrivial_unique_choice_at_completed_depth() {
        let text = "[FEN \"4k3/7p/8/8/8/8/P7/4K3 w - - 0 20\"]\n\n20. Kd2 *";
        let result = analyze_with(
            text,
            &AnalysisOptions::default(),
            &AtomicBool::new(false),
            |_, _| {},
            &LogisticModel,
            "Fake",
            |request| {
                Ok(if request.moves.is_empty() {
                    vec![fake_line(0, &["e1d2"]), fake_line(-500, &["a2a3"])]
                } else {
                    vec![fake_line(0, &["h7h6"])]
                })
            },
        )
        .unwrap();
        assert_eq!(result["moves"][0]["classification"], "great");
        let uncertain = analyze_with(
            text,
            &AnalysisOptions::default(),
            &AtomicBool::new(false),
            |_, _| {},
            &LogisticModel,
            "Fake",
            |request| {
                let mut lines = if request.moves.is_empty() {
                    vec![fake_line(0, &["e1d2"]), fake_line(-500, &["a2a3"])]
                } else {
                    vec![fake_line(0, &["h7h6"])]
                };
                for line in &mut lines {
                    line.depth = 8;
                }
                Ok(lines)
            },
        )
        .unwrap();
        assert_eq!(uncertain["moves"][0]["classification"], "best");
    }

    #[test]
    fn missed_opportunity_uses_previous_error_and_black_mover_perspective() {
        let text = "[FEN \"4k3/7p/8/8/8/8/P7/4K3 w - - 0 20\"]\n\n20. a3 h6 *";
        let initial = pgn::parse(text).unwrap().initial_fen;
        let result = analyze_with(
            text,
            &AnalysisOptions::default(),
            &AtomicBool::new(false),
            |_, _| {},
            &LogisticModel,
            "Fake",
            |request| {
                Ok(if request.initial_fen != initial {
                    vec![fake_line(0, &[])]
                } else {
                    match request.moves.len() {
                        0 => vec![fake_line(300, &["a2a4"]), fake_line(-300, &["a2a3"])],
                        1 => vec![fake_line(-300, &["h7h5"]), fake_line(0, &["h7h6"])],
                        _ => vec![fake_line(0, &["a3a4"])],
                    }
                })
            },
        )
        .unwrap();
        assert_eq!(result["moves"][0]["classification"], "blunder");
        assert_eq!(result["moves"][1]["classification"], "miss");
        assert!(result["moves"][1]["ep_loss"].as_f64().unwrap() > 0.2);
        assert!(result["moves"][1]["tags"].get("missed_punish").is_some());
    }

    #[test]
    fn paired_root_comparison_handles_black_and_rejects_mixed_depths() {
        let text = "[FEN \"4k3/7p/8/8/8/8/P7/4K3 b - - 0 20\"]\n\n20... h6 *";
        let evaluator = |mixed: bool| {
            move |request: &Search| -> Result<Vec<Line>> {
                if !request.root_moves.is_empty() {
                    let best = fake_line(-300, &["h7h5"]);
                    let mut played = fake_line(300, &["h7h6"]);
                    if mixed {
                        played.depth = 11;
                    }
                    return Ok(vec![best, played]);
                }
                if request.moves.is_empty() {
                    Ok(vec![fake_line(-300, &["h7h5"]), fake_line(-250, &["e8d7"])])
                } else {
                    Ok(vec![fake_line(-99999, &["a2a3"])])
                }
            }
        };
        let result = analyze_with(
            text,
            &AnalysisOptions::default(),
            &AtomicBool::new(false),
            |_, _| {},
            &LogisticModel,
            "Fake",
            evaluator(false),
        )
        .unwrap();
        assert_eq!(result["moves"][0]["classification"], "blunder");
        assert_eq!(result["moves"][0]["eval_after"], 300);
        assert!(
            analyze_with(
                text,
                &AnalysisOptions::default(),
                &AtomicBool::new(false),
                |_, _| {},
                &LogisticModel,
                "Fake",
                evaluator(true)
            )
            .is_err()
        );
    }
}
