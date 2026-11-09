fn built_frontend(directory: &std::path::Path) -> std::path::PathBuf {
    let frontend = directory.join("frontend");
    std::fs::create_dir_all(&frontend).unwrap();
    std::fs::write(frontend.join("index.html"), "<!doctype html>").unwrap();
    frontend
}

#[test]
fn occupied_port_fails_before_opening_or_migrating_data() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path().join("untouched");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_plyback"))
        .arg("--data")
        .arg(&data)
        .arg("--frontend")
        .arg(built_frontend(directory.path()))
        .args([
            "--host",
            "127.0.0.1",
            "--port",
            &listener.local_addr().unwrap().port().to_string(),
            "--no-background",
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        !data.exists(),
        "failed startup must leave the data directory untouched"
    );
}

#[test]
fn serve_without_a_built_interface_fails_and_names_the_directory() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path().join("untouched");
    let frontend = directory.path().join("dist");
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_plyback"))
        .arg("--data")
        .arg(&data)
        .arg("--frontend")
        .arg(&frontend)
        .args(["--host", "127.0.0.1", "--port", "0", "--no-background"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let error = String::from_utf8_lossy(&output.stderr);
    assert!(error.contains(&frontend.display().to_string()), "{error}");
    assert!(error.contains("PLYBACK_FRONTEND"), "{error}");
    assert!(!data.exists(), "a refused start must not create data");
}
