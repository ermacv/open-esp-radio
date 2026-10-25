//! The cores' owner starts hart 1 only with `cpu1-runs`, which does not
//! build: the staged runtime's critical-section is hart-local.

#[test]
fn starting_hart_1_refuses_the_hart_local_critical_section() {
    let build = tempfile::Builder::new()
        .prefix("cpu1-runs")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
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
