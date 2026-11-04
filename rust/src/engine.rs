//! Persistent UCI transport with complete-depth MultiPV results and bounded teardown.
use crate::{api_error, db::Database, now, pgn};
use anyhow::{Context, Result};
use rusqlite::OptionalExtension;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use shakmaty::{Chess, Position, uci::UciMove};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, RecvTimeoutError},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

pub const MATE_CP: i32 = 100_000;
pub const MATE_THRESHOLD: i32 = 99_000;
const CACHE_ENTRIES: i64 = 20_000;
/// How long a search waits for the shared connection before treating the
/// position cache as unavailable. Short next to a search, long enough that
/// worker contention no longer drops completed results, so warm re-reviews
/// replay them exactly. Cancellation is still checked every 10ms.
const CACHE_LOCK_WAIT: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Line {
    /// White's perspective. Mate distance uses ±(100000 - moves to mate).
    pub cp: i32,
    pub mate: Option<i32>,
    pub pv: Vec<String>,
    pub depth: u32,
    #[serde(default)]
    pub nodes: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Search {
    pub initial_fen: String,
    pub moves: Vec<String>,
    pub chess960: bool,
    pub depth: u32,
    pub multipv: u32,
    pub time_cap: f64,
    /// Optional deterministic benchmark ceiling; normal reviews do not reduce it.
    pub nodes: Option<u64>,
    pub root_moves: Vec<String>,
}

impl Search {
    pub fn position(fen: &str, depth: u32, multipv: u32, time_cap: f64) -> Self {
        Self {
            initial_fen: fen.into(),
            moves: vec![],
            chess960: false,
            depth,
            multipv,
            time_cap,
            nodes: None,
            root_moves: vec![],
        }
    }

    fn validate(&self) -> Result<(Chess, usize)> {
        if !(1..=32).contains(&self.depth)
            || !(1..=5).contains(&self.multipv)
            || !self.time_cap.is_finite()
            || !(0.01..=60.0).contains(&self.time_cap)
            || self.moves.len() > pgn::MAX_PLIES
            || self.root_moves.len() > 218
            || self.nodes == Some(0)
        {
            return Err(api_error(400, "Invalid engine search limits"));
        }
        let mut board = pgn::board(&self.initial_fen, self.chess960)?;
        let mut positions = HashMap::<String, usize>::new();
        positions.insert(pgn::epd(&board), 1);
        for text in &self.moves {
            let mv = parse_move(text, &board)?;
            board.play_unchecked(mv);
            *positions.entry(pgn::epd(&board)).or_default() += 1;
        }
        let mut roots = HashSet::new();
        for text in &self.root_moves {
            let mv = parse_move(text, &board)?;
            if !roots.insert(mv) {
                return Err(api_error(400, "Duplicate engine root move"));
            }
        }
        let repetitions = positions.get(&pgn::epd(&board)).copied().unwrap_or(1);
        Ok((board, repetitions))
    }
}

pub fn parse_move(text: &str, board: &Chess) -> Result<shakmaty::Move> {
    let uci: UciMove = text
        .parse()
        .map_err(|_| api_error(400, "Invalid UCI move"))?;
    uci.to_move(board)
        .map_err(|_| api_error(400, "Illegal UCI move"))
}

pub fn terminal_eval(board: &Chess, repetitions: usize) -> Option<i32> {
    if board.is_checkmate() {
        Some(if board.turn().is_white() {
            -MATE_CP
        } else {
            MATE_CP
        })
    } else if board.is_stalemate()
        || board.is_insufficient_material()
        || board.halfmoves() >= 150
        || repetitions >= 5
    {
        Some(0)
    } else {
        None
    }
}

pub fn locate_stockfish() -> Result<PathBuf> {
    let data = std::env::var_os("PLYBACK_DATA")
        .or_else(|| std::env::var_os("CHESS_INSIGHT_DATA"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../data"));
    let mut candidates = Vec::new();
    if let Some(path) =
        std::env::var_os("PLYBACK_STOCKFISH").or_else(|| std::env::var_os("STOCKFISH_PATH"))
    {
        // An explicit but invalid setting is a configuration error, never silently
        // run a different executable from PATH.
        let path = PathBuf::from(path);
        return path
            .canonicalize()
            .ok()
            .filter(|p| p.is_file())
            .ok_or_else(|| api_error(503, "Configured Stockfish executable does not exist"));
    }
    if let Ok(path) = std::fs::read_to_string(data.join("engine/stockfish.path")) {
        candidates.push(PathBuf::from(path.trim()));
    }
    candidates.extend([
        data.join("engine/stockfish"),
        data.join("engine/stockfish.exe"),
    ]);
    if let Some(paths) = std::env::var_os("PATH") {
        for path in std::env::split_paths(&paths) {
            candidates.push(path.join(if cfg!(windows) {
                "stockfish.exe"
            } else {
                "stockfish"
            }));
        }
    }
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .and_then(|p| p.canonicalize().ok())
        .ok_or_else(|| {
            api_error(
                503,
                "Stockfish not found. Run the setup script or set PLYBACK_STOCKFISH.",
            )
        })
}

struct Process {
    child: Child,
    input: ChildStdin,
    output: Option<Receiver<String>>,
    reader: Option<JoinHandle<()>>,
}

impl Process {
    fn spawn(path: &Path) -> Result<Self> {
        let mut command = Command::new(path);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }
        let mut child = command
            .spawn()
            .map_err(|_| api_error(503, "Cannot start Stockfish executable"))?;
        let input = child.stdin.take().context("Missing engine stdin")?;
        let stdout = child.stdout.take().context("Missing engine stdout")?;
        let (tx, rx) = mpsc::sync_channel(128);
        let reader = thread::Builder::new()
            .name("stockfish-output".into())
            .spawn(move || {
                let mut out = BufReader::new(stdout);
                loop {
                    let mut line = Vec::new();
                    // UCI PVs are much shorter; stop malicious/broken engine output
                    // before it can allocate unbounded memory.
                    let read = out.by_ref().take(65_537).read_until(b'\n', &mut line);
                    match read {
                        Ok(0) | Err(_) => break,
                        Ok(_) if line.len() > 65_536 => break,
                        Ok(_) => {
                            if tx
                                .send(String::from_utf8_lossy(&line).trim().to_owned())
                                .is_err()
                            {
                                break;
                            }
                        }
                    }
                }
            })?;
        Ok(Self {
            child,
            input,
            output: Some(rx),
            reader: Some(reader),
        })
    }

    fn send(&mut self, command: &str) -> Result<()> {
        writeln!(self.input, "{command}")
            .and_then(|_| self.input.flush())
            .map_err(|_| api_error(503, "Stockfish stopped responding"))
    }

    fn recv(&self, duration: Duration) -> Result<Option<String>> {
        match self
            .output
            .as_ref()
            .context("Engine closed")?
            .recv_timeout(duration)
        {
            Ok(line) => Ok(Some(line)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                Err(api_error(503, "Stockfish exited unexpectedly"))
            }
        }
    }

    fn close(&mut self) {
        let _ = self.send("quit");
        // Drop the receiver first: a stdout reader blocked on the bounded
        // channel must not deadlock teardown.
        self.output.take();
        let deadline = Instant::now() + Duration::from_millis(100);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                _ if Instant::now() >= deadline => {
                    let _ = self.child.kill();
                    break;
                }
                _ => thread::sleep(Duration::from_millis(5)),
            }
        }
        let _ = self.child.wait();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.close();
    }
}

/// One reusable process per worker. `&mut self` serializes each UCI conversation;
/// callers own the bounded worker/interactive pool, never launch per move.
pub struct Engine {
    pub name: String,
    path: PathBuf,
    threads: usize,
    hash_mb: usize,
    process: Option<Process>,
    options: HashSet<String>,
    db: Option<Arc<Database>>,
    cache_writes: usize,
    identity: String,
    metrics: EngineMetrics,
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub struct EngineMetrics {
    pub process_starts: u64,
    pub searches: u64,
    pub cache_hits: u64,
}

impl Engine {
    pub fn new(threads: usize, hash_mb: usize) -> Result<Self> {
        Self::with_path(locate_stockfish()?, threads, hash_mb)
    }

    pub fn with_path(path: PathBuf, threads: usize, hash_mb: usize) -> Result<Self> {
        let cpu = thread::available_parallelism()
            .map(usize::from)
            .unwrap_or(1);
        if !(1..=cpu.max(2)).contains(&threads) || !(16..=4096).contains(&hash_mb) {
            return Err(api_error(400, "Invalid engine resource limits"));
        }
        let metadata = std::fs::metadata(&path)
            .map_err(|_| api_error(503, "Stockfish executable not found"))?;
        let identity = format!(
            "{}:{}:{:?}",
            path.display(),
            metadata.len(),
            metadata.modified().ok()
        );
        let mut engine = Self {
            name: "Stockfish".into(),
            path,
            threads,
            hash_mb,
            process: None,
            options: HashSet::new(),
            db: None,
            cache_writes: 0,
            identity,
            metrics: EngineMetrics::default(),
        };
        engine.start()?;
        Ok(engine)
    }

    pub fn attach_database(&mut self, db: Arc<Database>) {
        self.db = Some(db);
    }

    /// Counters distinguish actual UCI searches from cache hits in benchmarks
    /// and diagnostics without inferring activity from timing alone.
    pub fn metrics(&self) -> EngineMetrics {
        self.metrics
    }

    fn start(&mut self) -> Result<()> {
        self.close();
        let mut process = Process::spawn(&self.path)?;
        process.send("uci")?;
        self.options.clear();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if Instant::now() >= deadline {
                return Err(api_error(503, "Stockfish UCI handshake timed out"));
            }
            if let Some(line) = process.recv(Duration::from_millis(25))? {
                if line == "uciok" {
                    break;
                }
                if let Some(name) = line.strip_prefix("id name ") {
                    self.name = name.to_owned();
                }
                if let Some(option) = line
                    .strip_prefix("option name ")
                    .and_then(|s| s.split(" type ").next())
                {
                    self.options.insert(option.to_owned());
                }
            }
        }
        for (name, value) in [("Threads", self.threads), ("Hash", self.hash_mb)] {
            if self.options.contains(name) {
                process.send(&format!("setoption name {name} value {value}"))?;
            }
        }
        process.send("isready")?;
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if Instant::now() >= deadline {
                return Err(api_error(503, "Stockfish initialization timed out"));
            }
            if process.recv(Duration::from_millis(25))?.as_deref() == Some("readyok") {
                break;
            }
        }
        self.process = Some(process);
        self.metrics.process_starts += 1;
        Ok(())
    }

    pub fn close(&mut self) {
        self.process.take();
    }

    pub fn analyse(
        &mut self,
        fen: &str,
        depth: u32,
        multipv: u32,
        time_cap: f64,
        cancel: &AtomicBool,
    ) -> Result<Vec<Value>> {
        let mut request = Search::position(fen, depth, multipv, time_cap);
        if pgn::board(fen, false).is_err() && pgn::board(fen, true).is_ok() {
            request.chess960 = true;
        }
        self.search(&request, cancel)?
            .into_iter()
            .map(|l| Ok(serde_json::to_value(l)?))
            .collect()
    }

    pub fn search(&mut self, request: &Search, cancel: &AtomicBool) -> Result<Vec<Line>> {
        if cancel.load(Ordering::Relaxed) {
            return Err(api_error(499, "Analysis request was cancelled"));
        }
        let (board, repetitions) = request.validate()?;
        if let Some(cp) = terminal_eval(&board, repetitions) {
            return Ok(vec![Line {
                cp,
                mate: (cp.abs() >= MATE_THRESHOLD).then_some(0),
                pv: vec![],
                depth: 0,
                nodes: 0,
            }]);
        }
        let expected = (request.multipv as usize).min(if request.root_moves.is_empty() {
            board.legal_moves().len()
        } else {
            request.root_moves.len()
        });
        let key = self.cache_key(request)?;
        if let Some(db) = &self.db
            && let Ok(Some(Some(data))) = db.read_until(
                Instant::now() + CACHE_LOCK_WAIT,
                || cancel.load(Ordering::Relaxed),
                |connection| {
                    Ok(connection
                        .query_row(
                            "SELECT data FROM position_cache WHERE key=?",
                            [&key],
                            |row| row.get::<_, String>(0),
                        )
                        .optional()?)
                },
            )
            && let Ok(lines) = serde_json::from_str::<Vec<Line>>(&data)
            && valid_lines(&lines, &board, request, expected)
        {
            if cancel.load(Ordering::Relaxed) {
                return Err(api_error(499, "Analysis request was cancelled"));
            }
            self.metrics.cache_hits += 1;
            return Ok(lines);
        }
        let mut last_error = None;
        for _ in 0..2 {
            if cancel.load(Ordering::Relaxed) {
                return Err(api_error(499, "Analysis request was cancelled"));
            }
            if self.process.is_none() {
                self.start()?;
            }
            match self.search_once(request, &board, expected, cancel) {
                Ok(lines) => {
                    if cancel.load(Ordering::Relaxed) {
                        return Err(api_error(499, "Analysis request was cancelled"));
                    }
                    if let Some(db) = &self.db {
                        let stored = serde_json::to_string(&lines)?;
                        if let Err(error) = db.transaction_until(Instant::now() + CACHE_LOCK_WAIT, || cancel.load(Ordering::Relaxed), |tx| Ok(tx.execute("INSERT INTO position_cache(key,depth,multipv,data,created_at) VALUES (?,?,?,?,?) ON CONFLICT(key) DO UPDATE SET data=excluded.data,created_at=excluded.created_at", rusqlite::params![key,request.depth,expected as i64,stored,now()])?)) {
                            tracing::warn!(%error, "Position cache write failed");
                        }
                        self.cache_writes += 1;
                        if self.cache_writes.is_multiple_of(128) {
                            let _ = db.try_transaction(|tx| Ok(tx.execute("DELETE FROM position_cache WHERE key IN (SELECT key FROM position_cache ORDER BY created_at DESC,key DESC LIMIT -1 OFFSET ?)", [CACHE_ENTRIES])?));
                        }
                    }
                    return Ok(lines);
                }
                Err(e) => {
                    self.close();
                    if cancel.load(Ordering::Relaxed) {
                        return Err(api_error(499, "Analysis request was cancelled"));
                    }
                    last_error = Some(e);
                }
            }
        }
        Err(last_error.unwrap_or_else(|| api_error(503, "Stockfish failed repeatedly")))
    }

    fn cache_key(&self, request: &Search) -> Result<String> {
        // The complete history is deliberate: a FEN alone cannot encode repetition.
        let input = serde_json::to_vec(&json!([
            "rust-uci-v1",
            crate::ANALYSIS_VERSION,
            self.identity,
            self.name,
            self.threads,
            self.hash_mb,
            request
        ]))?;
        Ok(format!("rust:{}", hex::encode(Sha256::digest(input))))
    }

    fn search_once(
        &mut self,
        request: &Search,
        board: &Chess,
        expected: usize,
        cancel: &AtomicBool,
    ) -> Result<Vec<Line>> {
        if expected > 1 && !self.options.contains("MultiPV") {
            return Err(api_error(503, "Engine does not support MultiPV"));
        }
        let process = self.process.as_mut().context("Engine not initialized")?;
        if self.options.contains("MultiPV") {
            process.send(&format!("setoption name MultiPV value {expected}"))?;
        }
        if self.options.contains("UCI_Chess960") {
            process.send(&format!(
                "setoption name UCI_Chess960 value {}",
                request.chess960
            ))?;
        } else if request.chess960 {
            return Err(api_error(503, "Engine does not support Chess960"));
        }
        let mut position = format!("position fen {}", request.initial_fen);
        if !request.moves.is_empty() {
            position.push_str(" moves ");
            position.push_str(&request.moves.join(" "));
        }
        process.send(&position)?;
        let mut go = format!(
            "go depth {} movetime {}",
            request.depth,
            (request.time_cap * 1000.0).ceil() as u64
        );
        if let Some(nodes) = request.nodes {
            go.push_str(&format!(" nodes {nodes}"));
        }
        if !request.root_moves.is_empty() {
            go.push_str(" searchmoves ");
            go.push_str(&request.root_moves.join(" "));
        }
        process.send(&go)?;
        self.metrics.searches += 1;
        let mut deadline = Instant::now() + Duration::from_secs_f64(request.time_cap + 2.0);
        let mut stopped = false;
        let mut iterations: BTreeMap<u32, BTreeMap<usize, Line>> = BTreeMap::new();
        loop {
            if cancel.load(Ordering::Relaxed) && !stopped {
                let _ = process.send("stop");
                stopped = true;
                deadline = Instant::now() + Duration::from_millis(500);
            }
            if Instant::now() >= deadline {
                return Err(api_error(
                    if stopped { 499 } else { 503 },
                    if stopped {
                        "Analysis request was cancelled"
                    } else {
                        "Stockfish search timed out"
                    },
                ));
            }
            let Some(line) = process.recv(Duration::from_millis(20))? else {
                continue;
            };
            if line.starts_with("bestmove ") {
                if stopped || cancel.load(Ordering::Relaxed) {
                    return Err(api_error(499, "Analysis request was cancelled"));
                }
                break;
            }
            if let Some((rank, parsed)) = parse_info(&line, board.turn().is_white())
                && parsed.depth <= request.depth
                && rank <= expected
            {
                iterations
                    .entry(parsed.depth)
                    .or_default()
                    .insert(rank, parsed);
            }
        }
        for (_, iteration) in iterations.into_iter().rev() {
            let lines: Vec<Line> = iteration.into_values().collect();
            if valid_lines(&lines, board, request, expected) {
                return Ok(lines);
            }
        }
        Err(api_error(
            503,
            "Stockfish returned no complete exact MultiPV iteration",
        ))
    }
}
impl Drop for Engine {
    fn drop(&mut self) {
        self.close();
    }
}

fn valid_lines(lines: &[Line], board: &Chess, request: &Search, expected: usize) -> bool {
    if lines.len() != expected || lines.is_empty() {
        return false;
    }
    let mut roots = HashSet::new();
    for line in lines {
        if line.depth == 0
            || line.depth > request.depth
            || line.depth != lines[0].depth
            || line.pv.is_empty()
            || line.pv.len() > 128
            || line.cp.unsigned_abs() > MATE_CP as u32
        {
            return false;
        }
        if !roots.insert(&line.pv[0])
            || (!request.root_moves.is_empty() && !request.root_moves.contains(&line.pv[0]))
        {
            return false;
        }
        let mut pos = board.clone();
        for text in &line.pv {
            let Ok(mv) = parse_move(text, &pos) else {
                return false;
            };
            pos.play_unchecked(mv);
        }
    }
    true
}

fn parse_info(text: &str, white: bool) -> Option<(usize, Line)> {
    if !text.starts_with("info ") {
        return None;
    }
    let tokens: Vec<&str> = text.split_whitespace().collect();
    if tokens.contains(&"lowerbound") || tokens.contains(&"upperbound") {
        return None;
    }
    let field = |name: &str| {
        tokens
            .iter()
            .position(|x| *x == name)
            .and_then(|i| tokens.get(i + 1).copied())
    };
    let depth = field("depth")?.parse::<u32>().ok()?;
    let rank = field("multipv").unwrap_or("1").parse().ok()?;
    let score = tokens.iter().position(|x| *x == "score")?;
    let amount = tokens.get(score + 2)?.parse::<i32>().ok()?;
    let sign = if white { 1 } else { -1 };
    let (cp, mate) = match *tokens.get(score + 1)? {
        "cp" => (amount.clamp(-98_000, 98_000) * sign, None),
        "mate" if amount.unsigned_abs() < 1000 => {
            let encoded = if amount > 0 {
                MATE_CP - amount
            } else {
                -MATE_CP - amount
            };
            (encoded * sign, Some(amount * sign))
        }
        _ => return None,
    };
    let pv_index = tokens.iter().position(|x| *x == "pv")?;
    let pv: Vec<String> = tokens[pv_index + 1..]
        .iter()
        .take(128)
        .map(|s| (*s).into())
        .collect();
    if pv.is_empty() {
        return None;
    }
    Some((
        rank,
        Line {
            cp,
            mate,
            pv,
            depth,
            nodes: field("nodes").and_then(|s| s.parse().ok()).unwrap_or(0),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mate_sign_and_exact_score_parsing() {
        let white = parse_info(
            "info depth 12 multipv 1 score mate 3 nodes 20 pv e2e4",
            true,
        )
        .unwrap()
        .1;
        assert_eq!((white.cp, white.mate), (99_997, Some(3)));
        let black = parse_info("info depth 12 score mate 3 pv e7e5", false)
            .unwrap()
            .1;
        assert_eq!((black.cp, black.mate), (-99_997, Some(-3)));
        assert!(parse_info("info depth 12 score cp 20 lowerbound pv e2e4", true).is_none());
    }
    #[test]
    fn repetition_and_automatic_draws_use_complete_history() {
        let mut request = Search::position(pgn::STARTING_FEN, 10, 2, 1.0);
        for _ in 0..4 {
            request
                .moves
                .extend(["g1f3", "g8f6", "f3g1", "f6g8"].map(String::from));
        }
        let (position, repetitions) = request.validate().unwrap();
        assert_eq!(repetitions, 5);
        assert_eq!(terminal_eval(&position, repetitions), Some(0));
        request.moves.truncate(8);
        let (position, repetitions) = request.validate().unwrap();
        assert_eq!(terminal_eval(&position, repetitions), None); // claimable is not automatic
        let mated = pgn::board("7k/6Q1/6K1/8/8/8/8/8 b - - 150 80", false).unwrap();
        assert_eq!(terminal_eval(&mated, 1), Some(MATE_CP)); // checkmate wins over 75-move rule
    }
    #[test]
    fn incomplete_or_mixed_depth_multipv_is_rejected() {
        let board = Chess::default();
        let req = Search::position(pgn::STARTING_FEN, 12, 2, 1.0);
        let a = Line {
            cp: 10,
            mate: None,
            depth: 12,
            pv: vec!["e2e4".into()],
            nodes: 10,
        };
        let mut b = Line {
            cp: 9,
            mate: None,
            depth: 11,
            pv: vec!["d2d4".into()],
            nodes: 10,
        };
        assert!(!valid_lines(&[a.clone(), b.clone()], &board, &req, 2));
        b.depth = 12;
        assert!(valid_lines(&[a, b], &board, &req, 2));
    }
}
