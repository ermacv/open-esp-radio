#![cfg(not(target_os = "none"))]

#[test]
fn executor_startup_rejects_a_foreign_harts_route_at_monomorphization() {
    use std::{fs, path::Path, process::Command};

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch_root = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::create_dir_all(scratch_root).expect("compiler scratch root");
    let scratch = tempfile::tempdir_in(scratch_root).expect("own compiler scratch");
    let source = fs::read_to_string(manifest.join("tests/ui/codegen/executor_hart.rs"))
        .expect("paired executor hart case")
        .replace(
            "__HART_BINDING__",
            &manifest.join("src/hart_binding.rs").to_string_lossy(),
        );
    fs::write(scratch.path().join("Cargo.toml"), format!(
        "[package]\nname = \"executor-hart\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[lib]\npath = \"case.rs\"\n[dependencies]\noer-espressif-interrupt-table = {{ path = {:?} }}\n",
        manifest.join("../../../chips/espressif/interrupt-table")
    )).expect("isolated compiler manifest");
    for (mode, cfg, accepted) in [("pass", "any()", true), ("reject", "all()", false)] {
        fs::write(
            scratch.path().join("case.rs"),
            source.replace("ui_reject", cfg),
        )
        .expect("paired case");
        let result = Command::new("cargo")
            .args(["build", "--offline", "--lib", "--manifest-path"])
            .arg(scratch.path().join("Cargo.toml"))
            .arg("--target-dir")
            .arg(scratch.path().join("build"))
            .output()
            .expect("pinned compiler");
        fs::write(scratch.path().join(format!("{mode}.log")), &result.stderr)
            .expect("compiler diagnostics");
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert_eq!(result.status.success(), accepted, "{mode}: {stderr}");
        if !accepted {
            assert!(stderr.contains("error[E0080]"), "{stderr}");
            assert!(
                stderr.contains("executor route is on this hart"),
                "{stderr}"
            );
        }
    }
}
