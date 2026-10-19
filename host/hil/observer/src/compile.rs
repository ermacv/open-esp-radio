//! Build the runner with the registry's profile and find its executable in
//! Cargo's artifact messages.
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub struct Compilation {
    pub executable: PathBuf,
    _lock: std::fs::File,
}

/// Build the runner as a plain build of the root workspace does: into its
/// `target/`, with the registry's profile and the caller's environment, so
/// the observer reuses `target/debug` instead of compiling the runner's
/// closure a second time. The build lock stays under `target/hil`.
pub fn compile(root: &Path) -> Result<Compilation> {
    let locks = root.join("target/hil");
    std::fs::create_dir_all(&locks)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(locks.join("observer-build.lock"))?;
    loop {
        oer_os_process::check_cancelled()?;
        match lock.try_lock() {
            Ok(()) => break,
            Err(std::fs::TryLockError::WouldBlock) => {
                oer_os_process::sleep(std::time::Duration::from_millis(20))?
            }
            Err(error) => return Err(error.into()),
        }
    }
    let registry: Value = serde_json::from_slice(&std::fs::read(
        root.join("host/hil/schema/observer-inputs.json"),
    )?)?;
    let profile = registry["build"]["profile"]
        .as_str()
        .ok_or("observer build profile missing")?;
    let profile = if profile == "debug" { "dev" } else { profile };
    let mut command = Command::new(oer_os_toolchain::cargo_program());
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
    let output = oer_os_process::owned::Child::spawn(&mut command)?.wait_with_output()?;
    if !output.status.success() {
        return Err("observer compilation failed".into());
    }
    let mut executable = None;
    for line in std::str::from_utf8(&output.stdout)?.lines() {
        let message: Value = serde_json::from_str(line)?;
        if message["reason"] == "compiler-artifact" || message["reason"] == "build-script-executed"
        {
            if message["target"]["name"] == "oer-hil-runner" {
                executable = message["executable"].as_str().map(PathBuf::from);
            }
        }
    }
    Ok(Compilation {
        executable: executable.ok_or("Cargo did not report the observer executable")?,
        _lock: lock,
    })
}
