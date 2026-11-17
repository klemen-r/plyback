//! Rebuild the preserved opening index using the Rust legal-move parser.
use plyback::pgn;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[test]
fn compiled_openings_match_tsv_sources() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("data");
    let compiled: Value =
        serde_json::from_str(&std::fs::read_to_string(root.join("openings.json")).unwrap())
            .unwrap();
    let mut named: BTreeMap<String, Value> = BTreeMap::new();
    let mut book = BTreeSet::new();
    let mut sources = 0;
    let mut files: Vec<_> = std::fs::read_dir(root.join("openings"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "tsv"))
        .collect();
    files.sort();
    assert!(!files.is_empty());
    for file in files {
        let text = std::fs::read_to_string(file).unwrap();
        let mut lines = text.lines();
        assert_eq!(lines.next(), Some("eco\tname\tpgn"));
        for line in lines {
            let fields: Vec<_> = line.splitn(3, '\t').collect();
            assert_eq!(fields.len(), 3);
            let game = pgn::parse(fields[2]).unwrap();
            for mv in &game.moves {
                book.insert(
                    mv.fen_after
                        .split_whitespace()
                        .take(4)
                        .collect::<Vec<_>>()
                        .join(" "),
                );
            }
            let key = pgn::epd(&pgn::board(&game.final_fen, false).unwrap());
            let ply = game.moves.len();
            if named
                .get(&key)
                .is_none_or(|old| ply < old[2].as_u64().unwrap() as usize)
            {
                named.insert(key, json!([fields[0], fields[1], ply]));
            }
            sources += 1;
        }
    }
    assert!(sources > 3000, "every source table must be read");
    assert_eq!(json!(named), compiled["named"]);
    assert_eq!(json!(book), compiled["book"]);
}
