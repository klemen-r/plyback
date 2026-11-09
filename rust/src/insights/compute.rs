use crate::{api_error, db::Database};
use anyhow::Result;
use chrono::{DateTime, Datelike, Timelike, Utc};
use serde::{
    Deserialize,
    de::{MapAccess, SeqAccess, Visitor},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::OnceLock;

const ERRORS: &[&str] = &["mistake", "blunder", "miss"];
const CLASSES: &[&str] = &[
    "brilliant",
    "great",
    "best",
    "excellent",
    "good",
    "book",
    "forced",
    "inaccuracy",
    "mistake",
    "miss",
    "blunder",
];
const PHASES: &[&str] = &["opening", "middlegame", "endgame"];
const TAG_LABELS: &[(&str, &str)] = &[
    ("hanging_piece", "Left a piece hanging"),
    ("threat_ignored", "Ignored an opponent's threat"),
    ("allowed_mate", "Allowed a forced mate"),
    ("back_rank", "Back-rank weakness"),
    ("allows_fork", "Allowed a fork"),
    ("loses_material", "Lost material to a tactic"),
    ("missed_punish", "Didn't punish the opponent's error"),
    ("missed_material", "Missed winning material"),
    ("missed_fork", "Missed a fork"),
    ("missed_mate", "Missed a forced mate"),
    ("self_inflicted", "Self-inflicted (no threat existed)"),
    ("positional", "Positional / strategic error"),
    ("tactical", "Tactical error"),
    ("in_check", "Wrong reply to a check"),
    ("time_pressure", "Error in time trouble"),
];

fn text(v: &Value) -> &str {
    v.as_str().unwrap_or("")
}
fn number(v: &Value) -> f64 {
    v.as_f64().unwrap_or(0.0)
}
fn integer(v: &Value) -> i64 {
    v.as_i64().unwrap_or(0)
}
fn round(x: f64, n: i32) -> f64 {
    // Multiplication before rounding creates false halfway values (69.475 * 100,
    // for example). Fixed-precision formatting rounds the original binary float,
    // matching the baseline's decimal rounding at classification/statistic edges.
    if n == 0 {
        x.round_ties_even()
    } else {
        format!("{x:.precision$}", precision = n as usize)
            .parse()
            .unwrap_or(x)
    }
}
fn pct(a: f64, b: usize) -> Value {
    if b == 0 {
        Value::Null
    } else {
        json!(round(100.0 * a / b as f64, 1))
    }
}
fn avg(xs: impl Iterator<Item = Option<f64>>) -> Value {
    let mut n = 0usize;
    let sum = accurate_sum(xs.flatten().inspect(|_| n += 1));
    if n == 0 {
        Value::Null
    } else {
        json!(round(sum / n as f64, 2))
    }
}
fn accurate_sum(xs: impl Iterator<Item = f64>) -> f64 {
    let (mut hi, mut lo) = (0.0_f64, 0.0_f64);
    for x in xs {
        let t = hi + x;
        lo += if hi.abs() >= x.abs() {
            (hi - t) + x
        } else {
            (x - t) + hi
        };
        hi = t;
    }
    if lo != 0.0 && lo.is_finite() {
        hi + lo
    } else {
        hi
    }
}
fn confidence(n: usize, lo: usize, mid: usize, hi: usize) -> &'static str {
    if n < lo {
        "insufficient"
    } else if n < mid {
        "low"
    } else if n < hi {
        "moderate"
    } else {
        "high"
    }
}
fn conf(n: usize) -> &'static str {
    confidence(n, 5, 15, 40)
}
fn wilson(k: f64, n: usize) -> (f64, f64) {
    if n == 0 {
        return (0.0, 1.0);
    }
    let n = n as f64;
    let z = 1.96_f64;
    let p = k / n;
    let denom = 1.0 + z * z / n;
    let center = (p + z * z / (2.0 * n)) / denom;
    let half = z * (p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt() / denom;
    ((center - half).max(0.0), (center + half).min(1.0))
}

/// Validate the public filter contract without coercing booleans into timestamps.
pub fn normalize_filters(input: &Value) -> Result<Value> {
    let empty = serde_json::Map::new();
    let d = if input.is_null() {
        &empty
    } else {
        input
            .as_object()
            .ok_or_else(|| api_error(400, "filters must be an object"))?
    };
    let mut out = serde_json::Map::new();
    for key in ["since", "until"] {
        if let Some(raw) = d.get(key).filter(|v| !v.is_null() && **v != "") {
            let n = match raw {
                Value::Number(n) => n.as_i64(),
                Value::String(s)
                    if s.len() <= 16
                        && !s.trim().is_empty()
                        && s.trim().chars().all(|c| c.is_ascii_digit()) =>
                {
                    s.trim().parse::<i64>().ok()
                }
                _ => None,
            }
            .ok_or_else(|| api_error(400, format!("{key} must be an integer Unix timestamp")))?;
            if !(0..=253_402_300_799).contains(&n) {
                return Err(api_error(
                    400,
                    format!("{key} must be between 0 and 253402300799"),
                ));
            }
            out.insert(key.into(), json!(n));
        }
    }
    if let (Some(a), Some(b)) = (out.get("since"), out.get("until"))
        && integer(a) > integer(b)
    {
        return Err(api_error(400, "since must not be later than until"));
    }
    for (key, limit) in [("color", 5), ("opening", 512), ("opponent", 256)] {
        if let Some(v) = d.get(key).filter(|v| !v.is_null()) {
            let s = v
                .as_str()
                .filter(|s| s.chars().count() <= limit)
                .ok_or_else(|| {
                    api_error(
                        400,
                        format!("{key} must be text of at most {limit} characters"),
                    )
                })?;
            if key == "color" && !["", "any", "white", "black"].contains(&s) {
                return Err(api_error(400, "color must be white, black or any"));
            }
            if !s.is_empty() && !(key == "color" && s == "any") {
                out.insert(key.into(), json!(s));
            }
        }
    }
    if let Some(v) = d.get("time_classes").filter(|v| !v.is_null()) {
        let tcs = match v {
            Value::String(s) if s.len() <= 64 => s
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| json!(s))
                .collect::<Vec<_>>(),
            Value::Array(a) => a.clone(),
            _ => {
                return Err(api_error(
                    400,
                    "time_classes must be a list of at most four time classes",
                ));
            }
        };
        if tcs.len() > 4 {
            return Err(api_error(
                400,
                "time_classes must be a list of at most four time classes",
            ));
        }
        let mut unique = Vec::new();
        for tc in tcs {
            if !["bullet", "blitz", "rapid", "daily"].contains(&text(&tc)) {
                return Err(api_error(
                    400,
                    "time_classes must contain bullet, blitz, rapid or daily",
                ));
            }
            if !unique.contains(&tc) {
                unique.push(tc);
            }
        }
        if !unique.is_empty() {
            out.insert("time_classes".into(), json!(unique));
        }
    }
    if let Some(v) = d
        .get("rated")
        .filter(|v| !v.is_null() && **v != "" && **v != "any")
    {
        let b = match v {
            Value::Bool(b) => Some(*b),
            Value::Number(n) => match n.as_i64() {
                Some(0) => Some(false),
                Some(1) => Some(true),
                _ => None,
            },
            Value::String(s) => match s.to_lowercase().as_str() {
                "true" | "1" | "yes" => Some(true),
                "false" | "0" | "no" => Some(false),
                _ => None,
            },
            _ => None,
        }
        .ok_or_else(|| api_error(400, "rated must be true, false or any"))?;
        out.insert("rated".into(), json!(b));
    }
    Ok(Value::Object(out))
}

pub(super) fn where_clause(account: i64, f: &Value) -> (Vec<String>, Vec<Value>) {
    let mut clauses = vec!["ag.account_id=?".into()];
    let mut params = vec![json!(account)];
    for (key, sql) in [
        ("since", "g.end_time >= ?"),
        ("until", "g.end_time < ?"),
        ("color", "ag.color = ?"),
        ("opening", "g.opening_family = ?"),
    ] {
        if !f[key].is_null() {
            clauses.push(sql.into());
            params.push(f[key].clone());
        }
    }
    if let Some(a) = f["time_classes"].as_array() {
        clauses.push(format!(
            "g.time_class IN ({})",
            vec!["?"; a.len()].join(",")
        ));
        params.extend(a.iter().cloned());
    }
    if let Some(b) = f["rated"].as_bool() {
        clauses.push("g.rated=?".into());
        params.push(json!(i64::from(b)));
    }
    if !f["opponent"].is_null() {
        clauses
            .push("(LOWER(g.white_username)=LOWER(?) OR LOWER(g.black_username)=LOWER(?))".into());
        params.extend([f["opponent"].clone(), f["opponent"].clone()]);
    }
    (clauses, params)
}

pub fn scope_game_ids(db: &Database, account: i64, filters: &Value) -> Result<Vec<i64>> {
    let f = normalize_filters(filters)?;
    let (mut clauses, params) = where_clause(account, &f);
    clauses.push("g.rules IN ('chess','chess960')".into());
    Ok(db.query(&format!("SELECT g.id FROM account_games ag JOIN games g ON g.id=ag.game_id WHERE {} ORDER BY g.end_time,g.id",clauses.join(" AND ")),&params)?.iter().map(|r|integer(&r["id"])).collect())
}

// Insights only needs tag presence and the hanging-piece name. Skip the nested
// engine explanation payloads instead of retaining a JSON tree for every move.
#[derive(Debug, Default)]
struct Tags {
    present: u16,
    hanging_piece: Option<String>,
}
fn tag_bit(key: &str) -> u16 {
    match key {
        "hanging_piece" => 1 << 0,
        "threat_ignored" => 1 << 1,
        "allowed_mate" => 1 << 2,
        "back_rank" => 1 << 3,
        "allows_fork" => 1 << 4,
        "loses_material" => 1 << 5,
        "missed_punish" => 1 << 6,
        "missed_material" => 1 << 7,
        "missed_fork" => 1 << 8,
        "missed_mate" => 1 << 9,
        "self_inflicted" => 1 << 10,
        "positional" => 1 << 11,
        "tactical" => 1 << 12,
        "in_check" => 1 << 13,
        "time_pressure" => 1 << 14,
        _ => 0,
    }
}
// Use the same recursive deserializer path as Value: IgnoredAny alone skips
// serde_json's number-range and nesting checks for discarded payloads.
struct DiscardValue;
impl<'de> Deserialize<'de> for DiscardValue {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct DiscardVisitor;
        impl<'de> Visitor<'de> for DiscardVisitor {
            type Value = DiscardValue;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a JSON value")
            }
            fn visit_unit<E: serde::de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(DiscardValue)
            }
            fn visit_bool<E: serde::de::Error>(
                self,
                _: bool,
            ) -> std::result::Result<Self::Value, E> {
                Ok(DiscardValue)
            }
            fn visit_i64<E: serde::de::Error>(self, _: i64) -> std::result::Result<Self::Value, E> {
                Ok(DiscardValue)
            }
            fn visit_u64<E: serde::de::Error>(self, _: u64) -> std::result::Result<Self::Value, E> {
                Ok(DiscardValue)
            }
            fn visit_f64<E: serde::de::Error>(self, _: f64) -> std::result::Result<Self::Value, E> {
                Ok(DiscardValue)
            }
            fn visit_str<E: serde::de::Error>(
                self,
                _: &str,
            ) -> std::result::Result<Self::Value, E> {
                Ok(DiscardValue)
            }
            fn visit_seq<S: SeqAccess<'de>>(
                self,
                mut seq: S,
            ) -> std::result::Result<Self::Value, S::Error> {
                while seq.next_element::<DiscardValue>()?.is_some() {}
                Ok(DiscardValue)
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Self::Value, M::Error> {
                while map.next_entry::<String, DiscardValue>()?.is_some() {}
                Ok(DiscardValue)
            }
        }
        deserializer.deserialize_any(DiscardVisitor)
    }
}

impl<'de> Deserialize<'de> for Tags {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct TagsVisitor;
        impl<'de> Visitor<'de> for TagsVisitor {
            type Value = Tags;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("a move tags object")
            }
            fn visit_map<M: MapAccess<'de>>(
                self,
                mut map: M,
            ) -> std::result::Result<Tags, M::Error> {
                let mut tags = Tags::default();
                while let Some(key) = map.next_key::<String>()? {
                    tags.present |= tag_bit(&key);
                    if key == "hanging_piece" {
                        let value = map.next_value::<Value>()?;
                        tags.hanging_piece = value["piece"].as_str().map(str::to_owned);
                    } else {
                        map.next_value::<DiscardValue>()?;
                    }
                }
                Ok(tags)
            }
        }
        deserializer.deserialize_map(TagsVisitor)
    }
}

#[derive(Debug)]
struct Move {
    ply: i64,
    color: String,
    san: String,
    class: String,
    phase: String,
    ep: f64,
    cp: f64,
    accuracy: Option<f64>,
    before: Option<f64>,
    after: Option<f64>,
    clock: Option<f64>,
    spent: Option<f64>,
    book: bool,
    tags: Tags,
    epd: String,
    best: Value,
    endgame: String,
}
impl Move {
    fn tag(&self, key: &str) -> bool {
        self.tags.present & tag_bit(key) != 0
    }
    fn error(&self) -> bool {
        ERRORS.contains(&self.class.as_str())
    }
}
#[derive(Debug)]
struct Game {
    id: i64,
    row: Value,
    color: String,
    score: f64,
    outcome: &'static str,
    reason: String,
    user_rating: Option<f64>,
    opp_rating: Option<f64>,
    opponent: String,
    end: i64,
    analyzed: bool,
    accuracy: Option<f64>,
    opp_accuracy: Option<f64>,
    acpl: Option<f64>,
    chesscom_accuracy: Option<f64>,
    user: Vec<Move>,
    opp: Vec<Move>,
}
impl Game {
    fn c(&self) -> &str {
        if self.color == "white" { "w" } else { "b" }
    }
    fn moves(&self) -> Vec<&Move> {
        let mut moves = self.user.iter().chain(self.opp.iter()).collect::<Vec<_>>();
        moves.sort_by_key(|m| m.ply);
        moves
    }
    fn pov(&self, m: &Move) -> f64 {
        if m.color == self.c() {
            m.after.unwrap_or(50.0)
        } else {
            100.0 - m.after.unwrap_or(50.0)
        }
    }
}
fn loss_reason(s: &str) -> &str {
    match s {
        "checkmated" => "checkmate",
        "resigned" => "resignation",
        "timeout" => "time",
        "abandoned" => "abandonment",
        _ => "other",
    }
}

fn load(db: &Database, account: i64, filters: &Value, report_id: Option<i64>) -> Result<Vec<Game>> {
    // Report filters describe the selection at creation. Later opening backfills,
    // corrected timestamps or ratings must not change that frozen membership.
    let (mut clauses, mut params) = if report_id.is_some() {
        (vec!["ag.account_id=?".to_string()], vec![json!(account)])
    } else {
        where_clause(account, filters)
    };
    // A frozen report includes only the original rows, even after account imports or ID reuse.
    if let Some(rid) = report_id {
        clauses.push("EXISTS (SELECT 1 FROM report_games rg WHERE rg.report_id=? AND rg.game_id=g.id AND rg.game_uuid=g.uuid)".into());
        params.push(json!(rid));
    }
    let rows=db.query(&format!("SELECT g.id,g.url,g.time_control,g.time_class,g.rated,g.start_time,g.end_time,g.white_username,g.white_rating,g.white_result,g.black_username,g.black_rating,g.black_result,g.result,g.plies,g.chesscom_accuracy_white,g.chesscom_accuracy_black,g.first_moves,g.opening_eco,g.opening_name,g.opening_family,g.opening_variation,g.rules,ag.color,ga.game_id AS analyzed,ga.revision,ga.accuracy_white,ga.accuracy_black,ga.acpl_white,ga.acpl_black FROM account_games ag JOIN games g ON g.id=ag.game_id LEFT JOIN game_analysis ga ON ga.game_id=g.id WHERE {} ORDER BY g.end_time,g.id",clauses.join(" AND ")),&params)?;
    let mut games = Vec::with_capacity(rows.len());
    for row in rows {
        let white = row["color"] == "white";
        let (us, them) = if white {
            ("white", "black")
        } else {
            ("black", "white")
        };
        let ures = text(&row[format!("{us}_result")]);
        let ores = text(&row[format!("{them}_result")]);
        const DRAWS: &[&str] = &[
            "agreed",
            "repetition",
            "stalemate",
            "insufficient",
            "50move",
            "timevsinsufficient",
        ];
        let (score, outcome, reason) = if ures == "win" {
            (1.0, "win", loss_reason(ores))
        } else if DRAWS.contains(&ures) || DRAWS.contains(&ores) {
            (0.5, "draw", if DRAWS.contains(&ures) { ures } else { ores })
        } else {
            (0.0, "loss", loss_reason(ures))
        };
        games.push(Game {
            id: integer(&row["id"]),
            color: text(&row["color"]).into(),
            score,
            outcome,
            reason: reason.into(),
            user_rating: row[format!("{us}_rating")].as_f64(),
            opp_rating: row[format!("{them}_rating")].as_f64(),
            opponent: row[format!("{them}_username")]
                .as_str()
                .unwrap_or("?")
                .into(),
            end: integer(&row["end_time"]),
            analyzed: !row["analyzed"].is_null(),
            accuracy: row[format!("accuracy_{us}")].as_f64(),
            opp_accuracy: row[format!("accuracy_{them}")].as_f64(),
            acpl: row[format!("acpl_{us}")].as_f64(),
            chesscom_accuracy: row[format!("chesscom_accuracy_{us}")].as_f64(),
            row,
            user: vec![],
            opp: vec![],
        });
    }
    let lookup = games
        .iter()
        .enumerate()
        .filter(|(_, g)| g.analyzed)
        .map(|(i, g)| (g.id, i))
        .collect::<HashMap<_, _>>();
    let ids = games
        .iter()
        .filter(|g| g.analyzed)
        .map(|g| json!(g.id))
        .collect::<Vec<_>>();
    for chunk in ids.chunks(500) {
        let sql = format!(
            "SELECT game_id,ply,color,san,classification,phase,ep_loss,cp_loss,accuracy,win_before,win_after,clock,time_spent,is_book,tags,epd_before,best_san,endgame_type FROM moves WHERE game_id IN ({}) ORDER BY game_id,ply",
            vec!["?"; chunk.len()].join(",")
        );
        // Profiling identified temporary JSON row maps as the dominant load cost.
        // Read only the covering-index columns directly into owned, typed values.
        // The connection lock ends before grouping or any statistical computation.
        let rows = db.read(|connection| {
            let mut statement = connection.prepare(&sql)?;
            let rows = statement.query_map(
                rusqlite::params_from_iter(crate::db::sql_values(chunk)?),
                |r| {
                    let ep = r.get::<_, Option<f64>>(6)?.unwrap_or(0.0);
                    let tags = r
                        .get_ref(14)?
                        .as_str()
                        .ok()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or_default();
                    Ok((
                        r.get::<_, i64>(0)?,
                        Move {
                            ply: r.get(1)?,
                            color: r.get(2)?,
                            san: r.get(3)?,
                            class: r.get::<_, Option<String>>(4)?.unwrap_or_default(),
                            phase: r.get::<_, Option<String>>(5)?.unwrap_or_default(),
                            ep,
                            cp: r.get::<_, Option<f64>>(7)?.unwrap_or(0.0),
                            accuracy: r.get(8)?,
                            before: r.get(9)?,
                            after: r.get(10)?,
                            clock: r.get(11)?,
                            spent: r.get(12)?,
                            book: r.get::<_, Option<bool>>(13)?.unwrap_or(false),
                            tags,
                            epd: if ep >= 0.05 {
                                r.get::<_, Option<String>>(15)?.unwrap_or_default()
                            } else {
                                String::new()
                            },
                            best: if ep > 0.0 {
                                r.get::<_, Option<String>>(16)?
                                    .map(Value::String)
                                    .unwrap_or(Value::Null)
                            } else {
                                Value::Null
                            },
                            endgame: r
                                .get::<_, Option<String>>(17)?
                                .unwrap_or_else(|| "mixed".into()),
                        },
                    ))
                },
            )?;
            Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?)
        })?;
        for (gid, m) in rows {
            let Some(&idx) = lookup.get(&gid) else {
                continue;
            };
            if m.color == games[idx].c() {
                games[idx].user.push(m)
            } else {
                games[idx].opp.push(m)
            }
        }
    }
    Ok(games)
}

fn groups<'a, T>(items: &[&'a T], key: impl Fn(&T) -> String) -> Vec<(String, Vec<&'a T>)> {
    let mut out: Vec<(String, Vec<&T>)> = vec![];
    let mut map = HashMap::new();
    for &item in items {
        let k = key(item);
        let idx = *map.entry(k.clone()).or_insert_with(|| {
            out.push((k, vec![]));
            out.len() - 1
        });
        out[idx].1.push(item);
    }
    out
}
fn wdl(gs: &[&Game]) -> Value {
    let n = gs.len();
    let w = gs.iter().filter(|g| g.outcome == "win").count();
    let d = gs.iter().filter(|g| g.outcome == "draw").count();
    let pts = w as f64 + 0.5 * d as f64;
    let (lo, hi) = wilson(pts, n);
    json!({"games":n,"wins":w,"draws":d,"losses":n-w-d,"score":pct(pts,n),"score_ci":[round(100.0*lo,1),round(100.0*hi,1)],"confidence":conf(n)})
}
fn extend(mut base: Value, extra: Value) -> Value {
    if let (Some(a), Some(b)) = (base.as_object_mut(), extra.as_object()) {
        a.extend(b.iter().map(|(k, v)| (k.clone(), v.clone())));
    }
    base
}
fn example(g: &Game, m: &Move) -> Value {
    json!({"game_id":g.id,"ply":m.ply,"fen":null,"san":m.san,"best_san":m.best,"classification":m.class,"ep_loss":m.ep,"explanation":null,"phase":m.phase,"opponent":g.opponent,"end_time":g.end,"url":g.row["url"],"color":g.color,"outcome":g.outcome,"time_class":g.row["time_class"],"lichess_url":null})
}
fn top_examples(mut pairs: Vec<(&Game, &Move)>, limit: usize) -> Value {
    pairs.sort_by(|a, b| b.1.ep.total_cmp(&a.1.ep));
    let mut seen = HashSet::new();
    json!(
        pairs
            .into_iter()
            .filter(|(g, _)| seen.insert(g.id))
            .take(limit)
            .map(|(g, m)| example(g, m))
            .collect::<Vec<_>>()
    )
}
fn uri_component(s: &str, slash: bool) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) || (slash && b == b'/') {
            out.push(b as char)
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}
fn resources(key: &str) -> Value {
    static R: OnceLock<Value> = OnceLock::new();
    R.get_or_init(|| {
        serde_json::from_str(include_str!("resources.json")).expect("embedded resources")
    })[key]
        .clone()
}
fn opening_resources(family: &str) -> Value {
    let key = family
        .replace([':', ',', '\''], "")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join("_");
    let encoded = uri_component(&key, true);
    json!([{"title":format!("{family} — Lichess opening explorer"),"url":format!("https://lichess.org/opening/{encoded}"),"kind":"opening"},{"title":format!("Puzzles from the {family}"),"url":format!("https://lichess.org/training/{encoded}"),"kind":"puzzles"},{"title":format!("Community studies on the {family}"),"url":format!("https://lichess.org/study/search?q={}",uri_component(family,true)),"kind":"study"}])
}

fn overview(games: &[&Game], analyzed: &[&Game]) -> Value {
    let count = |opponent: bool| -> Value {
        let mut out = json!({});
        for class in CLASSES {
            let n = analyzed
                .iter()
                .flat_map(|g| if opponent { &g.opp } else { &g.user })
                .filter(|m| m.class == *class)
                .count();
            out[*class] = if analyzed.is_empty() {
                Value::Null
            } else {
                json!(round(n as f64 / analyzed.len() as f64, 2))
            };
        }
        out
    };
    let mut tcs = groups(games, |g| text(&g.row["time_class"]).into());
    tcs.retain(|(k, _)| !k.is_empty());
    tcs.sort_by(|a, b| a.0.cmp(&b.0));
    let by_tc=tcs.into_iter().map(|(tc,gs)|extend(wdl(&gs),json!({"time_class":tc,"accuracy":avg(gs.iter().filter(|g|g.analyzed).map(|g|g.accuracy))}))).collect::<Vec<_>>();
    let rated = games
        .iter()
        .filter(|g| g.opp_rating.is_some_and(|r| r != 0.0))
        .collect::<Vec<_>>();
    let avg_opp = if rated.is_empty() {
        None
    } else {
        Some(
            rated
                .iter()
                .map(|g| g.opp_rating.unwrap_or(0.0))
                .sum::<f64>()
                / rated.len() as f64,
        )
    };
    let perf = avg_opp.map(|a| {
        round(
            a + 400.0
                * (rated.iter().filter(|g| g.outcome == "win").count() as f64
                    - rated.iter().filter(|g| g.outcome == "loss").count() as f64)
                / rated.len() as f64,
            0,
        )
    });
    extend(
        wdl(games),
        json!({"white":wdl(&games.iter().copied().filter(|g|g.color=="white").collect::<Vec<_>>()),"black":wdl(&games.iter().copied().filter(|g|g.color=="black").collect::<Vec<_>>()),"by_time_class":by_tc,"analyzed":analyzed.len(),"accuracy":avg(analyzed.iter().map(|g|g.accuracy)),"opp_accuracy":avg(analyzed.iter().map(|g|g.opp_accuracy)),"acpl":avg(analyzed.iter().map(|g|g.acpl)),"chesscom_accuracy":avg(games.iter().map(|g|g.chesscom_accuracy)),"chesscom_accuracy_games":games.iter().filter(|g|g.chesscom_accuracy.is_some()).count(),"performance_rating":perf,"avg_opponent_rating":avg_opp.map(|a|round(a,0)),"per_game":count(false),"opp_per_game":count(true),"first_game":games.first().map(|g|g.end),"last_game":games.last().map(|g|g.end)}),
    )
}

fn ratings(games: &[&Game]) -> Value {
    let rated = games
        .iter()
        .copied()
        .filter(|g| {
            g.user_rating.is_some_and(|r| r != 0.0)
                && !text(&g.row["time_class"]).is_empty()
                && integer(&g.row["rated"]) != 0
        })
        .collect::<Vec<_>>();
    let mut out = json!({});
    for (tc, gs) in groups(&rated, |g| text(&g.row["time_class"]).into()) {
        let points = gs
            .iter()
            .map(|g| json!({"t":g.end,"rating":g.user_rating,"game_id":g.id}))
            .collect::<Vec<_>>();
        let step = (points.len() / 400).max(1);
        let mut sampled = points.iter().step_by(step).cloned().collect::<Vec<_>>();
        if (points.len() - 1) % step != 0 {
            sampled.push(points[points.len() - 1].clone());
        }
        let vals = gs.iter().filter_map(|g| g.user_rating).collect::<Vec<_>>();
        let start = vals[0];
        let end = vals[vals.len() - 1];
        out[tc] = json!({"points":sampled,"start":start,"end":end,"change":end-start,"peak":vals.iter().copied().reduce(f64::max),"low":vals.iter().copied().reduce(f64::min),"games":points.len()});
    }
    out
}

fn results(games: &[&Game]) -> Value {
    let reasons = |outcome: &str| {
        let mut map = BTreeMap::<String, usize>::new();
        for g in games.iter().filter(|g| g.outcome == outcome) {
            *map.entry(g.reason.clone()).or_default() += 1;
        }
        json!(map)
    };
    let mut by_opp = Vec::new();
    for (lo, hi, label) in [
        (-10000.0, -150.0, "≤ −150"),
        (-150.0, -50.0, "−150 to −50"),
        (-50.0, 50.0, "±50"),
        (50.0, 150.0, "+50 to +150"),
        (150.0, 10000.0, "≥ +150"),
    ] {
        let sub = games
            .iter()
            .copied()
            .filter(|g| {
                g.user_rating
                    .zip(g.opp_rating)
                    .is_some_and(|(u, o)| u != 0.0 && o != 0.0 && lo <= o - u && o - u < hi)
            })
            .collect::<Vec<_>>();
        if sub.is_empty() {
            continue;
        }
        let expected = sub
            .iter()
            .map(|g| {
                1.0 / (1.0 + 10_f64.powf((g.opp_rating.unwrap() - g.user_rating.unwrap()) / 400.0))
            })
            .sum::<f64>()
            / sub.len() as f64;
        by_opp.push(extend(
            wdl(&sub),
            json!({"label":label,"expected":round(100.0*expected,1)}),
        ));
    }
    let hours=(0..24).map(|h|{let sub=games.iter().filter(|g|g.end!=0&&DateTime::from_timestamp(g.end,0).is_some_and(|d|d.hour()==h)).collect::<Vec<_>>();json!({"hour_utc":h,"games":sub.len(),"score":pct(sub.iter().map(|g|g.score).sum(),sub.len())})}).collect::<Vec<_>>();
    // Native extension: 15-minute UTC buckets as [index, games, score points], so the browser can
    // place each game in the local hour that applied on its date, summer time included.
    let mut quarters = BTreeMap::<i64, (usize, f64)>::new();
    for g in games.iter().filter(|g| g.end != 0) {
        let bucket = quarters.entry(g.end.div_euclid(900)).or_default();
        bucket.0 += 1;
        bucket.1 += g.score;
    }
    let by_quarter_hour = quarters
        .into_iter()
        .map(|(q, (n, points))| json!([q, n, round(points, 1)]))
        .collect::<Vec<_>>();
    let mut lengths = Vec::new();
    for (lo, hi, label) in [
        (0, 40, "< 20 moves"),
        (40, 80, "20–40"),
        (80, 120, "40–60"),
        (120, 10000, "60+"),
    ] {
        let sub = games
            .iter()
            .copied()
            .filter(|g| g.row["plies"].as_i64().is_some_and(|p| lo <= p && p < hi))
            .collect::<Vec<_>>();
        if !sub.is_empty() {
            lengths.push(extend(wdl(&sub), json!({"label":label})));
        }
    }
    json!({"wins_by":reasons("win"),"losses_by":reasons("loss"),"draws_by":reasons("draw"),"by_opponent":by_opp,"by_hour_utc":hours,"by_quarter_hour":by_quarter_hour,"by_length":lengths})
}

fn sessions(games: &[&Game]) -> Value {
    let mut after = HashMap::from([
        ("win", Vec::new()),
        ("loss", Vec::new()),
        ("draw", Vec::new()),
    ]);
    let mut positions: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    let mut n = 0;
    let mut idx = 0;
    let mut prev: Option<&Game> = None;
    for &g in games {
        // A daily game lasts days, so it is not part of a sitting and must not become the
        // "previous game" of the live game played next.
        if g.row["time_class"] == "daily" {
            continue;
        }
        let start = g.row["start_time"]
            .as_i64()
            .filter(|t| *t != 0)
            .unwrap_or(g.end);
        // A game that started before the previous one ended (overlapping games) is not its
        // continuation either.
        if prev.is_none_or(|p| g.end == 0 || !(0..=1800).contains(&(start - p.end))) {
            n += 1;
            idx = 0;
        } else if let Some(p) = prev {
            after.get_mut(p.outcome).unwrap().push(g.score);
        }
        idx += 1;
        positions
            .entry(if idx <= 3 {
                "1–3"
            } else if idx <= 6 {
                "4–6"
            } else {
                "7+"
            })
            .or_default()
            .push(g.score);
        prev = Some(g);
    }
    let base = if games.is_empty() {
        0.0
    } else {
        games.iter().map(|g| g.score).sum::<f64>() / games.len() as f64
    };
    let mut res = json!({});
    for (k, v) in &after {
        res[*k] = json!({"games":v.len(),"score":pct(v.iter().sum(),v.len()),"confidence":confidence(v.len(),10,30,80)});
    }
    let losses = &after["loss"];
    let tilt = if losses.len() >= 20 {
        let sum = losses.iter().sum::<f64>();
        json!({"score_after_loss":res["loss"]["score"],"baseline":round(100.0*base,1),"difference":round(100.0*(sum/losses.len() as f64-base),1),"significant":wilson(sum,losses.len()).1<base})
    } else {
        Value::Null
    };
    let pos = positions
        .into_iter()
        .map(|(k, v)| {
            (
                k.to_owned(),
                json!({"games":v.len(),"score":pct(v.iter().sum(),v.len())}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    json!({"sessions":n,"after":res,"by_game_in_session":pos,"tilt":tilt})
}

fn trend(games: &[&Game]) -> Value {
    if games.is_empty() {
        return json!([]);
    }
    let monthly = games.last().unwrap().end - games[0].end > 150 * 86400;
    let mut buckets = groups(games, |g| {
        let d = DateTime::<Utc>::from_timestamp(g.end, 0).unwrap_or_default();
        if monthly {
            d.format("%Y-%m").to_string()
        } else {
            let w = d.iso_week();
            format!("{}-W{:02}", w.year(), w.week())
        }
    });
    buckets.sort_by(|a, b| a.0.cmp(&b.0));
    json!(buckets.into_iter().map(|(period,sub)|{let an=sub.iter().copied().filter(|g|g.analyzed).collect::<Vec<_>>();json!({"period":period,"games":sub.len(),"score":pct(sub.iter().map(|g|g.score).sum(),sub.len()),"accuracy":avg(an.iter().map(|g|g.accuracy)),"analyzed":an.len(),"blunders_per_game":if an.is_empty(){Value::Null}else{json!(round(an.iter().flat_map(|g|&g.user).filter(|m|m.class=="blunder").count() as f64/an.len() as f64,2))},"rating":sub.last().unwrap().user_rating})}).collect::<Vec<_>>())
}

fn openings(games: &[&Game], overall: f64) -> Value {
    let mut out = json!({});
    for color in ["white", "black"] {
        let sub = games
            .iter()
            .copied()
            .filter(|g| g.color == color)
            .collect::<Vec<_>>();
        let mut families = Vec::new();
        for (fam, gs) in groups(&sub, |g| {
            g.row["opening_family"]
                .as_str()
                .filter(|s| !s.is_empty())
                .unwrap_or("Unknown")
                .into()
        }) {
            let an = gs
                .iter()
                .copied()
                .filter(|g| g.analyzed)
                .collect::<Vec<_>>();
            let moves = an
                .iter()
                .flat_map(|g| &g.user)
                .filter(|m| m.phase == "opening" && !m.book)
                .collect::<Vec<_>>();
            let exit = an
                .iter()
                .filter_map(|g| {
                    let ms = g.moves();
                    let target = ms.iter().find(|m| m.ply == 20).copied().or_else(|| {
                        if ms.len() < 20 {
                            ms.last().copied()
                        } else {
                            None
                        }
                    });
                    target.map(|m| g.pov(m))
                })
                .collect::<Vec<_>>();
            let stats = wdl(&gs);
            let mut flag = None;
            if gs.len() >= 5 {
                let score = number(&stats["score"]);
                if number(&stats["score_ci"][1]) < overall
                    || (score <= overall - 15.0 && gs.len() >= 8)
                {
                    flag = Some("weakness");
                } else if number(&stats["score_ci"][0]) > overall
                    || (score >= overall + 15.0 && gs.len() >= 8)
                {
                    flag = Some("strength");
                }
            }
            let mut vars = groups(&gs, |g| {
                g.row["opening_name"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .unwrap_or(&fam)
                    .into()
            })
            .into_iter()
            .map(|(name, v)| extend(wdl(&v), json!({"name":name})))
            .collect::<Vec<_>>();
            vars.sort_by_key(|r| -integer(&r["games"]));
            vars.truncate(12);
            families.push(extend(stats,json!({"family":fam,"accuracy":avg(an.iter().map(|g|g.accuracy)),"opening_accuracy":avg(moves.iter().map(|m|m.accuracy)),"opening_errors_per_game":if an.is_empty(){Value::Null}else{json!(round(moves.iter().filter(|m|m.error()).count() as f64/an.len() as f64,2))},"exit_win":if exit.is_empty(){Value::Null}else{json!(round(exit.iter().sum::<f64>()/exit.len() as f64,1))},"analyzed":an.len(),"flag":flag,"variations":vars,"recent_game_ids":gs.iter().rev().take(5).map(|g|g.id).collect::<Vec<_>>(),"resources":if fam=="Unknown"{json!([])}else{opening_resources(&fam)}})));
        }
        families.sort_by_key(|r| -integer(&r["games"]));
        let mut first = groups(&sub, |g| {
            let fm = text(&g.row["first_moves"])
                .split_whitespace()
                .collect::<Vec<_>>();
            if color == "white" && !fm.is_empty() {
                fm[0].into()
            } else if color == "black" && fm.len() >= 2 {
                format!("1.{} {}", fm[0], fm[1])
            } else {
                String::new()
            }
        })
        .into_iter()
        .filter(|(k, _)| !k.is_empty())
        .map(|(m, gs)| extend(wdl(&gs), json!({"move":m})))
        .collect::<Vec<_>>();
        first.sort_by_key(|r| -integer(&r["games"]));
        first.truncate(12);
        out[color] = json!({"families":families,"first_moves":first});
    }
    out
}

fn rates(moves: &[&Move]) -> Value {
    let n = moves.len();
    let count = |classes: &[&str]| {
        moves
            .iter()
            .filter(|m| classes.contains(&m.class.as_str()))
            .count() as f64
    };
    json!({"moves":n,"accuracy":avg(moves.iter().map(|m|m.accuracy)),"acpl":if n==0{Value::Null}else{json!(round(moves.iter().map(|m|m.cp.min(1000.0)).sum::<f64>()/n as f64,2))},"inaccuracies_per_100":pct(count(&["inaccuracy"]),n),"mistakes_per_100":pct(count(&["mistake","miss"]),n),"blunders_per_100":pct(count(&["blunder"]),n)})
}

fn phases(analyzed: &[&Game]) -> Value {
    let mut out = json!({});
    let n = analyzed.len().max(1) as f64;
    for ph in PHASES {
        let u = analyzed
            .iter()
            .flat_map(|g| &g.user)
            .filter(|m| m.phase == *ph && !m.book)
            .collect::<Vec<_>>();
        let o = analyzed
            .iter()
            .flat_map(|g| &g.opp)
            .filter(|m| m.phase == *ph && !m.book)
            .collect::<Vec<_>>();
        out[*ph] = json!({"user":rates(&u),"opponent":rates(&o),"games":analyzed.iter().filter(|g|g.user.iter().any(|m|m.phase==*ph)).count(),"ep_lost_per_game":round(u.iter().map(|m|m.ep).sum::<f64>()/n,3),"opp_ep_lost_per_game":round(o.iter().map(|m|m.ep).sum::<f64>()/n,3),"confidence":confidence(u.len(),30,150,500)});
    }
    let all = analyzed
        .iter()
        .flat_map(|g| g.user.iter().chain(&g.opp))
        .filter(|m| m.phase == "endgame")
        .collect::<Vec<_>>();
    let mut kinds = Vec::new();
    let mut seen = HashSet::new();
    for m in all {
        if seen.insert(m.endgame.clone()) {
            kinds.push(m.endgame.clone());
        }
    }
    let mut types = Vec::new();
    for t in kinds {
        let u = analyzed
            .iter()
            .flat_map(|g| &g.user)
            .filter(|m| m.phase == "endgame" && m.endgame == t)
            .collect::<Vec<_>>();
        let o = analyzed
            .iter()
            .flat_map(|g| &g.opp)
            .filter(|m| m.phase == "endgame" && m.endgame == t)
            .collect::<Vec<_>>();
        types.push(json!({"type":t,"games":analyzed.iter().filter(|g|g.user.iter().any(|m|m.phase=="endgame"&&m.endgame==t)).count(),"user":rates(&u),"opponent":rates(&o),"confidence":confidence(u.len(),20,80,250)}));
    }
    types.sort_by_key(|r| -integer(&r["games"]));
    out["endgame_types"] = json!(types);
    out
}

fn errors(analyzed: &[&Game]) -> Value {
    let n = analyzed.len().max(1) as f64;
    let mut rows = Vec::new();
    let mut hung = BTreeMap::<String, usize>::new();
    for g in analyzed {
        for m in &g.user {
            if m.error() && m.tag("hanging_piece") {
                *hung
                    .entry(m.tags.hanging_piece.as_deref().unwrap_or("?").into())
                    .or_default() += 1;
            }
        }
    }
    let user_tag = |m: &Move, t: &str| {
        m.tag(t)
            && (m.error()
                || (t == "missed_mate"
                    && ["good", "excellent", "inaccuracy"].contains(&m.class.as_str())))
    };
    for (tag, label) in TAG_LABELS {
        let u = analyzed
            .iter()
            .flat_map(|g| g.user.iter().filter(|m| user_tag(m, tag)).map(|m| (*g, m)))
            .collect::<Vec<_>>();
        let o = analyzed
            .iter()
            .flat_map(|g| &g.opp)
            .filter(|m| m.error() && m.tag(tag))
            .count();
        rows.push(json!({"tag":tag,"label":label,"count":u.len(),"per_game":round(u.len() as f64/n,2),"opp_per_game":round(o as f64/n,2),"ep_lost_per_game":round(u.iter().map(|(_,m)|m.ep).sum::<f64>()/n,3),"examples":top_examples(u.clone(),3),"confidence":conf(u.len())}));
    }
    rows.sort_by(|a, b| number(&b["ep_lost_per_game"]).total_cmp(&number(&a["ep_lost_per_game"])));
    let total = analyzed
        .iter()
        .flat_map(|g| &g.user)
        .filter(|m| m.error())
        .count();
    let count_tag = |tag: &str| {
        analyzed
            .iter()
            .flat_map(|g| &g.user)
            .filter(|m| m.error() && m.tag(tag))
            .count() as f64
    };
    json!({"tags":rows,"hung_pieces":hung,"total_errors":total,"tactical_share":pct(count_tag("tactical"),total),"positional_share":pct(count_tag("positional"),total),"errors_per_game":round(total as f64/n,2),"opp_errors_per_game":round(analyzed.iter().flat_map(|g|&g.opp).filter(|m|m.error()).count() as f64/n,2)})
}

fn time_info(analyzed: &[&Game], games: &[&Game]) -> Value {
    let timed = analyzed
        .iter()
        .filter_map(|g| {
            let tc = text(&g.row["time_control"]).trim();
            static TC: OnceLock<regex::Regex> = OnceLock::new();
            if !TC
                .get_or_init(|| {
                    regex::Regex::new(r"^\d+(?:\+\d+(?:\.\d+)?)?$")
                        .expect("time-control expression")
                })
                .is_match(tc)
            {
                return None;
            }
            let base = tc.split('+').next()?.parse::<f64>().ok()?;
            if !base.is_finite() || base <= 0.0 || !g.user.iter().any(|m| m.clock.is_some()) {
                None
            } else {
                Some((*g, base))
            }
        })
        .collect::<Vec<_>>();
    let mut out = json!({"games":timed.len(),"confidence":confidence(timed.len(),5,20,60)});
    if timed.is_empty() {
        return out;
    }
    let mut tu: HashMap<&str, Vec<f64>> = HashMap::new();
    let mut to: HashMap<&str, Vec<f64>> = HashMap::new();
    let mut cu: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
    let mut co: BTreeMap<i64, Vec<f64>> = BTreeMap::new();
    let mut trouble = Vec::new();
    let mut low = Vec::new();
    let mut normal = Vec::new();
    let mut speed: [Vec<&Move>; 4] = Default::default();
    let mut endings = HashMap::from([
        ("win", Vec::new()),
        ("loss", Vec::new()),
        ("draw", Vec::new()),
    ]);
    for &(g, base) in &timed {
        let threshold = 10_f64.max(0.1 * base);
        let mut had_low = false;
        for m in &g.user {
            if let Some(t) = m.spent {
                tu.entry(&m.phase).or_default().push(t);
            }
            if let Some(c) = m.clock {
                cu.entry((m.ply + 1) / 2).or_default().push(c / base);
                if !m.book {
                    if c <= threshold {
                        had_low = true;
                        low.push(m);
                    } else {
                        normal.push(m);
                        if let Some(t) = m.spent {
                            let fraction = t / base;
                            if (0.0..10.0).contains(&fraction) {
                                speed[if fraction < 0.01 {
                                    0
                                } else if fraction < 0.03 {
                                    1
                                } else if fraction < 0.08 {
                                    2
                                } else {
                                    3
                                }]
                                .push(m);
                            }
                        }
                    }
                }
            }
        }
        for m in &g.opp {
            if let Some(t) = m.spent {
                to.entry(&m.phase).or_default().push(t);
            }
            if let Some(c) = m.clock {
                co.entry((m.ply + 1) / 2).or_default().push(c / base);
            }
        }
        if had_low {
            trouble.push(g);
        }
        if let Some(c) = g.user.iter().rev().find_map(|m| m.clock) {
            endings.get_mut(g.outcome).unwrap().push(100.0 * c / base);
        }
    }
    let mut avg_time = json!({});
    for &phase in PHASES {
        avg_time[phase] = json!({"user":avg(tu.get(phase).into_iter().flatten().map(|v|Some(*v))),"opponent":avg(to.get(phase).into_iter().flatten().map(|v|Some(*v)))});
    }
    out["avg_time_per_move"] = avg_time;
    out["clock_curve"]=json!(cu.iter().filter(|(_,v)|v.len()>=(timed.len()/10).max(3)).take(80).map(|(k,u)|json!({"move":k,"user":round(100.0*u.iter().sum::<f64>()/u.len() as f64,1),"opponent":co.get(k).map(|o|round(100.0*o.iter().sum::<f64>()/o.len() as f64,1)),"games":u.len()})).collect::<Vec<_>>());
    out["time_trouble"] = json!({"games":trouble.len(),"share":pct(trouble.len() as f64,timed.len()),"score":if trouble.is_empty(){Value::Null}else{wdl(&trouble)["score"].clone()},"errors_per_100_low":pct(low.iter().filter(|m|m.error()).count() as f64,low.len()),"errors_per_100_normal":pct(normal.iter().filter(|m|m.error()).count() as f64,normal.len()),"moves_low":low.len()});
    out["speed_vs_quality"]=json!(speed.iter().enumerate().map(|(i,ms)|json!({"label":(["very fast","fast","normal","long think"][i]),"range":(["0–1% of base","1–3% of base","3–8% of base","> 8% of base"][i]),"moves":ms.len(),"errors_per_100":pct(ms.iter().filter(|m|m.error()).count() as f64,ms.len()),"blunders_per_100":pct(ms.iter().filter(|m|m.class=="blunder").count() as f64,ms.len()),"accuracy":avg(ms.iter().map(|m|m.accuracy))})).collect::<Vec<_>>());
    let timeouts = games
        .iter()
        .copied()
        .filter(|g| g.outcome == "loss" && g.reason == "time")
        .collect::<Vec<_>>();
    let to_an = timeouts
        .iter()
        .copied()
        .filter(|g| g.analyzed && (!g.user.is_empty() || !g.opp.is_empty()))
        .collect::<Vec<_>>();
    let not_losing = to_an
        .iter()
        .copied()
        .filter(|g| g.moves().last().is_some_and(|m| g.pov(m) >= 50.0))
        .collect::<Vec<_>>();
    out["timeouts"] = json!({"losses_on_time":timeouts.len(),"share_of_losses":pct(timeouts.len() as f64,games.iter().filter(|g|g.outcome=="loss").count()),"in_non_losing_position":not_losing.len(),"analyzed":to_an.len(),"wins_on_time":games.iter().filter(|g|g.outcome=="win"&&g.reason=="time").count(),"examples":not_losing.iter().skip(not_losing.len().saturating_sub(5)).map(|g|json!({"game_id":g.id,"opponent":g.opponent,"end_time":g.end})).collect::<Vec<_>>()});
    out["clock_left_at_end"] = json!(
        endings
            .into_iter()
            .map(|(k, v)| (k, avg(v.into_iter().map(Some))))
            .collect::<BTreeMap<_, _>>()
    );
    out
}

fn advantage(analyzed: &[&Game]) -> Value {
    let (mut wins, mut converted, mut losing, mut saved) = (0usize, 0usize, 0usize, 0usize);
    let mut lost = Vec::new();
    type Decisive<'a> = (String, usize, Vec<(&'a Game, &'a Move)>);
    let mut decisive: Vec<Decisive<'_>> = Vec::new();
    for &g in analyzed {
        let ms = g.moves();
        let series = ms
            .iter()
            .copied()
            .filter(|m| m.ply >= 10)
            .map(|m| (m, g.pov(m)))
            .collect::<Vec<_>>();
        if series.is_empty() {
            continue;
        }
        // The first maximum has the same semantics as Python max, including equal plateaus.
        let &(peak_m, peak) = series
            .iter()
            .reduce(|a, b| if b.1 > a.1 { b } else { a })
            .unwrap();
        let low = series.iter().map(|(_, v)| *v).reduce(f64::min).unwrap();
        if peak >= 80.0 {
            wins += 1;
            if g.outcome == "win" {
                converted += 1;
            } else {
                let culprit = g
                    .user
                    .iter()
                    .filter(|m| m.ply > peak_m.ply)
                    .reduce(|a, b| if b.ep > a.ep { b } else { a });
                lost.push((g, peak, culprit));
            }
        }
        if low <= 20.0 {
            losing += 1;
            if g.outcome != "loss" {
                saved += 1;
            }
        }
        if g.outcome == "loss" && !g.user.is_empty() {
            let worst = g
                .user
                .iter()
                .reduce(|a, b| if b.ep > a.ep { b } else { a })
                .unwrap();
            let (key, evidence) = if worst.ep >= 0.2 {
                let kind = [
                    "allowed_mate",
                    "hanging_piece",
                    "allows_fork",
                    "threat_ignored",
                    "missed_punish",
                    "loses_material",
                    "positional",
                ]
                .into_iter()
                .find(|t| worst.tag(t))
                .unwrap_or("other");
                (format!("{}:{kind}", worst.phase), Some((g, worst)))
            } else if g.reason == "time" {
                ("time:timeout".into(), None)
            } else {
                ("gradual:no single decisive error".into(), None)
            };
            if let Some(entry) = decisive.iter_mut().find(|(k, _, _)| *k == key) {
                entry.1 += 1;
                if let Some(p) = evidence {
                    entry.2.push(p);
                }
            } else {
                decisive.push((key, 1, evidence.into_iter().collect()));
            }
        }
    }
    lost.sort_by_key(|(g, _, _)| -g.end);
    lost.truncate(15);
    let n_loss = analyzed.iter().filter(|g| g.outcome == "loss").count();
    decisive.sort_by_key(|(_, n, _)| std::cmp::Reverse(*n));
    json!({"winning_positions":wins,"converted":converted,"conversion_rate":pct(converted as f64,wins),"conversion_confidence":conf(wins),"losing_positions":losing,"saved":saved,"save_rate":pct(saved as f64,losing),"lost_advantages":lost.into_iter().map(|(g,peak,culprit)|json!({"game_id":g.id,"opponent":g.opponent,"end_time":g.end,"peak_win":round(peak,1),"outcome":g.outcome,"reason":g.reason,"culprit":culprit.map(|m|example(g,m))})).collect::<Vec<_>>(),"how_you_lose":decisive.into_iter().map(|(key,n,pairs)|{let (phase,kind)=key.split_once(':').unwrap();json!({"key":key,"phase":phase,"kind":kind,"count":n,"share":pct(n as f64,n_loss),"examples":top_examples(pairs,2)})}).collect::<Vec<_>>(),"losses_analyzed":n_loss})
}

fn missed(analyzed: &[&Game]) -> Value {
    let mut misses = Vec::new();
    let mut mates = Vec::new();
    let (mut errors, mut punished) = (0, 0);
    for &g in analyzed {
        let ms = g.moves();
        for (i, &m) in ms.iter().enumerate() {
            if m.color != g.c()
                && ["mistake", "blunder"].contains(&m.class.as_str())
                && i + 1 < ms.len()
            {
                errors += 1;
                let reply = ms[i + 1];
                if reply.ep < 0.05 {
                    punished += 1;
                } else {
                    misses.push((g, reply));
                }
            }
            if m.color == g.c() && m.tag("missed_mate") {
                mates.push((g, m));
            }
        }
    }
    json!({"opponent_errors":errors,"punished":punished,"punish_rate":pct(punished as f64,errors),"confidence":confidence(errors,10,30,100),"missed_count":misses.len(),"missed_mates":mates.len(),"examples":top_examples(misses,6),"missed_mate_examples":top_examples(mates,4)})
}

fn recurring(analyzed: &[&Game]) -> Value {
    let mut groups: Vec<(String, Vec<(&Game, &Move)>)> = Vec::new();
    let mut index = HashMap::new();
    for &g in analyzed {
        for m in &g.user {
            if m.ep >= 0.05 && !m.book && !m.epd.is_empty() {
                let i = *index.entry(&m.epd).or_insert_with(|| {
                    groups.push((m.epd.clone(), vec![]));
                    groups.len() - 1
                });
                groups[i].1.push((g, m));
            }
        }
    }
    let mut out = Vec::new();
    for (_, items) in groups {
        let mut gids = items.iter().map(|(g, _)| g.id).collect::<Vec<_>>();
        gids.sort_unstable();
        gids.dedup();
        if gids.len() < 2 {
            continue;
        }
        let mut played: Vec<(String, usize)> = Vec::new();
        let mut best: Vec<(Value, usize)> = Vec::new();
        for (_, m) in &items {
            if let Some((_, n)) = played.iter_mut().find(|(s, _)| *s == m.san) {
                *n += 1;
            } else {
                played.push((m.san.clone(), 1));
            }
            if let Some((_, n)) = best.iter_mut().find(|(s, _)| *s == m.best) {
                *n += 1;
            } else {
                best.push((m.best.clone(), 1));
            }
        }
        played.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        best.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        let avg_loss = items.iter().map(|(_, m)| m.ep).sum::<f64>() / items.len() as f64;
        let &(g, m) = items
            .iter()
            .reduce(|a, b| if b.1.ep > a.1.ep { b } else { a })
            .unwrap();
        out.push(json!({"fen":null,"occurrences":items.len(),"games":&gids[gids.len().saturating_sub(8)..],"played":played.into_iter().map(|(s,n)|json!({"san":s,"count":n})).collect::<Vec<_>>(),"best_san":best[0].0,"avg_ep_loss":round(avg_loss,3),"phase":m.phase,"move_number":(m.ply+1)/2,"color":g.color,"explanation":null,"example":example(g,m),"lichess_url":null,"impact":round(avg_loss*items.len() as f64,3)}));
    }
    out.sort_by(|a, b| number(&b["impact"]).total_cmp(&number(&a["impact"])));
    out.truncate(15);
    json!(out)
}

const AREAS: &[(&str, &str, &str, &str)] = &[
    (
        "hanging_piece",
        "Piece safety (hanging pieces)",
        "hanging_piece",
        "You left material en prise",
    ),
    (
        "threat_ignored",
        "Spotting your opponent's threats",
        "threat_ignored",
        "You missed an existing threat",
    ),
    (
        "allowed_mate",
        "King safety (allowing mates)",
        "allowed_mate",
        "You allowed a forced mate",
    ),
    (
        "allows_fork",
        "Avoiding forks and double attacks",
        "allows_fork",
        "You allowed a fork",
    ),
    (
        "missed_tactics",
        "Punishing mistakes & finding tactics",
        "missed_tactics",
        "You missed a tactic or failed to punish an error",
    ),
    (
        "conversion",
        "Converting winning positions",
        "conversion",
        "You made a serious error while clearly winning",
    ),
    (
        "endgame",
        "Endgame technique",
        "endgame",
        "You made a serious endgame error",
    ),
    (
        "opening",
        "Opening play",
        "opening",
        "You made a serious opening error",
    ),
    (
        "time_pressure",
        "Time management",
        "time",
        "You made a serious error in time trouble",
    ),
    (
        "positional",
        "Positional decisions",
        "positional",
        "You made a serious positional (non-tactical) error",
    ),
];
fn area_match(key: &str, m: &Move) -> bool {
    match key {
        "missed_tactics" => {
            m.class == "miss"
                || ["missed_material", "missed_fork", "missed_punish"]
                    .iter()
                    .any(|t| m.tag(t))
        }
        "conversion" => m.before.unwrap_or(0.0) >= 80.0,
        "endgame" => m.phase == "endgame",
        "opening" => m.phase == "opening",
        "positional" => m.tag("positional") && m.phase != "endgame",
        _ => m.tag(key),
    }
}

fn priorities(analyzed: &[&Game], openings: &Value, time: &Value, sessions: &Value) -> Value {
    if analyzed.is_empty() {
        return json!([]);
    }
    let n = analyzed.len() as f64;
    let mut out = Vec::new();
    for &(key, title, res, what) in AREAS {
        let u = analyzed
            .iter()
            .flat_map(|g| {
                g.user
                    .iter()
                    .filter(|m| (m.error() || m.class == "inaccuracy") && area_match(key, m))
                    .map(|m| (*g, m))
            })
            .collect::<Vec<_>>();
        let o = analyzed
            .iter()
            .flat_map(|g| &g.opp)
            .filter(|m| (m.error() || m.class == "inaccuracy") && area_match(key, m))
            .collect::<Vec<_>>();
        let serious = u
            .iter()
            .copied()
            .filter(|(_, m)| m.error())
            .collect::<Vec<_>>();
        let ep_u = accurate_sum(u.iter().map(|(_, m)| m.ep)) / n;
        let ep_o = accurate_sum(o.iter().map(|m| m.ep)) / n;
        let ratio = if ep_o > 0.0 {
            ep_u / ep_o
        } else if ep_u > 0.0 {
            2.0
        } else {
            1.0
        };
        let score = ep_u * ratio.clamp(0.6, 1.6);
        let ru = serious.len() as f64 / n;
        let ro = o.iter().filter(|m| m.error()).count() as f64 / n;
        let cmp = if ro == 0.0 {
            String::new()
        } else if ratio >= 1.25 {
            format!(" — {ratio:.1}× more costly than for your opponents")
        } else if ratio <= 0.8 {
            " — better than your opponents, but still a source of lost points".into()
        } else {
            " — similar to your opponents".into()
        };
        let summary = format!(
            "{what} {ru:.2} times per game (opponents: {ro:.2}){cmp}. This costs about {:.0} percentage points of winning chances per game.",
            100.0 * ep_u
        );
        let mut item = json!({"key":key,"title":title,"score":round(score,4),"ep_lost_per_game":round(ep_u,3),"opp_ep_lost_per_game":round(ep_o,3),"vs_opponents":round(ratio,2),"errors":serious.len(),"errors_per_game":round(ru,2),"opp_errors_per_game":round(ro,2),"confidence":conf(serious.len()),"summary":summary,"examples":top_examples(if serious.is_empty(){u}else{serious},4),"resources":resources(res)});
        if key == "opening" {
            let weak = ["white", "black"]
                .into_iter()
                .flat_map(|c| openings[c]["families"].as_array().into_iter().flatten())
                .filter(|f| f["flag"] == "weakness")
                .min_by(|a, b| number(&a["score"]).total_cmp(&number(&b["score"])));
            if let Some(worst) = weak {
                item["detail"] = json!(format!(
                    "Weakest opening: {} — {}% score over {} games.",
                    text(&worst["family"]),
                    worst["score"],
                    worst["games"]
                ));
                let mut r = worst["resources"].as_array().cloned().unwrap_or_default();
                r.extend(item["resources"].as_array().into_iter().flatten().cloned());
                item["resources"] = json!(r);
            }
        }
        if key == "time_pressure" && integer(&time["timeouts"]["in_non_losing_position"]) > 0 {
            item["detail"] = json!(format!(
                "{} games lost on time in positions that were not losing.",
                time["timeouts"]["in_non_losing_position"]
            ));
        }
        out.push(item);
    }
    let (mut ranked, mut rest): (Vec<_>, Vec<_>) = out
        .into_iter()
        .partition(|x| x["confidence"] != "insufficient" && number(&x["score"]) > 0.0);
    ranked.sort_by(|a, b| number(&b["score"]).total_cmp(&number(&a["score"])));
    for (i, item) in ranked.iter_mut().enumerate() {
        item["rank"] = json!(i + 1);
    }
    for item in &mut rest {
        item["rank"] = Value::Null;
    }
    let tilt = &sessions["tilt"];
    if tilt["significant"] == true {
        ranked.push(json!({"key":"tilt","title":"Playing on after losses (tilt)","rank":ranked.len()+1,"confidence":"moderate","score":0,"summary":format!("You score {}% in games started right after a loss, versus {}% overall. Taking a short break after a loss may help.",tilt["score_after_loss"],tilt["baseline"]),"examples":[],"resources":resources("tilt")}));
    }
    ranked.extend(rest);
    json!(ranked)
}

fn hydrate(db: &Database, result: &mut Value) -> Result<()> {
    fn keys(node: &Value, out: &mut HashSet<(i64, i64)>) {
        match node {
            Value::Object(map) => {
                if map.contains_key("lichess_url")
                    && node["fen"].is_null()
                    && map.contains_key("ply")
                {
                    out.insert((integer(&node["game_id"]), integer(&node["ply"])));
                }
                for v in map.values() {
                    keys(v, out);
                }
            }
            Value::Array(a) => {
                for v in a {
                    keys(v, out);
                }
            }
            _ => {}
        }
    }
    let mut all = HashSet::new();
    keys(result, &mut all);
    let all = all.into_iter().collect::<Vec<_>>();
    let mut found = HashMap::new();
    for chunk in all.chunks(400) {
        let sql = format!(
            "SELECT game_id,ply,fen_before,explanation FROM moves WHERE (game_id,ply) IN (VALUES {})",
            vec!["(?,?)"; chunk.len()].join(",")
        );
        let params = chunk
            .iter()
            .flat_map(|(g, p)| [json!(g), json!(p)])
            .collect::<Vec<_>>();
        for r in db.query(&sql, &params)? {
            found.insert(
                (integer(&r["game_id"]), integer(&r["ply"])),
                (r["fen_before"].clone(), r["explanation"].clone()),
            );
        }
    }
    fn fill(node: &mut Value, found: &HashMap<(i64, i64), (Value, Value)>) {
        match node {
            Value::Object(map) => {
                if map.contains_key("lichess_url")
                    && map.get("fen").is_none_or(Value::is_null)
                    && map.contains_key("ply")
                {
                    let key = (
                        map.get("game_id").map(integer).unwrap_or(0),
                        map.get("ply").map(integer).unwrap_or(0),
                    );
                    if let Some((fen, expl)) = found.get(&key) {
                        map.insert("fen".into(), fen.clone());
                        map.insert("explanation".into(), expl.clone());
                        map.insert(
                            "lichess_url".into(),
                            fen.as_str()
                                .filter(|s| !s.is_empty())
                                .map(|s| {
                                    json!(format!(
                                        "https://lichess.org/analysis/{}",
                                        uri_component(&s.replace(' ', "_"), true)
                                    ))
                                })
                                .unwrap_or(Value::Null),
                        );
                    }
                }
                for v in map.values_mut() {
                    fill(v, found);
                }
            }
            Value::Array(a) => {
                for v in a {
                    fill(v, found);
                }
            }
            _ => {}
        }
    }
    fill(result, &found);
    if let Some(rows) = result["recurring"].as_array_mut() {
        for r in rows {
            for key in ["fen", "lichess_url", "explanation"] {
                r[key] = r["example"][key].clone();
            }
        }
    }
    Ok(())
}

pub fn compute(db: &Database, account_id: i64, filters: &Value) -> Result<Value> {
    compute_scope(db, account_id, filters, None)
}
pub(super) fn compute_scope(
    db: &Database,
    account_id: i64,
    filters: &Value,
    report_id: Option<i64>,
) -> Result<Value> {
    // Revision changes are committed atomically with the analysis rows. Do not publish
    // a mixture of old game summaries and new move rows during concurrent analysis.
    for _ in 0..5 {
        let before = db.scalar("SELECT revision FROM data_revision WHERE id=1", &[])?;
        let result = compute_once(db, account_id, filters, report_id)?;
        if before == db.scalar("SELECT revision FROM data_revision WHERE id=1", &[])? {
            return Ok(result);
        }
    }
    Err(api_error(
        503,
        "Analysis data is changing; retry the insight request shortly.",
    ))
}
fn compute_once(
    db: &Database,
    account_id: i64,
    filters: &Value,
    report_id: Option<i64>,
) -> Result<Value> {
    let began = std::time::Instant::now();
    let f = normalize_filters(filters)?;
    let loaded = load(db, account_id, &f, report_id)?;
    let loaded_at = std::time::Instant::now();
    let games = loaded.iter().collect::<Vec<_>>();
    let analyzed = games
        .iter()
        .copied()
        .filter(|g| g.analyzed && !g.user.is_empty())
        .collect::<Vec<_>>();
    let overview = overview(&games, &analyzed);
    let openings = openings(&games, number(&overview["score"]));
    let sessions = sessions(&games);
    let time = time_info(&analyzed, &games);
    let mut notes = Vec::new();
    if analyzed.len() < 10 {
        notes.push(
            "Fewer than 10 analysed games: engine-based patterns are not yet reliable.".to_string(),
        );
    } else if analyzed.len() < 40 {
        notes.push(
            "Fewer than 40 analysed games: treat engine-based patterns as preliminary.".to_string(),
        );
    }
    let un = games
        .iter()
        .filter(|g| !g.analyzed && ["chess", "chess960"].contains(&text(&g.row["rules"])))
        .map(|g| g.id)
        .collect::<Vec<_>>();
    // Variants are never reviewed, so they are not "not analysed yet". The coverage bar
    // in the interface names them separately.
    if !un.is_empty() {
        notes.push(format!("{} game(s) in this selection are not analysed yet; result statistics include them, engine statistics do not.",un.len()));
    }
    let mut result = json!({"filters":f,"data_quality":{"games":games.len(),"analyzed":analyzed.len(),"coverage":pct(analyzed.len() as f64,games.len()),"unanalyzed_ids":&un[un.len().saturating_sub(2000)..],"notes":notes,"confidence":confidence(analyzed.len(),10,40,150)},"overview":overview,"ratings":ratings(&games),"results":results(&games),"sessions":sessions,"trend":trend(&games),"openings":openings,"phases":phases(&analyzed),"errors":errors(&analyzed),"time":time,"advantage":advantage(&analyzed),"missed":missed(&analyzed),"recurring":recurring(&analyzed),"priorities":priorities(&analyzed,&openings,&time,&sessions)});
    let aggregated_at = std::time::Instant::now();
    hydrate(db, &mut result)?;
    tracing::debug!(
        games = games.len(),
        load_ms = loaded_at.duration_since(began).as_secs_f64() * 1000.0,
        aggregate_ms = aggregated_at.duration_since(loaded_at).as_secs_f64() * 1000.0,
        hydrate_ms = aggregated_at.elapsed().as_secs_f64() * 1000.0,
        "Computed account insights"
    );
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn compact_tags_match_json_presence_and_piece_semantics() {
        for raw in [
            r#"{}"#,
            r#"{"hanging_piece":{"piece":"queen","extra":[1,2]},"tactical":true}"#,
            r#"{"hanging_piece":null,"positional":null,"back_rank":false}"#,
            r#"{"hanging_piece":{"piece":42},"unknown":{"deep":[null,true,{}]}}"#,
            r#"{"hanging_piece":{"piece":"rook"},"hanging_piece":{"piece":"pawn"}}"#,
            r#"{"tactical":true,"tactical":null}"#,
            r#"[]"#,
            r#"null"#,
            r#"42"#,
            r#"{"tactical":true,"broken":]}"#,
            r#"{"tactical":true,"unknown":1e999}"#,
            r#"{"tactical":true,"unknown":{"\uD800":0}}"#,
            r#"{"tactical":true,"unknown":"\uD800"}"#,
            r#"{"tactical":true,"unknown":{"\uD83D\uDE00":"valid pair"}}"#,
        ] {
            let original = serde_json::from_str::<Value>(raw).unwrap_or_else(|_| json!({}));
            let compact = serde_json::from_str::<Tags>(raw).unwrap_or_default();
            for &(key, _) in TAG_LABELS {
                assert_ne!(
                    tag_bit(key),
                    0,
                    "tag missing from compact representation: {key}"
                );
                assert_eq!(
                    compact.present & tag_bit(key) != 0,
                    original.get(key).is_some(),
                    "{raw}: {key}"
                );
            }
            assert_eq!(
                compact.hanging_piece.as_deref(),
                original["hanging_piece"]["piece"].as_str(),
                "{raw}"
            );
        }
    }
    #[test]
    fn compact_tags_preserve_json_nesting_limit() {
        for depth in [125, 126, 127, 128, 129, 256] {
            let raw = format!(
                r#"{{"tactical":true,"unknown":{}0{}}}"#,
                "[".repeat(depth),
                "]".repeat(depth)
            );
            let original = serde_json::from_str::<Value>(&raw);
            let compact = serde_json::from_str::<Tags>(&raw);
            assert_eq!(compact.is_ok(), original.is_ok(), "depth {depth}");
        }
    }
    #[test]
    fn strict_filters() {
        for bad in [
            json!([]),
            json!({"since":true}),
            json!({"since":-1}),
            json!({"since":1.5}),
            json!({"until":253402300800_i64}),
            json!({"since":2,"until":1}),
            json!({"time_classes":vec!["blitz";5]}),
            json!({"color":"purple"}),
            json!({"rated":2}),
            json!({"opponent":[]}),
        ] {
            assert!(normalize_filters(&bad).is_err(), "accepted {bad}");
        }
        assert_eq!(normalize_filters(&json!({"since":"0","until":253402300799_i64,"rated":"false","time_classes":"blitz, rapid,blitz","color":"any"})).unwrap(),json!({"since":0,"until":253402300799_i64,"rated":false,"time_classes":["blitz","rapid"]}));
    }
    #[test]
    fn statistical_boundaries() {
        assert_eq!(wilson(0.0, 0), (0.0, 1.0));
        let (lo, hi) = wilson(5.0, 10);
        assert!(lo < 0.5 && hi > 0.5);
        assert_eq!(conf(3), "insufficient");
        assert_eq!(conf(50), "high");
        assert_eq!(pct(2.5, 4), json!(62.5));
    }
}
