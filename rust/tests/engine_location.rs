//! Stockfish is found in the configured data directory, not the build's default one.
//! Kept in its own test binary: it changes process environment and a process-wide setting.
use plyback::engine::{locate_stockfish, set_data_dir};

#[test]
fn stockfish_is_found_in_the_configured_data_directory() {
    // CI exports an explicit engine path, which would take precedence over the data directory.
    // SAFETY: this binary runs a single test, so no other thread reads the environment.
    unsafe {
        std::env::remove_var("PLYBACK_STOCKFISH");
        std::env::remove_var("STOCKFISH_PATH");
        std::env::remove_var("PLYBACK_DATA");
        std::env::remove_var("CHESS_INSIGHT_DATA");
    }
    let data = tempfile::tempdir().unwrap();
    let engine = data.path().join("engine");
    std::fs::create_dir(&engine).unwrap();
    let binary = engine.join(if cfg!(windows) {
        "stockfish.exe"
    } else {
        "stockfish"
    });
    std::fs::write(&binary, b"not a real engine").unwrap();

    set_data_dir(data.path().to_owned());
    assert_eq!(
        locate_stockfish().unwrap(),
        binary.canonicalize().unwrap(),
        "the engine in --data's engine folder should be used"
    );
}
