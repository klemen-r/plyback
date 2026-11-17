//! Bounded, legal main-line PGN parsing. Variations never affect the played game.
use crate::api_error;
use anyhow::Result;
use pgn_reader::{RawComment, RawTag, Reader, SanPlus, Visitor};
use serde::{Deserialize, Serialize};
use shakmaty::{CastlingMode, Chess, EnPassantMode, Position, fen::Fen, uci::UciMove};
use std::{cell::Cell, collections::BTreeMap, io::Read, ops::ControlFlow, rc::Rc};

pub const MAX_GAME_BYTES: usize = 2_000_000;
pub const MAX_PLIES: usize = 2_000;
pub const STARTING_FEN: &str = "rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBNR w KQkq - 0 1";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PgnMove {
    pub ply: usize,
    pub color: String,
    pub san: String,
    pub uci: String,
    pub fen_before: String,
    pub fen_after: String,
    pub clock: Option<f64>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ParsedGame {
    pub headers: BTreeMap<String, String>,
    pub initial_fen: String,
    pub final_fen: String,
    pub rules: String,
    pub moves: Vec<PgnMove>,
    pub result: String,
    pub chess960: bool,
}

pub fn board(fen: &str, chess960: bool) -> Result<Chess> {
    if fen.len() > 256 || fen.contains(['\n', '\r', '\0']) {
        return Err(api_error(400, "Invalid FEN"));
    }
    let parsed: Fen = fen.parse().map_err(|_| api_error(400, "Invalid FEN"))?;
    parsed
        .into_position(if chess960 {
            CastlingMode::Chess960
        } else {
            CastlingMode::Standard
        })
        .map_err(|_| api_error(400, "Illegal chess position"))
}

pub fn fen(pos: &Chess) -> String {
    Fen::from_position(pos, EnPassantMode::Legal).to_string()
}

pub fn epd(pos: &Chess) -> String {
    fen(pos)
        .split_whitespace()
        .take(4)
        .collect::<Vec<_>>()
        .join(" ")
}

pub fn uci(m: shakmaty::Move, chess960: bool) -> String {
    UciMove::from_move(
        m,
        if chess960 {
            CastlingMode::Chess960
        } else {
            CastlingMode::Standard
        },
    )
    .to_string()
}

struct GameVisitor;
struct Movetext {
    game: ParsedGame,
    pos: Chess,
}

impl Visitor for GameVisitor {
    type Tags = BTreeMap<String, String>;
    type Movetext = Movetext;
    type Output = Result<ParsedGame>;

    fn begin_tags(&mut self) -> ControlFlow<Self::Output, Self::Tags> {
        ControlFlow::Continue(BTreeMap::new())
    }

    fn tag(
        &mut self,
        tags: &mut Self::Tags,
        name: &[u8],
        value: RawTag<'_>,
    ) -> ControlFlow<Self::Output> {
        if tags.len() >= 256 || value.as_bytes().len() > 8192 {
            return ControlFlow::Break(Err(api_error(
                400,
                "PGN headers exceed the supported limit",
            )));
        }
        let key = String::from_utf8_lossy(name).into_owned();
        let val = value.decode_utf8_lossy().into_owned();
        tags.insert(key, val);
        ControlFlow::Continue(())
    }

    fn begin_movetext(&mut self, headers: Self::Tags) -> ControlFlow<Self::Output, Self::Movetext> {
        let variant = headers
            .get("Variant")
            .map(|x| x.to_ascii_lowercase())
            .unwrap_or_default();
        let chess960 = matches!(
            variant.as_str(),
            "chess960" | "fischerandom" | "fischer random"
        );
        if !matches!(
            variant.as_str(),
            "" | "standard" | "chess960" | "fischerandom" | "fischer random" | "from position"
        ) {
            return ControlFlow::Break(Err(api_error(400, "Unsupported chess variant")));
        }
        let pos = match board(
            headers
                .get("FEN")
                .map(String::as_str)
                .unwrap_or(STARTING_FEN),
            chess960,
        ) {
            Ok(b) => b,
            Err(e) => return ControlFlow::Break(Err(e)),
        };
        let initial_fen = fen(&pos);
        ControlFlow::Continue(Movetext {
            game: ParsedGame {
                result: headers.get("Result").cloned().unwrap_or_else(|| "*".into()),
                headers,
                initial_fen: initial_fen.clone(),
                final_fen: initial_fen,
                chess960,
                rules: if chess960 { "chess960" } else { "chess" }.into(),
                moves: Vec::new(),
            },
            pos,
        })
    }

    fn san(&mut self, text: &mut Self::Movetext, san: SanPlus) -> ControlFlow<Self::Output> {
        if text.game.moves.len() >= MAX_PLIES {
            return ControlFlow::Break(Err(api_error(400, "PGN exceeds 2000 plies")));
        }
        let m = match san.san.to_move(&text.pos) {
            Ok(m) => m,
            Err(_) => {
                return ControlFlow::Break(Err(api_error(
                    400,
                    format!(
                        "Illegal PGN move at ply {}: {san}",
                        text.game.moves.len() + 1
                    ),
                )));
            }
        };
        let before = fen(&text.pos);
        let color = if text.pos.turn().is_white() { "w" } else { "b" }.into();
        let uci = uci(m, text.game.chess960);
        let normalized_san = SanPlus::from_move_and_play_unchecked(&mut text.pos, m).to_string();
        let after = fen(&text.pos);
        text.game.moves.push(PgnMove {
            ply: text.game.moves.len() + 1,
            color,
            san: normalized_san,
            uci,
            fen_before: before,
            fen_after: after.clone(),
            clock: None,
        });
        text.game.final_fen = after;
        ControlFlow::Continue(())
    }

    fn comment(
        &mut self,
        text: &mut Self::Movetext,
        comment: RawComment<'_>,
    ) -> ControlFlow<Self::Output> {
        if let Some(last) = text.game.moves.last_mut() {
            let comment = String::from_utf8_lossy(comment.as_bytes());
            if let Some(clock) = comment
                .split("[%clk ")
                .nth(1)
                .and_then(|s| s.split(']').next())
            {
                let parts: Option<Vec<f64>> = clock
                    .split(':')
                    .map(|s| s.trim().parse::<f64>().ok())
                    .collect();
                if let Some(p) =
                    parts.filter(|p| p.len() == 3 && p.iter().all(|v| v.is_finite() && *v >= 0.0))
                {
                    let seconds = p[0] * 3600.0 + p[1] * 60.0 + p[2];
                    if p[1] < 60.0 && p[2] < 60.0 && seconds <= 31_536_000.0 {
                        last.clock = Some(seconds);
                    }
                }
            }
        }
        ControlFlow::Continue(())
    }

    fn outcome(
        &mut self,
        text: &mut Self::Movetext,
        outcome: pgn_reader::Outcome,
    ) -> ControlFlow<Self::Output> {
        text.game.result = outcome.to_string();
        text.game
            .headers
            .insert("Result".into(), text.game.result.clone());
        ControlFlow::Continue(())
    }

    fn end_game(&mut self, text: Self::Movetext) -> Self::Output {
        Ok(text.game)
    }
}

pub fn parse(text: &str) -> Result<ParsedGame> {
    if text.len() > MAX_GAME_BYTES || text.contains('\0') {
        return Err(api_error(400, "PGN is too large or contains a NUL byte"));
    }
    let mut reader = Reader::build(text.as_bytes())
        .set_supported_tag_line_length(16_384)
        .finish();
    let game = reader
        .read_game(&mut GameVisitor)
        .map_err(|_| api_error(400, "Malformed PGN"))?
        .ok_or_else(|| api_error(400, "No game found in PGN"))??;
    if reader
        .has_more()
        .map_err(|_| api_error(400, "Malformed PGN"))?
    {
        return Err(api_error(400, "Expected one PGN game"));
    }
    Ok(game)
}

struct CountingReader<'a> {
    text: &'a [u8],
    read: Rc<Cell<usize>>,
}

struct BoundaryVisitor;
impl Visitor for BoundaryVisitor {
    type Tags = ();
    type Movetext = ();
    type Output = ();
    fn begin_tags(&mut self) -> ControlFlow<(), ()> {
        ControlFlow::Continue(())
    }
    fn begin_movetext(&mut self, _: ()) -> ControlFlow<(), ()> {
        ControlFlow::Continue(())
    }
    fn outcome(&mut self, _: &mut (), _: pgn_reader::Outcome) -> ControlFlow<()> {
        // A result token terminates a game even without a blank separator.
        ControlFlow::Break(())
    }
    fn end_game(&mut self, _: ()) {}
}
impl Read for CountingReader<'_> {
    fn read(&mut self, into: &mut [u8]) -> std::io::Result<usize> {
        let n = self.text.read(into)?;
        self.read.set(self.read.get() + n);
        Ok(n)
    }
}

/// The most games one PGN import may contain.
pub const MAX_IMPORT_GAMES: usize = 10_000;

/// Splits a PGN archive with the PGN parser rather than at blank lines or `[Event` text, which
/// may also occur inside comments, and keeps each game's original text for stable IDs.
/// The second value is true when the text after the last complete game could not be read:
/// those games are kept, and the unreadable rest is reported rather than imported.
pub fn split_games(text: &str) -> Result<(Vec<String>, bool)> {
    if text.len() > 20_000_000 || text.contains('\0') {
        return Err(api_error(400, "PGN import exceeds the supported limit"));
    }
    let mut out = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let bytes_read = Rc::new(Cell::new(0));
        let mut reader = Reader::build(CountingReader {
            text: &text.as_bytes()[start..],
            read: bytes_read.clone(),
        })
        .set_supported_tag_line_length(16_384)
        .finish();
        match reader.read_game(&mut BoundaryVisitor) {
            Ok(Some(())) => {}
            Ok(None) => break,
            Err(_) if out.is_empty() => return Err(api_error(400, "Malformed PGN archive")),
            Err(_) => return Ok((out, true)),
        }
        let end = start + bytes_read.get() - reader.buffer().len();
        if end <= start {
            return Err(api_error(400, "Malformed PGN archive"));
        }
        let game = text
            .get(start..end)
            .ok_or_else(|| api_error(400, "Invalid PGN encoding"))?
            .trim();
        if !game.is_empty() {
            out.push(game.to_owned());
        }
        start = end;
        if out.len() > MAX_IMPORT_GAMES {
            return Err(api_error(400, "Import at most 10000 games at a time"));
        }
    }
    Ok((out, false))
}

#[derive(Debug, Clone, Default)]
pub struct TimeControl {
    pub base: Option<f64>,
    pub increment: f64,
    pub daily: bool,
}

pub fn parse_time_control(raw: &str) -> TimeControl {
    if raw.contains('/') {
        return TimeControl {
            daily: true,
            ..Default::default()
        };
    }
    let mut p = raw.split('+');
    let base = p
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0);
    let increment = p
        .next()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|s| s.is_finite() && *s >= 0.0)
        .unwrap_or(0.0);
    if p.next().is_some() {
        return TimeControl::default();
    }
    TimeControl {
        base,
        increment,
        daily: false,
    }
}

/// Per-player controls are emitted only by the Lichess mapper for berserk games. The shared
/// TimeControl remains unchanged for rating pools and filters, and ordinary PGNs ignore overrides.
pub(crate) fn clock_controls(headers: &BTreeMap<String, String>) -> [TimeControl; 2] {
    let shared = parse_time_control(headers.get("TimeControl").map(String::as_str).unwrap_or(""));
    let mut controls = [shared.clone(), shared];
    if headers.get("PlybackClockSource").map(String::as_str) == Some("lichess") {
        for (side, key) in ["WhiteTimeControl", "BlackTimeControl"].iter().enumerate() {
            if let Some(raw) = headers.get(*key) {
                let control = parse_time_control(raw);
                if control.base.is_some() && !control.daily {
                    controls[side] = control;
                }
            }
        }
    }
    controls
}

struct ClockVisitor;
impl Visitor for ClockVisitor {
    type Tags = BTreeMap<String, String>;
    type Movetext = ();
    type Output = Option<[TimeControl; 2]>;

    fn begin_tags(&mut self) -> ControlFlow<Self::Output, Self::Tags> {
        ControlFlow::Continue(BTreeMap::new())
    }
    fn tag(
        &mut self,
        tags: &mut Self::Tags,
        name: &[u8],
        value: RawTag<'_>,
    ) -> ControlFlow<Self::Output> {
        if value.as_bytes().len() > 8192 {
            return ControlFlow::Break(None);
        }
        if matches!(
            name,
            b"TimeControl" | b"WhiteTimeControl" | b"BlackTimeControl" | b"PlybackClockSource"
        ) {
            tags.insert(
                String::from_utf8_lossy(name).into_owned(),
                value.decode_utf8_lossy().into_owned(),
            );
        }
        ControlFlow::Continue(())
    }
    fn begin_movetext(&mut self, headers: Self::Tags) -> ControlFlow<Self::Output, Self::Movetext> {
        // Stop before parsing any moves: Insights needs only the clock metadata.
        ControlFlow::Break(
            (headers.get("PlybackClockSource").map(String::as_str) == Some("lichess"))
                .then(|| clock_controls(&headers)),
        )
    }
    fn end_game(&mut self, _: Self::Movetext) -> Self::Output {
        None
    }
}

pub(crate) fn lichess_clock_controls(text: &str) -> Option<[TimeControl; 2]> {
    if text.len() > MAX_GAME_BYTES || text.contains('\0') {
        return None;
    }
    Reader::build(text.as_bytes())
        .set_supported_tag_line_length(16_384)
        .finish()
        .read_game(&mut ClockVisitor)
        .ok()
        .flatten()
        .flatten()
}

pub fn time_spent(game: &ParsedGame) -> Vec<Option<f64>> {
    let controls = clock_controls(&game.headers);
    let mut last = [controls[0].base, controls[1].base];
    game.moves
        .iter()
        .map(|m| {
            let side = usize::from(m.color == "b");
            let tc = &controls[side];
            let spent = if tc.daily {
                None
            } else {
                last[side].zip(m.clock).map(|(old, clock)| {
                    ((old - clock + tc.increment).max(0.0) * 100.0).round() / 100.0
                })
            };
            last[side] = m.clock;
            spent
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn legal_mainline_clocks_and_variations() {
        let game =
            parse("[TimeControl \"300+2\"]\n\n1. e4 {[%clk 0:05:01]} (1. d4 d5) e5 2. Nf3 *")
                .unwrap();
        assert_eq!(game.moves.len(), 3);
        assert_eq!(game.moves[0].clock, Some(301.0));
        assert_eq!(time_spent(&game)[0], Some(1.0));
        let ordinary = parse(
            "[TimeControl \"300+2\"]\n[WhiteTimeControl \"150+0\"]\n\n1. e4 {[%clk 0:05:01]} *",
        )
        .unwrap();
        assert_eq!(time_spent(&ordinary), vec![Some(1.0)]);
        assert_eq!(game.moves[2].uci, "g1f3");
        assert!(parse("1. e5 *").is_err());
    }
    #[test]
    fn black_initial_position_clock_and_chess960() {
        let g = parse("[FEN \"6k1/8/8/8/8/8/7p/4K3 b - - 0 40\"]\n[TimeControl \"60+1\"]\n\n40... h1=Q+ {[%clk 0:00:58]} *").unwrap();
        assert_eq!(g.moves[0].color, "b");
        assert_eq!(time_spent(&g), vec![Some(3.0)]);
        let c =
            parse("[Variant \"Chess960\"]\n[FEN \"4k3/8/8/8/8/8/8/RK5R w AH - 0 1\"]\n\n1. O-O *")
                .unwrap();
        assert_eq!(c.moves[0].uci, "b1h1");
        assert!(c.final_fen.starts_with("4k3/8/8/8/8/8/8/R4RK1"));
    }
    #[test]
    fn exact_splitting_does_not_use_comment_text() {
        let raw = "[Event \"one\"]\n\n1. e4 {hello\n\n[Event \"fake\"]} e5 *\n\n[Event \"two\"]\n\n1. d4 d5 *";
        let (games, malformed_tail) = split_games(raw).unwrap();
        assert!(!malformed_tail);
        assert_eq!(games.len(), 2);
        assert_eq!(parse(&games[0]).unwrap().headers["Event"], "one");
        assert_eq!(parse(&games[1]).unwrap().headers["Event"], "two");
        assert_eq!(split_games("1. e4 *\n1. d4 *").unwrap().0.len(), 2);
    }
    #[test]
    fn the_game_limit_counts_complete_games_only() {
        let full = "[Event \"x\"]\n\n1. e4 *\n\n".repeat(MAX_IMPORT_GAMES);
        let (games, malformed_tail) =
            split_games(&format!("{full}[Event \"broken\"]\n\n1. d4 {{unclosed")).unwrap();
        assert_eq!((games.len(), malformed_tail), (MAX_IMPORT_GAMES, true));
        assert!(split_games(&format!("{full}[Event \"one more\"]\n\n1. d4 *")).is_err());
    }
}
