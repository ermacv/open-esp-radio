//! Offline fixture installation grammar and output through the actual
//! `cargo stand` binary.

use std::{fs, os::unix::fs::PermissionsExt as _, process::Command};

fn trap(directory: &std::path::Path, name: &str) -> std::path::PathBuf {
    let path = directory.join(name);
    fs::write(&path, "#!/bin/sh\nexit 97\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    path
}

fn dry_run(arguments: &[&str]) -> std::process::Output {
    let directory = tempfile::tempdir().unwrap();
    let cargo = trap(directory.path(), "cargo-must-not-run");
    let _sudo = trap(directory.path(), "sudo");
    let invalid_lab = directory.path().join("private-lab.toml");
    fs::write(&invalid_lab, "this is not TOML = [").unwrap();
    let mut command = Command::new(env!("CARGO_BIN_EXE_oer-stand"));
    command
        .args([
            "--root",
            concat!(env!("CARGO_MANIFEST_DIR"), "/../.."),
            "fixture",
        ])
        .args(arguments)
        .env("OER_STAND_FILE", &invalid_lab)
        .env("CARGO", cargo)
        .env(
            "PATH",
            format!("{}:/usr/bin:/bin", directory.path().display()),
        );
    command.output().unwrap()
}

#[test]
fn the_provider_plan_is_valid_json_without_build_sudo_or_lab_access() {
    let first = dry_run(&["install", "--provider", "linux-net", "--dry-run"]);
    assert!(
        first.status.success(),
        "{}",
        String::from_utf8_lossy(&first.stderr)
    );
    let plan: serde_json::Value = serde_json::from_slice(&first.stdout).unwrap();
    assert_eq!(plan["provider"], "linux-net");
    assert_eq!(plan["automatic_hardware_checks"], false);
    let second = dry_run(&["install", "--provider", "linux-net", "--dry-run"]);
    assert_eq!(first.stdout, second.stdout);
    // The Bluetooth provider went with its family.
    assert!(
        !dry_run(&["install", "--provider", "linux-bluetooth", "--dry-run"])
            .status
            .success()
    );
}

#[test]
fn removed_alias_is_rejected_before_any_effect() {
    let canonical = dry_run(&["install", "--provider", "linux-net", "--dry-run"]);
    let alias = dry_run(&["install-host", "--dry-run"]);
    assert!(canonical.status.success());
    assert_eq!(alias.status.code(), Some(2));
    assert!(alias.stdout.is_empty());
}

#[test]
fn missing_or_invalid_provider_fails_in_cli_before_any_effect() {
    for arguments in [
        vec!["install", "--dry-run"],
        vec!["install", "--provider", "all", "--dry-run"],
    ] {
        let output = dry_run(&arguments);
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
}
