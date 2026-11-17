//! Compare SQLite page-cache choices on an explicitly supplied disposable copy.
//! Opens read-only; never discovers or opens the application's default database.
use anyhow::Result;
use rusqlite::{Connection, OpenFlags};
use std::{hint::black_box, time::Instant};

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .expect("pass a disposable database copy");
    let queries = [
        (
            "games_count_page",
            "SELECT g.id,g.url,g.time_class,g.white_username,g.black_username,ga.counts_json FROM games g JOIN account_games ag ON ag.game_id=g.id LEFT JOIN game_analysis ga ON ga.game_id=g.id WHERE ag.account_id=1 ORDER BY g.end_time DESC,g.id DESC LIMIT 30",
        ),
        (
            "practice_rows",
            "SELECT m.fen_before,m.lines_json,m.explanation FROM account_games ag JOIN games g ON g.id=ag.game_id JOIN moves m ON m.game_id=ag.game_id AND m.color=substr(ag.color,1,1) WHERE ag.account_id=1 AND m.classification IN ('blunder','mistake','miss') AND m.best_uci IS NOT NULL AND m.best_uci<>m.uci AND m.lines_json IS NOT NULL AND g.rules='chess' ORDER BY g.end_time DESC,m.ply",
        ),
        (
            "repertoire_rows",
            "SELECT m.ply,m.san,m.classification,m.ep_loss FROM account_games ag JOIN games g ON g.id=ag.game_id JOIN moves m ON m.game_id=g.id WHERE ag.account_id=1 AND ag.color='white' AND g.rules='chess' AND m.ply<=16 ORDER BY g.id,m.ply",
        ),
        (
            "insight_rows",
            "SELECT game_id,ply,color,san,classification,phase,ep_loss,cp_loss,accuracy,win_before,win_after,clock,time_spent,is_book,tags,epd_before,best_san,endgame_type FROM moves WHERE game_id IN (SELECT game_id FROM account_games WHERE account_id=1) ORDER BY game_id,ply",
        ),
    ];
    for (name, sql) in queries {
        for (variant, pragmas) in [
            ("default", "PRAGMA cache_size=-2000; PRAGMA mmap_size=0"),
            ("cache64", "PRAGMA cache_size=-65536; PRAGMA mmap_size=0"),
            (
                "mmap256",
                "PRAGMA cache_size=-2000; PRAGMA mmap_size=268435456",
            ),
            (
                "both",
                "PRAGMA cache_size=-65536; PRAGMA mmap_size=268435456",
            ),
        ] {
            let c = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
            c.execute_batch(pragmas)?;
            let mut times = Vec::new();
            for _ in 0..6 {
                let began = Instant::now();
                if name == "games_count_page" {
                    black_box(c.query_row("SELECT COUNT(*) FROM games g JOIN account_games ag ON ag.game_id=g.id LEFT JOIN game_analysis ga ON ga.game_id=g.id WHERE ag.account_id=1", [], |r| r.get::<_, i64>(0))?);
                }
                let mut stmt = c.prepare_cached(sql)?;
                let cols = stmt.column_count();
                let mut rows = stmt.query([])?;
                while let Some(row) = rows.next()? {
                    for col in 0..cols {
                        black_box(row.get_ref(col)?);
                    }
                }
                times.push(began.elapsed().as_secs_f64() * 1000.0);
            }
            let cold = times[0];
            times[1..].sort_by(f64::total_cmp);
            println!("{name},{variant},{cold:.3},{:.3},{:.3}", times[3], times[5]);
        }
    }
    Ok(())
}
