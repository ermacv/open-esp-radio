#![cfg(not(target_os = "none"))]

// Inline const checks in a runtime constructor are evaluated when it is
// monomorphized. A metadata-only UI check cannot observe their rejection.
#[test]
fn doorbell_constructor_checks_direction_and_urgent_source_identity() {
    use std::{fs, path::Path, process::Command};

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch_root = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::create_dir_all(scratch_root).expect("create compiler scratch root");
    let scratch = tempfile::tempdir_in(scratch_root).expect("own compiler scratch");
    for (case, code, expected) in [
        (
            "doorbell_direction",
            "error[E0080]",
            "a request handler must be above its executor",
        ),
        (
            "doorbell_requester",
            "error[E0080]",
            "the doorbell requester must be an executor entry",
        ),
        (
            "doorbell_urgent_source",
            "error[E0308]",
            "UrgentSource<FromCpu1Irq>",
        ),
    ] {
        let source = fs::read_to_string(manifest.join(format!("tests/ui/codegen/{case}.rs")))
            .expect("read paired doorbell case");
        let source = source.replace(
            "include!(\"support/",
            &format!("include!(\"{}/tests/ui/support/", manifest.display()),
        );
        fs::write(scratch.path().join("Cargo.toml"), format!(
        "[package]\nname = \"doorbell-direction\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[lib]\npath = \"case.rs\"\n[dependencies]\noer-interrupt-table = {{ path = {:?} }}\n",
        manifest.join("../../../portable/interrupt-table")
    )).expect("write isolated compiler manifest");
        for (mode, cfg, accepted) in [("pass", "any()", true), ("reject", "all()", false)] {
            fs::write(
                scratch.path().join("case.rs"),
                source.replace("ui_reject", cfg),
            )
            .expect("write paired case");
            let result = Command::new("cargo")
                .args(["build", "--offline", "--lib", "--manifest-path"])
                .arg(scratch.path().join("Cargo.toml"))
                .arg("--target-dir")
                .arg(scratch.path().join("build"))
                .output()
                .expect("run pinned compiler");
            fs::write(scratch.path().join(format!("{mode}.log")), &result.stderr)
                .expect("save compiler diagnostics");
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert_eq!(result.status.success(), accepted, "{mode}: {stderr}");
            if !accepted {
                assert!(stderr.contains(code), "{stderr}");
                assert!(stderr.contains(expected), "{stderr}");
            }
        }
    }
}
