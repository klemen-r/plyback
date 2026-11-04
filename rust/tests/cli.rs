#[test]
fn occupied_port_fails_before_opening_or_migrating_data() {
    let directory = tempfile::tempdir().unwrap();
    let data = directory.path().join("untouched");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_plyback"))
        .arg("--data")
        .arg(&data)
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
