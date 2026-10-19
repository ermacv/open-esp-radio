#[test]
fn plan_and_catalog_commands_ignore_an_invalid_lab_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let invalid_lab = directory.path().join("invalid.toml");
    std::fs::write(&invalid_lab, b"this is not a lab configuration").unwrap();

    for arguments in [
        &["plan", "boot-smoke"][..],
        &["scenario", "list"][..],
        &["scenario", "validate", "boot-smoke"][..],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_oer-hil-runner"))
            .arg("--stand-file")
            .arg(&invalid_lab)
            .args(arguments)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{arguments:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout)
            .unwrap_or_else(|error| panic!("{arguments:?}: invalid JSON: {error}"));
    }
}
