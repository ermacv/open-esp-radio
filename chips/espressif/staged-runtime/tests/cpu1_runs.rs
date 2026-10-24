//! An image whose hart 1 runs code does not build: the runtime's
//! critical-section is hart-local.

#[test]
fn a_running_second_hart_refuses_the_hart_local_critical_section() {
    let build = tempfile::Builder::new()
        .prefix("cpu1-runs")
        .tempdir()
        .expect("a build directory");
    let output = std::process::Command::new(env!("CARGO"))
        .args(["check", "--quiet", "--offline", "--locked", "--lib"])
        .args(["--features", "cpu1-runs", "--manifest-path"])
        .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml"))
        .arg("--target-dir")
        .arg(build.path())
        .output()
        .expect("the pinned cargo runs");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "{stderr}");
    assert!(
        stderr.contains("needs a cross-hart lock without spinning"),
        "{stderr}"
    );
}
