use super::*;
use shakmaty::Position;

fn draw_reason(parsed: &crate::pgn::ParsedGame) -> Result<&'static str> {
    let position = crate::pgn::board(&parsed.final_fen, parsed.chess960)?;
    if position.is_insufficient_material() {
        return Ok("insufficient");
    }
    if position.halfmoves() >= 100 {
        return Ok("50move");
    }
    // Repetition compares side to move, castling rights and legal en passant,
    // excluding the halfmove/fullmove counters. Parsed FENs use this same
    // normalized legal-en-passant representation throughout the main line.
    let final_key = crate::pgn::epd(&position);
    let occurrences = std::iter::once(&parsed.initial_fen)
        .chain(parsed.moves.iter().map(|m| &m.fen_after))
        .filter(|fen| {
            fen.split_whitespace()
                .take(4)
                .eq(final_key.split_whitespace())
        })
        .count();
    Ok(if occurrences >= 3 {
        "repetition"
    } else {
        "agreed"
    })
}

pub(super) fn game_row(game: &Value) -> Result<Option<Value>> {
    if !game.is_object() || !game["variant"].is_string() {
        return Err(api_error(502, "Invalid Lichess game object"));
    }
    let variant = game["variant"].as_str().unwrap_or("");
    // Chess960 is reviewed like the Chess.com imports; other variants are not chess.
    if !matches!(variant, "standard" | "fromPosition" | "chess960")
        || game["rules"].as_str().is_some_and(|rules| rules != "chess")
        || matches!(
            game["status"].as_str(),
            Some("created" | "started" | "aborted" | "noStart")
        )
    {
        return Ok(None);
    }
    let id = game["id"]
        .as_str()
        .filter(|s| (1..=32).contains(&s.len()) && s.bytes().all(|c| c.is_ascii_alphanumeric()))
        .ok_or_else(|| api_error(502, "Invalid Lichess game ID"))?;
    let raw = game["pgn"]
        .as_str()
        .ok_or_else(|| api_error(502, "Lichess game has no PGN"))?;
    let parsed = crate::pgn::parse(raw)?;
    if !matches!(parsed.rules.as_str(), "chess" | "chess960") {
        return Ok(None);
    }
    let status = game["status"]
        .as_str()
        .ok_or_else(|| api_error(502, "Lichess game has no status"))?;
    if !matches!(game["winner"].as_str(), Some("white" | "black")) && parsed.result == "*" {
        return Ok(None);
    }
    let result = match game["winner"].as_str() {
        Some("white") => "1-0",
        Some("black") => "0-1",
        _ if matches!(status, "draw" | "stalemate") => "1/2-1/2",
        _ => parsed.result.as_str(),
    };
    if result == "*" {
        return Ok(None);
    }
    let drawn = if result == "1/2-1/2" {
        Some(match status {
            "outoftime" => "timevsinsufficient",
            "stalemate" => "stalemate",
            "draw" => draw_reason(&parsed)?,
            _ => "agreed",
        })
    } else {
        None
    };
    // Rebuild the legal main line, retaining only clocks from comments. This
    // prevents even an unexpected evals-enabled PGN leaking platform analysis.
    let mut text = String::new();
    let time_control = if let (Some(initial), Some(increment)) = (
        game["clock"]["initial"].as_u64(),
        game["clock"]["increment"].as_u64(),
    ) {
        Some(format!("{initial}+{increment}"))
    } else {
        game["daysPerTurn"]
            .as_u64()
            .map(|d| format!("1/{}", d.saturating_mul(86400)))
    };
    for (key, value) in &parsed.headers {
        let lower = key.to_ascii_lowercase();
        if lower.contains("accuracy")
            || lower.contains("eval")
            || lower.contains("analysis")
            || (key == "TimeControl" && time_control.is_some())
            || matches!(
                key.as_str(),
                "PlybackClockSource" | "WhiteTimeControl" | "BlackTimeControl"
            )
        {
            continue;
        }
        let value = if key == "Result" { result } else { value };
        text.push_str(&format!(
            "[{key} \"{}\"]\n",
            value.replace('\\', "\\\\").replace('"', "\\\"")
        ));
    }
    if let Some(tc) = &time_control {
        text.push_str(&format!("[TimeControl \"{tc}\"]\n"));
    }
    if let (Some(initial), Some(_)) = (
        game["clock"]["initial"].as_u64(),
        game["clock"]["increment"].as_u64(),
    ) {
        let mut marked = false;
        for (color, tag) in [("white", "WhiteTimeControl"), ("black", "BlackTimeControl")] {
            if game["players"][color]["berserk"].as_bool() == Some(true) {
                if !marked {
                    text.push_str("[PlybackClockSource \"lichess\"]\n");
                    marked = true;
                }
                text.push_str(&format!("[{tag} \"{}+0\"]\n", initial as f64 / 2.0));
            }
        }
    }
    text.push('\n');
    let mut fullmove = parsed
        .initial_fen
        .split_whitespace()
        .last()
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(1);
    for (i, m) in parsed.moves.iter().enumerate() {
        if m.color == "w" {
            text.push_str(&format!("{fullmove}. "));
        } else if i == 0 {
            text.push_str(&format!("{fullmove}... "));
        }
        text.push_str(&m.san);
        let clock = game["clocks"]
            .get(i)
            .and_then(Value::as_u64)
            .map(|n| n as f64 / 100.)
            .or(m.clock);
        if let Some(clock) = clock.filter(|c| c.is_finite() && (0.0..=31_536_000.0).contains(c)) {
            let hours = (clock / 3600.) as u64;
            let mins = ((clock % 3600.) / 60.) as u64;
            text.push_str(&format!(
                " {{ [%clk {hours}:{mins:02}:{:05.2}] }}",
                clock % 60.
            ));
        }
        text.push(' ');
        if m.color == "b" {
            fullmove += 1;
        }
    }
    text.push_str(result);
    let mut row = store::game_row_from_pgn(&text)?;
    row["uuid"] = json!(format!("lichess:{id}"));
    row["url"] = json!(format!("{BASE}/{id}"));
    row["source"] = json!("lichess");
    row["rated"] = json!(i64::from(game["rated"].as_bool().unwrap_or(false)));
    row["termination"] = json!(status);
    row["result"] = json!(result);
    if row["opening_family"].is_null() {
        if let Some(name) = game["opening"]["name"]
            .as_str()
            .filter(|s| s.len() <= 512 && !s.contains(['\n', '\r', '\0']))
        {
            row["opening_name"] = json!(name);
            row["opening_family"] = json!(name.split(':').next().unwrap_or(name));
            row["opening_variation"] =
                json!(name.split_once(':').map(|(_, s)| s.trim()).unwrap_or(""));
        }
        if let Some(eco) = game["opening"]["eco"].as_str().filter(|s| {
            s.len() == 3
                && (b'A'..=b'E').contains(&s.as_bytes()[0])
                && s.as_bytes()[1..].iter().all(u8::is_ascii_digit)
        }) {
            row["opening_eco"] = json!(eco);
            row["eco"] = json!(eco);
        }
    }
    row["time_class"] = json!(match game["speed"].as_str().unwrap_or("") {
        "ultraBullet" | "bullet" => "bullet",
        "blitz" => "blitz",
        "rapid" | "classical" => "rapid",
        "correspondence" => "daily",
        _ => return Err(api_error(502, "Invalid Lichess speed")),
    });
    for (input, output) in [("createdAt", "start_time"), ("lastMoveAt", "end_time")] {
        let ms = game[input]
            .as_i64()
            .filter(|n| (0..=253402300799000).contains(n))
            .ok_or_else(|| api_error(502, format!("Invalid Lichess {input}")))?;
        // Shared game timestamps use whole Unix seconds, like Chess.com. The
        // export's millisecond creation cursor belongs to the account instead.
        row[output] = json!(ms / 1000);
    }
    if let Some(tc) = time_control {
        row["time_control"] = json!(tc);
    }
    for color in ["white", "black"] {
        let player = &game["players"][color];
        // The Lichess AI has no user, only {"aiLevel": N}; name it as Lichess does.
        let ai = player["aiLevel"]
            .as_u64()
            .map(|level| format!("Lichess AI level {level}"));
        let name = player["user"]["name"]
            .as_str()
            .or_else(|| player["user"]["id"].as_str())
            .or_else(|| player["name"].as_str())
            .or(ai.as_deref())
            .unwrap_or("Anonymous");
        if name.len() > 200 || name.contains(['\n', '\r', '\0']) {
            return Err(api_error(502, "Invalid Lichess player name"));
        }
        row[format!("{color}_username")] = json!(name);
        // The export gives the rating before the game; Chess.com games store the rating after it.
        row[format!("{color}_rating")] = player["rating"]
            .as_i64()
            .map(|n| n + player["ratingDiff"].as_i64().unwrap_or(0))
            .filter(|n| (0..=10000).contains(n))
            .map_or(Value::Null, |n| json!(n));
        let won = matches!((result, color), ("1-0", "white") | ("0-1", "black"));
        let reason = if won {
            "win"
        } else if let Some(reason) = drawn {
            reason
        } else {
            match status {
                "mate" => "checkmated",
                "resign" => "resigned",
                "outoftime" => "timeout",
                "timeout" => "abandoned",
                _ => "lose",
            }
        };
        row[format!("{color}_result")] = json!(reason);
    }
    Ok(Some(row))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(status: &str, result: &str, movetext: &str) -> Value {
        json!({
            "id":"resultfixture", "variant":"standard", "speed":"blitz", "rated":true,
            "createdAt":1770000000123i64, "lastMoveAt":1770000060456i64,
            "status":status,
            "players":{"white":{"user":{"name":"fixture"},"rating":1500},
                       "black":{"user":{"name":"opponent"},"rating":1510}},
            "pgn":format!("[White \"fixture\"]\n[Black \"opponent\"]\n\n{movetext} {result}")
        })
    }

    fn assert_draw(mut game: Value, code: &str) {
        game["winner"] = Value::Null;
        let row = game_row(&game).unwrap().unwrap();
        assert_eq!(row["result"], "1/2-1/2");
        assert_eq!(row["white_result"], code);
        assert_eq!(row["black_result"], code);
    }

    fn position_draw(status: &str, fen: &str) -> Value {
        let mut game = fixture(status, "1/2-1/2", "");
        game["variant"] = json!("fromPosition");
        game["pgn"] = json!(format!("[SetUp \"1\"]\n[FEN \"{fen}\"]\n\n1/2-1/2"));
        game
    }

    #[test]
    fn a_chess960_draw_keeps_its_castling_rights() {
        // The king and rooks are off the standard squares, so the final position is only legal
        // with Chess960 castling rules.
        let fen = "bbqnnrkr/pppppppp/8/8/8/8/PPPPPPPP/BBQNNRKR w HFhf - 0 1";
        let mut game = fixture("draw", "1/2-1/2", "1. e4 e5 2. Nd3 Nd6");
        game["variant"] = json!("chess960");
        game["pgn"] = json!(format!(
            "[Variant \"Chess960\"]
[SetUp \"1\"]
[FEN \"{fen}\"]

1. e4 e5 2. Nd3 Nd6 1/2-1/2"
        ));
        assert_draw(game.clone(), "agreed");
        assert_eq!(game_row(&game).unwrap().unwrap()["rules"], "chess960");
    }

    #[test]
    fn berserk_clocks_use_each_sides_start_and_increment() {
        for color in ["white", "black"] {
            let mut game = fixture("resign", "1-0", "1. e4 e5 2. Nf3 Nc6");
            game["clock"] = json!({"initial":300,"increment":5});
            game["players"][color]["berserk"] = json!(true);
            game["clocks"] = if color == "white" {
                json!([14900, 30400, 14800, 30200])
            } else {
                json!([30400, 14900, 30200, 14800])
            };
            let row = game_row(&game).unwrap().unwrap();
            // The shared time control still describes the rating pool/filter, not the handicap.
            assert_eq!(row["time_control"], "300+5");
            let parsed = crate::pgn::parse(row["pgn"].as_str().unwrap()).unwrap();
            let expected = if color == "white" {
                vec![Some(1.0), Some(1.0), Some(1.0), Some(7.0)]
            } else {
                vec![Some(1.0), Some(1.0), Some(7.0), Some(1.0)]
            };
            assert_eq!(crate::pgn::time_spent(&parsed), expected, "{color}");
        }
        let mut game = fixture("resign", "1-0", "1. e4 e5");
        game["clock"] = json!({"initial":61,"increment":2});
        game["players"]["white"]["berserk"] = json!(true);
        game["players"]["black"]["berserk"] = json!(true);
        game["clocks"] = json!([3000, 2950]);
        let row = game_row(&game).unwrap().unwrap();
        let parsed = crate::pgn::parse(row["pgn"].as_str().unwrap()).unwrap();
        assert_eq!(crate::pgn::time_spent(&parsed), vec![Some(0.5), Some(1.0)]);
    }

    #[test]
    fn ratings_are_stored_after_the_game() {
        let mut game = fixture("resign", "1-0", "1. e4");
        game["players"]["white"]["ratingDiff"] = json!(6);
        game["players"]["black"]["ratingDiff"] = json!(-6);
        let row = game_row(&game).unwrap().unwrap();
        assert_eq!(row["white_rating"], 1506);
        assert_eq!(row["black_rating"], 1504);
    }

    #[test]
    fn lichess_game_rules_arrays_do_not_change_the_variant() {
        for variant in ["standard", "fromPosition"] {
            for rules in [json!(["noAbort"]), json!([]), Value::Null, json!("chess")] {
                let mut game = fixture("resign", "1-0", "1. e4");
                game["variant"] = json!(variant);
                game["rules"] = rules;
                let row = game_row(&game).unwrap().unwrap();
                assert_eq!(row["rules"], "chess");
                assert_eq!(row["uuid"], "lichess:resultfixture");
            }
        }
        let mut game = fixture("resign", "1-0", "1. e4");
        game["rules"] = json!("atomic");
        assert!(game_row(&game).unwrap().is_none());
        game["rules"] = json!(["noAbort"]);
        game["variant"] = json!("atomic");
        assert!(game_row(&game).unwrap().is_none());
    }

    #[test]
    fn claimed_abandonment_and_clock_losses_use_distinct_chesscom_codes() {
        for (status, code) in [("timeout", "abandoned"), ("outoftime", "timeout")] {
            let mut game = fixture(status, "1-0", "1. e4");
            game["winner"] = json!("white");
            let row = game_row(&game).unwrap().unwrap();
            assert_eq!(row["white_result"], "win");
            assert_eq!(row["black_result"], code, "{status}");
        }
        // The PGN result remains authoritative when the optional winner is absent.
        let row = game_row(&fixture("resign", "1-0", "1. e4"))
            .unwrap()
            .unwrap();
        assert_eq!(row["white_result"], "win");
        assert_eq!(row["black_result"], "resigned");
    }

    #[test]
    fn the_lichess_ai_is_named_by_level() {
        let mut game = fixture("mate", "1-0", "1. e4");
        game["players"]["black"] = json!({"aiLevel": 3});
        let row = game_row(&game).unwrap().unwrap();
        assert_eq!(row["black_username"], "Lichess AI level 3");
        assert!(row["black_rating"].is_null());
        game["players"]["black"] = json!({});
        let row = game_row(&game).unwrap().unwrap();
        assert_eq!(row["black_username"], "Anonymous");
    }

    #[test]
    fn a_draw_on_time_is_time_vs_insufficient_for_both_players() {
        assert_draw(fixture("outoftime", "1/2-1/2", ""), "timevsinsufficient");
    }

    #[test]
    fn automatic_draw_codes_come_from_the_played_position_and_history() {
        assert_draw(
            position_draw("draw", "7k/8/8/8/8/8/8/K7 w - - 100 51"),
            "insufficient",
        );
        assert_draw(
            position_draw("draw", "7k/8/8/8/8/8/8/KR6 w - - 100 51"),
            "50move",
        );
        assert_draw(
            fixture(
                "draw",
                "1/2-1/2",
                "1. Nf3 Nf6 2. Ng1 Ng8 3. Nf3 Nf6 4. Ng1 Ng8",
            ),
            "repetition",
        );
        assert_draw(
            fixture("draw", "1/2-1/2", "1. Nf3 Nf6 2. Ng1 Ng8"),
            "agreed",
        );
        assert_draw(
            position_draw("stalemate", "7k/5K2/6Q1/8/8/8/8/8 b - - 0 1"),
            "stalemate",
        );
    }

    #[test]
    fn unresolved_game_is_not_imported_as_two_losses() {
        for status in ["unknownFinish", "draw"] {
            assert!(game_row(&fixture(status, "*", "1. e4")).unwrap().is_none());
        }
    }

    #[test]
    fn whole_second_timestamps_keep_a_long_followup_game_in_its_session() {
        let db = crate::db::Database::open(":memory:").unwrap();
        db.execute(
            "INSERT INTO accounts(id,username,platform,added_at) VALUES(1,'fixture','lichess',0)",
            &[],
        )
        .unwrap();
        let mut first = fixture("resign", "1-0", "1. e4");
        first["id"] = json!("sessionfirst");
        let mut second = fixture("resign", "1-0", "1. e4");
        second["id"] = json!("sessionsecond");
        // The second game starts ten minutes after the first ends, but lasts
        // an hour. Falling back to its end time incorrectly starts a session.
        second["createdAt"] = json!(1770000660789i64);
        second["lastMoveAt"] = json!(1770004260999i64);
        let rows = [first, second]
            .iter()
            .map(|game| game_row(game).unwrap().unwrap())
            .collect::<Vec<_>>();
        db.transaction(|tx| {
            let ids = store::upsert_games(tx, &rows)?;
            store::link_games(tx, 1, "fixture", &rows, &ids)
        })
        .unwrap();
        let insights = crate::insights::compute(&db, 1, &json!({})).unwrap();
        assert_eq!(insights["overview"]["games"], 2);
        assert_eq!(insights["sessions"]["sessions"], 1);
        assert_eq!(insights["sessions"]["after"]["win"]["games"], 1);
        assert_eq!(
            db.scalar(
                "SELECT COUNT(*) FROM games WHERE typeof(start_time)='integer' AND typeof(end_time)='integer'",
                &[],
            )
            .unwrap(),
            2
        );
        assert_eq!(rows[0]["start_time"], 1770000000i64);
        assert_eq!(rows[0]["end_time"], 1770000060i64);
        assert_eq!(rows[1]["start_time"], 1770000660i64);
        assert_eq!(rows[1]["end_time"], 1770004260i64);
    }
}
