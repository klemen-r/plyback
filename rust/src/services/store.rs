use crate::{
    ANALYSIS_VERSION, analysis,
    db::{Database, sql_values},
    now, pgn,
};
use anyhow::Result;
use chrono::NaiveDateTime;
use rusqlite::{OptionalExtension, Transaction, params_from_iter};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use url::Url;

fn safe_url(raw: Option<&str>) -> Option<String> {
    let raw = raw.filter(|s| s.len() <= 2048)?;
    let url = Url::parse(raw).ok()?;
    (matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none())
    .then(|| url.to_string())
}
fn validate_chesscom_metadata(game: &Value) -> Result<()> {
    if !game.is_object() {
        anyhow::bail!("Chess.com game must be an object");
    }
    for (key, max) in [
        ("uuid", 200),
        ("url", 2048),
        ("rules", 40),
        ("time_control", 100),
        ("time_class", 40),
        ("initial_setup", 256),
    ] {
        if let Some(v) = game.get(key).filter(|v| !v.is_null())
            && v.as_str()
                .is_none_or(|s| s.len() > max || s.contains(['\n', '\r', '\0']))
        {
            anyhow::bail!("Invalid Chess.com {key}");
        }
    }
    for key in ["start_time", "end_time"] {
        if game
            .get(key)
            .filter(|v| !v.is_null())
            .is_some_and(|v| v.as_i64().is_none_or(|n| !(0..=253402300799).contains(&n)))
        {
            anyhow::bail!("Invalid Chess.com {key}");
        }
    }
    if game
        .get("rated")
        .is_some_and(|v| !v.is_boolean() && !v.is_null())
    {
        anyhow::bail!("Invalid Chess.com rated flag");
    }
    for color in ["white", "black"] {
        if let Some(player) = game.get(color).filter(|v| !v.is_null()) {
            if !player.is_object() {
                anyhow::bail!("Invalid Chess.com player metadata");
            }
            for key in ["username", "result"] {
                if player.get(key).filter(|v| !v.is_null()).is_some_and(|v| {
                    v.as_str()
                        .is_none_or(|s| s.len() > 200 || s.contains(['\n', '\r', '\0']))
                }) {
                    anyhow::bail!("Invalid Chess.com player {key}");
                }
            }
            if player
                .get("rating")
                .filter(|v| !v.is_null())
                .is_some_and(|v| v.as_i64().is_none_or(|n| !(0..=10000).contains(&n)))
            {
                anyhow::bail!("Invalid Chess.com player rating");
            }
        }
        if game["accuracies"]
            .get(color)
            .filter(|v| !v.is_null())
            .is_some_and(|v| v.as_f64().is_none_or(|n| !(0.0..=100.0).contains(&n)))
        {
            anyhow::bail!("Invalid Chess.com accuracy");
        }
    }
    Ok(())
}

pub fn decode(value: &Value, default: Value) -> Value {
    value
        .as_str()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or(default)
}
fn timestamp(date: Option<&str>, time: Option<&str>) -> Value {
    date.zip(time)
        .and_then(|(d, t)| {
            NaiveDateTime::parse_from_str(&format!("{d} {t}"), "%Y.%m.%d %H:%M:%S").ok()
        })
        .map(|t| json!(t.and_utc().timestamp()))
        .unwrap_or(Value::Null)
}
const GAME_COLS: &[&str] = &[
    "uuid",
    "url",
    "source",
    "pgn",
    "rules",
    "time_control",
    "time_class",
    "rated",
    "start_time",
    "end_time",
    "white_username",
    "white_rating",
    "white_result",
    "black_username",
    "black_rating",
    "black_result",
    "result",
    "termination",
    "eco",
    "eco_url",
    "initial_fen",
    "plies",
    "first_moves",
    "opening_eco",
    "opening_name",
    "opening_family",
    "opening_variation",
    "chesscom_accuracy_white",
    "chesscom_accuracy_black",
    "imported_at",
];

pub fn game_row_from_pgn(text: &str) -> Result<Value> {
    let game = pgn::parse(text)?;
    let h = &game.headers;
    let get = |key: &str| h.get(key).map(String::as_str);
    let result = get("Result").unwrap_or("*");
    // How the loser lost: checkmate is read from the final position, other reasons from the
    // Termination header; otherwise the generic "lose" (shown as just "Won"/"Lost").
    let termination = get("Termination").unwrap_or("").to_ascii_lowercase();
    let mated = pgn::board(&game.final_fen, game.chess960)
        .map(|b| shakmaty::Position::is_checkmate(&b))
        .unwrap_or(false);
    let loser = if mated || termination.contains("checkmate") {
        "checkmated"
    } else if termination.contains("time") {
        "timeout"
    } else if termination.contains("resign") {
        "resigned"
    } else if termination.contains("abandon") {
        "abandoned"
    } else {
        "lose"
    };
    let (wr, br) = match result {
        "1-0" => (Some("win"), Some(loser)),
        "0-1" => (Some(loser), Some("win")),
        "1/2-1/2" => (Some("agreed"), Some("agreed")),
        _ => (None, None),
    };
    let tc = get("TimeControl");
    let tc_class = tc.and_then(|tc| {
        // Chess.com writes daily games as "1/<seconds per move>".
        if tc
            .strip_prefix("1/")
            .is_some_and(|s| s.parse::<u64>().is_ok())
        {
            return Some("daily");
        }
        // Classical PGN periods ("40/7200:3600") are classed by the first period's time.
        let first = tc.split(':').next()?;
        let mut pieces = first.rsplit('/').next()?.split('+');
        let secs = pieces.next()?.parse::<f64>().ok()?
            + 40. * pieces.next().unwrap_or("0").parse::<f64>().ok()?;
        Some(if secs < 180. {
            "bullet"
        } else if secs < 600. {
            "blitz"
        } else {
            "rapid"
        })
    });
    let (opening, _) = analysis::identify_opening(&game);
    let opening = opening.unwrap_or(Value::Null);
    let mut row = json!({
        "uuid":format!("pgn-{}",&hex::encode(Sha1::digest(text.trim().as_bytes()))[..24]),
        "url":safe_url(get("Link")),"source":"pgn","pgn":format!("{}\n",text.trim()),"rules":game.rules,
        "time_control":tc,"time_class":tc_class,"rated":null,
        "start_time":timestamp(get("UTCDate"),get("UTCTime")),
        // Without EndTime (Lichess and most other sites), the start time is a far better end time
        // than midnight, which put every such game in the 00:00 slot and broke session detection.
        "end_time":timestamp(get("EndDate").or(get("UTCDate")).or(get("Date")),Some(get("EndTime").or(get("UTCTime")).unwrap_or("00:00:00").split(' ').next().unwrap_or("00:00:00"))),
        "white_username":get("White"),"white_rating":get("WhiteElo").and_then(|s|s.parse::<i64>().ok()),"white_result":wr,
        "black_username":get("Black"),"black_rating":get("BlackElo").and_then(|s|s.parse::<i64>().ok()),"black_result":br,
        "result":result,"termination":get("Termination"),"eco":get("ECO"),"eco_url":get("ECOUrl"),
        "initial_fen":game.initial_fen,"plies":game.moves.len(),
        "first_moves":game.moves.iter().take(2).map(|m|m.san.as_str()).collect::<Vec<_>>().join(" "),
        "opening_eco":opening["eco"],"opening_name":opening["name"],"opening_family":opening["family"],"opening_variation":opening["variation"],
        "chesscom_accuracy_white":null,"chesscom_accuracy_black":null,"imported_at":now()
    });
    // ECO metadata is useful for unindexed named openings, but does not mark a move Book.
    if row["opening_family"].is_null()
        && let Some(url) = get("ECOUrl")
        && let Some(name) = url.strip_prefix("https://www.chess.com/openings/")
    {
        let name = name.split('?').next().unwrap_or(name);
        if let Some((family, variation)) = eco_url_family(name) {
            row["opening_name"] = json!(name.replace('-', " "));
            row["opening_family"] = json!(family);
            row["opening_eco"] = json!(get("ECO"));
            row["opening_variation"] = json!(variation);
        }
    }
    Ok(row)
}

/// Family and variation of a Chess.com ECOUrl name such as
/// "Sicilian-Defense-Old-Sicilian-Variation-3.d4", so these games join their family
/// ("Sicilian Defense") instead of each line becoming a family of its own. The family is
/// the longest bundled family whose words begin the name, else the name up to its first
/// family word (Opening, Defense, Game, ...), else the whole name.
fn eco_url_family(name: &str) -> Option<(String, String)> {
    fn key(word: &str) -> String {
        word.chars()
            .filter(|c| c.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    }
    static FAMILIES: std::sync::OnceLock<Vec<(Vec<String>, &'static str)>> =
        std::sync::OnceLock::new();
    let families = FAMILIES.get_or_init(|| {
        analysis::opening_families()
            .iter()
            .map(|family| {
                let words = family
                    .split(|c: char| c.is_whitespace() || c == '-')
                    .map(key)
                    .filter(|w| !w.is_empty())
                    .collect();
                (words, family.as_str())
            })
            .collect()
    });
    let words = name
        .split('-')
        .filter(|w| !w.is_empty())
        .collect::<Vec<_>>();
    if words.is_empty() {
        return None;
    }
    let keys = words.iter().map(|w| key(w)).collect::<Vec<_>>();
    let (len, family) = families
        .iter()
        .filter(|(f, _)| !f.is_empty() && keys.starts_with(f))
        .max_by_key(|(f, _)| f.len())
        .map(|(f, family)| (f.len(), family.to_string()))
        .unwrap_or_else(|| {
            let len = keys
                .iter()
                .position(|k| {
                    matches!(
                        k.as_str(),
                        "opening" | "defense" | "defence" | "game" | "gambit" | "attack" | "system"
                    )
                })
                .map_or(words.len(), |i| i + 1);
            (len, words[..len].join(" "))
        });
    Some((family, words[len..].join(" ")))
}

pub fn game_row_from_chesscom(game: &Value) -> Result<Option<Value>> {
    validate_chesscom_metadata(game)?;
    let Some(text) = game["pgn"].as_str().filter(|s| !s.is_empty()) else {
        return Ok(None);
    };
    let rules = game["rules"].as_str().unwrap_or("chess");
    // Unsupported variants remain in the library but are never queued for Stockfish.
    let mut row = match game_row_from_pgn(text) {
        Ok(row) => row,
        Err(e) if !matches!(rules, "chess" | "chess960") => {
            tracing::debug!(error=%e,"Imported unsupported chess variant metadata");
            let mut row = json!({});
            for col in GAME_COLS {
                row[*col] = Value::Null;
            }
            row["pgn"] = json!(text);
            row["imported_at"] = json!(now());
            row
        }
        Err(e) => return Err(e),
    };
    row["uuid"] = game
        .get("uuid")
        .filter(|v| v.is_string())
        .cloned()
        .unwrap_or_else(|| {
            json!(hex::encode(Sha1::digest(
                game["url"].as_str().unwrap_or(text).as_bytes()
            )))
        });
    row["url"] = json!(safe_url(game["url"].as_str()));
    row["source"] = json!("chesscom");
    row["rules"] = json!(rules);
    row["rated"] = json!(i64::from(game["rated"].as_bool().unwrap_or(false)));
    for key in ["time_control", "time_class", "end_time"] {
        row[key] = game[key].clone();
    }
    if !game["start_time"].is_null() {
        row["start_time"] = game["start_time"].clone();
    }
    if let Some(fen) = game["initial_setup"].as_str().filter(|s| !s.is_empty()) {
        row["initial_fen"] = json!(fen);
    }
    for color in ["white", "black"] {
        for key in ["username", "rating", "result"] {
            row[format!("{color}_{key}")] = game[color][key].clone();
        }
        row[format!("chesscom_accuracy_{color}")] = game["accuracies"][color].clone();
    }
    let wr = row["white_result"].as_str().unwrap_or("");
    let br = row["black_result"].as_str().unwrap_or("");
    let draws = [
        "agreed",
        "repetition",
        "stalemate",
        "insufficient",
        "50move",
        "timevsinsufficient",
    ];
    if wr == "win" {
        row["result"] = json!("1-0")
    } else if br == "win" {
        row["result"] = json!("0-1")
    } else if draws.contains(&wr) || draws.contains(&br) {
        row["result"] = json!("1/2-1/2")
    }
    Ok(Some(row))
}

pub fn upsert_games(tx: &Transaction<'_>, rows: &[Value]) -> Result<Vec<(i64, bool)>> {
    let sql = format!(
        "INSERT INTO games({}) VALUES({})",
        GAME_COLS.join(","),
        vec!["?"; GAME_COLS.len()].join(",")
    );
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let existing = tx
            .query_row(
                "SELECT id,chesscom_accuracy_white FROM games WHERE uuid=?",
                [row["uuid"].as_str()],
                |r| Ok((r.get::<_, i64>(0)?, r.get::<_, Option<f64>>(1)?)),
            )
            .optional()?;
        if let Some((id, acc)) = existing {
            if acc.is_none() && !row["chesscom_accuracy_white"].is_null() {
                tx.execute("UPDATE games SET chesscom_accuracy_white=?,chesscom_accuracy_black=? WHERE id=?",rusqlite::params![row["chesscom_accuracy_white"].as_f64(),row["chesscom_accuracy_black"].as_f64(),id])?;
            }
            out.push((id, false));
            continue;
        }
        tx.execute(
            &sql,
            params_from_iter(sql_values(
                &GAME_COLS
                    .iter()
                    .map(|key| row[*key].clone())
                    .collect::<Vec<_>>(),
            )?),
        )?;
        out.push((tx.last_insert_rowid(), true));
    }
    Ok(out)
}
pub fn link_games(
    tx: &Transaction<'_>,
    account_id: i64,
    username: &str,
    rows: &[Value],
    ids: &[(i64, bool)],
) -> Result<Vec<bool>> {
    // For each row, whether it was newly linked to this account. A game another tracked account
    // already imported is not new to the database, but it is new to this account.
    let mut linked = Vec::with_capacity(rows.len());
    for (row, (id, _)) in rows.iter().zip(ids) {
        let color = if row["white_username"]
            .as_str()
            .unwrap_or("")
            .eq_ignore_ascii_case(username)
        {
            "white"
        } else if row["black_username"]
            .as_str()
            .unwrap_or("")
            .eq_ignore_ascii_case(username)
        {
            "black"
        } else {
            linked.push(false);
            continue;
        };
        linked.push(
            tx.execute(
                "INSERT OR IGNORE INTO account_games(account_id,game_id,color) VALUES(?,?,?)",
                rusqlite::params![account_id, id, color],
            )? > 0,
        );
    }
    Ok(linked)
}

pub struct AnalysisClaim<'a> {
    pub game_id: i64,
    pub job_id: i64,
    pub token: &'a str,
    pub uuid: &'a str,
    pub pgn: &'a str,
    pub cancel: &'a std::sync::atomic::AtomicBool,
}
pub fn save_analysis(
    db: &Database,
    claim: &AnalysisClaim<'_>,
    result: &Value,
    depth: i64,
    multipv: i64,
    engine: &str,
) -> Result<bool> {
    let id = claim.game_id;
    let moves = result["moves"]
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("Analysis omitted moves"))?;
    Ok(db.transaction_until(
        std::time::Instant::now()+std::time::Duration::from_secs(2),
        ||claim.cancel.load(std::sync::atomic::Ordering::SeqCst),
        |tx| {
        if claim.cancel.load(std::sync::atomic::Ordering::SeqCst)
            || !tx.query_row("SELECT EXISTS(SELECT 1 FROM games g JOIN jobs j ON j.game_id=g.id WHERE g.id=? AND g.uuid=? AND g.pgn=? AND j.id=? AND j.claim_token=? AND j.status='running')",rusqlite::params![id,claim.uuid,claim.pgn,claim.job_id,claim.token],|r|r.get::<_,bool>(0))?{return Ok(false);}
        tx.execute("DELETE FROM moves WHERE game_id=?",[id])?;
        for m in moves {
            if claim.cancel.load(std::sync::atomic::Ordering::SeqCst){return Ok(false);}
            let mut vals=vec![json!(id)];
            for k in ["ply","color","san","uci","fen_before"]{vals.push(m[k].clone());}
            vals.push(json!(m["fen_before"].as_str().unwrap_or("").split_whitespace().take(4).collect::<Vec<_>>().join(" ")));
            for k in ["eval_before","eval_after","win_before","win_after","ep_loss","cp_loss","accuracy","best_uci","best_san","classification","phase","clock","time_spent","is_book"]{vals.push(m[k].clone());}
            vals.push(json!(m.get("tags").cloned().unwrap_or(json!({})).to_string()));vals.push(m["explanation"].clone());
            vals.push(json!(m.get("lines").cloned().unwrap_or(json!([])).to_string()));vals.push(json!(m["reply"].to_string()));vals.push(m["endgame_type"].clone());
            tx.execute("INSERT INTO moves(game_id,ply,color,san,uci,fen_before,epd_before,eval_before,eval_after,win_before,win_after,ep_loss,cp_loss,accuracy,best_uci,best_san,classification,phase,clock,time_spent,is_book,tags,explanation,lines_json,reply_json,endgame_type) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",params_from_iter(sql_values(&vals)?))?;
        }
        let mut vals=vec![json!(id),json!(depth),json!(multipv),json!(engine),json!(ANALYSIS_VERSION),json!(now()),result["duration_ms"].clone()];
        for prefix in ["accuracy","acpl"]{for color in ["white","black"]{vals.push(result.get(format!("{prefix}_{color}")).cloned().unwrap_or_else(||result[prefix][color].clone()));}}
        for key in ["eco","name","family","variation"]{vals.push(result["opening"][key].clone());}
        vals.push(result["book_plies"].clone());for key in ["counts","key_moments","evals"]{vals.push(json!(result[key].to_string()));}
        tx.execute("INSERT INTO game_analysis(game_id,depth,multipv,engine,version,analyzed_at,duration_ms,accuracy_white,accuracy_black,acpl_white,acpl_black,opening_eco,opening_name,opening_family,opening_variation,book_plies,counts_json,key_moments_json,evals_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?) ON CONFLICT(game_id) DO UPDATE SET depth=excluded.depth,multipv=excluded.multipv,engine=excluded.engine,version=excluded.version,analyzed_at=excluded.analyzed_at,duration_ms=excluded.duration_ms,accuracy_white=excluded.accuracy_white,accuracy_black=excluded.accuracy_black,acpl_white=excluded.acpl_white,acpl_black=excluded.acpl_black,opening_eco=excluded.opening_eco,opening_name=excluded.opening_name,opening_family=excluded.opening_family,opening_variation=excluded.opening_variation,book_plies=excluded.book_plies,counts_json=excluded.counts_json,key_moments_json=excluded.key_moments_json,evals_json=excluded.evals_json,revision=game_analysis.revision+1",params_from_iter(sql_values(&vals)?))?;
        if !result["opening"].is_null(){tx.execute("UPDATE games SET opening_eco=?,opening_name=?,opening_family=?,opening_variation=? WHERE id=?",params_from_iter(sql_values(&[result["opening"]["eco"].clone(),result["opening"]["name"].clone(),result["opening"]["family"].clone(),result["opening"]["variation"].clone(),json!(id)])?))?;}
        tx.execute("UPDATE jobs SET status='done',finished_at=?,partial_json=NULL,error=NULL,progress=total WHERE id=? AND claim_token=?",rusqlite::params![now(),claim.job_id,claim.token])?;
        tx.execute(
            "INSERT INTO jobs(game_id,status,priority,depth,created_at,sort_key) SELECT j.game_id,'queued',j.followup_priority,j.followup_depth,?,COALESCE(g.end_time,0) FROM jobs j JOIN games g ON g.id=j.game_id WHERE j.id=? AND j.followup_depth>?",
            rusqlite::params![now(),claim.job_id,depth],
        )?;
        tx.execute("UPDATE jobs SET followup_priority=NULL,followup_depth=NULL WHERE id=? AND followup_depth IS NOT NULL",[claim.job_id])?;
        Ok(true)
    })?.unwrap_or(false))
}

pub fn account_public(db: &Database, account: &Value) -> Result<Value> {
    let stats = decode(&account["stats_json"], json!({}));
    let profile = decode(&account["profile_json"], json!({}));
    let mut ratings = json!({});
    if let Some(stats) = stats.as_object() {
        for (key, val) in stats {
            if let Some(tc) = key.strip_prefix("chess_")
                && val.is_object()
                && !tc.is_empty()
                && tc.len() <= 64
                && tc.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'_')
            {
                let rating = |v: &Value| v.as_i64().filter(|n| (0..=10000).contains(n));
                let record = val["record"].as_object().map(|record| {
                    let count = |key: &str| record.get(key).and_then(Value::as_u64);
                    json!({"win":count("win"),"loss":count("loss"),"draw":count("draw")})
                });
                ratings[tc] = json!({"rating":rating(&val["last"]["rating"]),"best":rating(&val["best"]["rating"]),"record":record});
            }
        }
    }
    let counts=db.one("SELECT COUNT(*) AS games,COUNT(ga.game_id) AS analyzed FROM account_games ag LEFT JOIN game_analysis ga ON ga.game_id=ag.game_id WHERE ag.account_id=?",&[account["id"].clone()])?.unwrap_or(json!({}));
    let queued=db.scalar("SELECT COUNT(*) FROM jobs j JOIN account_games ag ON ag.game_id=j.game_id WHERE ag.account_id=? AND j.status IN ('queued','running')",&[account["id"].clone()])?;
    Ok(
        json!({"id":account["id"],"username":account["username"],"display_name":account.get("display_name").filter(|v|!v.is_null()).unwrap_or(&account["username"]),"avatar":safe_url(account["avatar"].as_str()),"title":profile["title"].as_str().filter(|s|s.len()<=40),
        "country":profile["country"].as_str().and_then(|s|s.rsplit('/').next()).filter(|s|!s.is_empty()),
        "profile_url":safe_url(profile["url"].as_str()).unwrap_or_else(||format!("https://www.chess.com/member/{}",account["username"].as_str().unwrap_or(""))),
        "joined":profile["joined"].as_i64().filter(|t|(0..=253402300799).contains(t)),"ratings":ratings,"added_at":account["added_at"],"last_sync_at":account["last_sync_at"],"sync_status":account["sync_status"],"sync_error":account["sync_error"],
        "sync_enabled":account["sync_enabled"].as_i64()==Some(1),"auto_analyze":account["auto_analyze"].as_i64()==Some(1),"analyze_history":account["analyze_history"].as_i64()==Some(1),"import_since":account["import_since"],"games":counts["games"],"analyzed":counts["analyzed"],"queued":queued}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn save_analysis_bumps_revision_in_the_same_second() {
        let db = Database::open(":memory:").unwrap();
        db.execute(
            "INSERT INTO games(id,uuid,pgn,imported_at) VALUES(1,'revision','1. e4 e5 *',0)",
            &[],
        )
        .unwrap();
        db.execute("INSERT INTO jobs(id,game_id,status,created_at,claim_token) VALUES(1,1,'running',0,'claim')", &[]).unwrap();
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let claim = AnalysisClaim {
            game_id: 1,
            job_id: 1,
            token: "claim",
            uuid: "revision",
            pgn: "1. e4 e5 *",
            cancel: &cancel,
        };
        let result = json!({"moves":[],"opening":null,"duration_ms":0,"book_plies":0,"counts":{},"key_moments":[],"evals":[]});
        // now() has one-second resolution, so back-to-back saves normally share
        // analyzed_at; retry in the rare case a second boundary falls between them.
        let save = || {
            db.execute("UPDATE jobs SET status='running' WHERE id=1", &[])
                .unwrap();
            assert!(save_analysis(&db, &claim, &result, 6, 2, "fixture").unwrap());
            db.one("SELECT analyzed_at,revision FROM game_analysis", &[])
                .unwrap()
                .unwrap()
        };
        let (first, second) = (0..5)
            .map(|_| (save(), save()))
            .find(|(first, second)| first["analyzed_at"] == second["analyzed_at"])
            .expect("five save pairs never fell within one second");
        assert_eq!(
            second["revision"].as_i64().unwrap(),
            first["revision"].as_i64().unwrap() + 1
        );
    }
    #[test]
    fn pgn_time_classes() {
        let class = |tc: &str| {
            game_row_from_pgn(&format!(
                "[TimeControl \"{tc}\"]

1. e4 e5 *"
            ))
            .unwrap()["time_class"]
                .clone()
        };
        assert_eq!(class("1/86400"), json!("daily"));
        assert_eq!(class("180+2"), json!("blitz"));
        assert_eq!(class("40/7200:3600"), json!("rapid"));
        assert_eq!(class("-"), Value::Null);
        assert_eq!(class("?"), Value::Null);
    }
    #[test]
    fn eco_url_names_fall_back_to_their_family() {
        // An odds game: no indexed opening position, so only the ECOUrl names it.
        let row = |url: &str| {
            game_row_from_pgn(&format!(
                "[SetUp \"1\"]\n[FEN \"rnbqkbnr/pppppppp/8/8/8/8/PPPPPPPP/RNBQKBN1 w Qkq - 0 1\"]\n[ECOUrl \"https://www.chess.com/openings/{url}\"]\n\n1. e4 c5 *"
            ))
            .unwrap()
        };
        let sicilian = row("Sicilian-Defense-Old-Sicilian-Variation-3.d4");
        assert_eq!(sicilian["opening_family"], "Sicilian Defense");
        assert_eq!(sicilian["opening_variation"], "Old Sicilian Variation 3.d4");
        assert_eq!(
            sicilian["opening_name"],
            "Sicilian Defense Old Sicilian Variation 3.d4"
        );
        // The bundled spelling joins games the index named directly.
        assert_eq!(
            row("Caro-Kann-Defense-Advance-Variation")["opening_family"],
            "Caro-Kann Defense"
        );
        // Not in the index: up to the first family word.
        assert_eq!(
            row("Queens-Pawn-Opening-Zukertort-Variation-2...c6")["opening_family"],
            "Queens Pawn Opening"
        );
        assert_eq!(row("Some-Odd-Line")["opening_family"], "Some Odd Line");
        assert_eq!(row("")["opening_family"], Value::Null);
    }
    #[test]
    fn cancelled_result_save_does_not_wait_for_an_external_writer() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::{Duration, Instant};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result-busy.db");
        let db = Database::open(&path).unwrap();
        db.execute(
            "INSERT INTO games(id,uuid,pgn,imported_at) VALUES(1,'one','1. e4 e5 *',0)",
            &[],
        )
        .unwrap();
        db.execute("INSERT INTO jobs(id,game_id,status,created_at,claim_token) VALUES(1,1,'running',0,'claim')",&[]).unwrap();
        let blocker = rusqlite::Connection::open(&path).unwrap();
        blocker.execute_batch("BEGIN IMMEDIATE").unwrap();
        let cancel = AtomicBool::new(false);
        let claim = AnalysisClaim {
            game_id: 1,
            job_id: 1,
            token: "claim",
            uuid: "one",
            pgn: "1. e4 e5 *",
            cancel: &cancel,
        };
        std::thread::scope(|scope| {
            scope.spawn(|| {
                std::thread::sleep(Duration::from_millis(50));
                cancel.store(true, Ordering::SeqCst);
            });
            let start = Instant::now();
            assert!(!save_analysis(&db, &claim, &json!({"moves":[]}), 6, 2, "fixture").unwrap());
            assert!(start.elapsed() < Duration::from_secs(1));
        });
        blocker.execute_batch("ROLLBACK").unwrap();
        assert_eq!(
            db.scalar("SELECT status FROM jobs", &[]).unwrap(),
            "running"
        );
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM game_analysis", &[])
                .unwrap(),
            0
        );
        assert_eq!(db.scalar("SELECT COUNT(*) FROM moves", &[]).unwrap(), 0);
    }
    #[test]
    fn imported_games_without_end_time_end_at_their_start_time() {
        let row = game_row_from_pgn(
            "[UTCDate \"2026.11.01\"]
[UTCTime \"18:30:05\"]

1. e4 e5 1-0",
        )
        .unwrap();
        assert_eq!(row["start_time"], 1_793_557_805);
        assert_eq!(row["end_time"], row["start_time"]);
        let row = game_row_from_pgn(
            "[UTCDate \"2026.11.01\"]
[UTCTime \"18:30:05\"]
[EndTime \"18:40:00 GMT+0000\"]

1. e4 e5 1-0",
        )
        .unwrap();
        assert_eq!(row["end_time"], 1_793_558_400);
    }
    #[test]
    fn imported_links_cannot_use_executable_schemes() {
        let row = game_row_from_pgn("[Link \"javascript:alert(1)\"]\n\n1. e4 e5 *").unwrap();
        assert!(row["url"].is_null());
        assert!(safe_url(Some("https://www.chess.com/game/live/123")).is_some());
        assert!(safe_url(Some("https://name:password@example.test/game")).is_none());
    }
    #[test]
    fn malformed_archive_metadata_cannot_reach_sql_or_browser_as_objects() {
        let fixture: Value = serde_json::from_str(include_str!(
            "../../../backend/tests/fixtures/chesscom/2026-10.json"
        ))
        .unwrap();
        let good = fixture["games"][0].clone();
        assert!(game_row_from_chesscom(&good).unwrap().is_some());
        for (key, value) in [
            ("uuid", json!({"bad":true})),
            ("end_time", json!(-1)),
            ("rated", json!("yes")),
            ("time_control", json!([300])),
        ] {
            let mut game = good.clone();
            game[key] = value;
            assert!(game_row_from_chesscom(&game).is_err(), "{key}");
        }
        let mut game = good.clone();
        game["white"]["rating"] = json!({"value":1500});
        assert!(game_row_from_chesscom(&game).is_err());
        let mut game = good;
        game["accuracies"] = json!({"white":101});
        assert!(game_row_from_chesscom(&game).is_err());
    }
    #[test]
    fn public_profile_fields_are_safe_and_typed() {
        let db = Database::open(":memory:").unwrap();
        let profile =
            json!({"url":"javascript:alert(1)","title":{"html":"bad"},"joined":"yesterday"});
        let stats = json!({"chess_":{"last":{"rating":1400}},"chess_blitz":{"last":{"rating":{}},"best":{"rating":1800},"record":{"win":{},"loss":4,"draw":2}}});
        db.execute("INSERT INTO accounts(username,added_at,profile_json,stats_json) VALUES('fixture',0,?,?)",&[json!(profile.to_string()),json!(stats.to_string())]).unwrap();
        let account = db.one("SELECT * FROM accounts", &[]).unwrap().unwrap();
        let public = account_public(&db, &account).unwrap();
        assert_eq!(
            public["profile_url"],
            "https://www.chess.com/member/fixture"
        );
        assert!(public["title"].is_null());
        assert!(public["joined"].is_null());
        assert!(public["ratings"].get("").is_none());
        assert!(public["ratings"]["blitz"]["rating"].is_null());
        assert_eq!(public["ratings"]["blitz"]["best"], 1800);
    }
    #[test]
    fn stale_job_cannot_overwrite_reused_game_or_job_ids() {
        use std::sync::atomic::AtomicBool;
        let db = Database::open(":memory:").unwrap();
        db.execute(
            "INSERT INTO games(id,uuid,pgn,imported_at) VALUES(1,'old','1. e4 e5 *',0)",
            &[],
        )
        .unwrap();
        db.execute("INSERT INTO jobs(id,game_id,status,created_at,claim_token) VALUES(1,1,'running',0,'old-claim')",&[]).unwrap();
        let cancel = AtomicBool::new(false);
        let claim = AnalysisClaim {
            game_id: 1,
            job_id: 1,
            token: "old-claim",
            uuid: "old",
            pgn: "1. e4 e5 *",
            cancel: &cancel,
        };
        db.execute("DELETE FROM games WHERE id=1", &[]).unwrap();
        db.execute(
            "INSERT INTO games(id,uuid,pgn,imported_at) VALUES(1,'replacement','1. d4 d5 *',0)",
            &[],
        )
        .unwrap();
        db.execute("INSERT INTO jobs(id,game_id,status,created_at,claim_token) VALUES(1,1,'running',0,'new-claim')",&[]).unwrap();
        let stale = json!({"moves":[]});
        assert!(!save_analysis(&db, &claim, &stale, 6, 2, "test").unwrap());
        assert_eq!(
            db.scalar("SELECT COUNT(*) FROM game_analysis", &[])
                .unwrap(),
            0
        );
        assert_eq!(
            db.scalar("SELECT status FROM jobs WHERE id=1", &[])
                .unwrap(),
            "running"
        );
        // Reimporting identical bytes must still not let an older low-depth job
        // finish a newly queued job with a different claim.
        db.execute(
            "UPDATE games SET uuid='old',pgn='1. e4 e5 *' WHERE id=1",
            &[],
        )
        .unwrap();
        assert!(!save_analysis(&db, &claim, &stale, 6, 2, "test").unwrap());
        assert_eq!(
            db.scalar("SELECT claim_token FROM jobs WHERE id=1", &[])
                .unwrap(),
            "new-claim"
        );
        db.execute("UPDATE jobs SET claim_token='old-claim' WHERE id=1", &[])
            .unwrap();
        cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        assert!(!save_analysis(&db, &claim, &stale, 6, 2, "test").unwrap());
    }
}
