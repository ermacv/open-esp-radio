//! Build the runner and capture Cargo's compilation-unit record, which
//! [`oer_hil_run_bundle_format::observer::artifacts::apply`] binds to the executable's build identity.
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub struct Compilation {
    pub executable: PathBuf,
    pub artifacts: Vec<Value>,
    pub profile: String,
    _lock: std::fs::File,
}

/// Build the runner as a plain build of the root workspace does: into its
/// `target/`, with the registry's profile and the caller's environment, so
/// the observer reuses `target/debug` instead of compiling the runner's
/// closure a second time. The receipt lock stays under `target/hil`.
pub fn compile(root: &Path) -> Result<Compilation> {
    let receipts = root.join("target/hil");
    std::fs::create_dir_all(&receipts)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(receipts.join("observer-receipt.lock"))?;
    loop {
        oer_process::check_cancelled()?;
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => {
                oer_process::sleep(std::time::Duration::from_millis(20))?
            }
            Err(error) => return Err(error.into()),
        }
    }
    let registry: Value = serde_json::from_slice(&std::fs::read(
        root.join("hil/schema/observer-inputs.json"),
    )?)?;
    let profile = registry["build"]["profile"]
        .as_str()
        .ok_or("observer build profile missing")?;
    let profile = if profile == "debug" { "dev" } else { profile };
    let mut command = Command::new(oer_toolchain::cargo_program());
    command
        .current_dir(root)
        .env("CARGO_TARGET_DIR", root.join("target"))
        .args([
            "build",
            "--profile",
            profile,
            "--locked",
            "-p",
            "oer-hil-runner",
            "--bin",
            "oer-hil-runner",
            "--message-format=json-render-diagnostics",
        ])
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .stdout(Stdio::piped());
    let output = oer_process::owned::Child::spawn(&mut command)?.wait_with_output()?;
    if !output.status.success() {
        return Err("observer compilation failed".into());
    }
    let mut artifacts = Vec::new();
    let mut executable = None;
    for line in std::str::from_utf8(&output.stdout)?.lines() {
        let message: Value = serde_json::from_str(line)?;
        if message["reason"] == "compiler-artifact" || message["reason"] == "build-script-executed"
        {
            if message["target"]["name"] == "oer-hil-runner" {
                executable = message["executable"].as_str().map(PathBuf::from);
            }
            artifacts.push(message);
        }
    }
    Ok(Compilation {
        executable: executable.ok_or("Cargo did not report the observer executable")?,
        artifacts,
        profile: profile.to_owned(),
        _lock: lock,
    })
}
