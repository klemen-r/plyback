// Standalone cross-platform UCI failure fixture compiled by engine_transport.rs.
use std::io::{BufRead, Write};
fn main() {
    let executable = std::env::current_exe().unwrap();
    let mode = executable.file_stem().unwrap().to_string_lossy();
    let mut log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(executable.with_extension("log"))
        .unwrap();
    writeln!(log, "start {}", std::process::id()).unwrap();
    let mut multipv = 1;
    for command in std::io::stdin().lock().lines() {
        let Ok(command) = command else {
            break;
        };
        writeln!(log, "{command}").unwrap();
        log.flush().unwrap();
        if command == "uci" {
            println!(
                "id name FakeFish\noption name Threads type spin default 1 min 1 max 128\noption name Hash type spin default 16 min 16 max 4096\noption name MultiPV type spin default 1 min 1 max 5\noption name UCI_Chess960 type check default false\nuciok"
            );
        } else if command == "isready" {
            println!("readyok");
        } else if command.starts_with("setoption name MultiPV value ") {
            multipv = command
                .split_whitespace()
                .last()
                .unwrap()
                .parse::<usize>()
                .unwrap();
        } else if command.starts_with("go ") {
            if mode.contains("exit") {
                std::process::exit(3);
            }
            if mode.contains("hang") || mode.contains("stubborn") {
                continue;
            }
            let roots = if let Some((_, roots)) = command.split_once(" searchmoves ") {
                roots.split_whitespace().collect::<Vec<_>>()
            } else {
                vec!["e2e4", "d2d4", "g1f3", "c2c4", "b1c3"]
            };
            for (rank, mv) in roots.iter().take(multipv).enumerate() {
                println!(
                    "info depth 8 multipv {} score cp {} nodes 100 pv {mv}",
                    rank + 1,
                    20 - rank as i32
                );
            }
            if mode.contains("partial") {
                println!(
                    "info depth 9 multipv 1 score cp 999 lowerbound pv {}",
                    roots[0]
                );
                println!("info depth 9 multipv 1 score cp 50 pv {}", roots[0]);
            }
            println!("bestmove {}", roots[0]);
        } else if command == "stop" && !mode.contains("stubborn") {
            println!("bestmove e2e4");
        } else if command == "quit" {
            break;
        }
        std::io::stdout().flush().unwrap();
    }
}
