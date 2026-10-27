#![cfg(not(target_os = "none"))]

// Stable Rust cannot use associated scheduling constants in generic const
// expressions in a signature. These real API calls therefore need codegen.
#[test]
fn request_directions_are_rejected_during_codegen() {
    use std::{fs, path::Path, process::Command};
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch_root = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::create_dir_all(scratch_root).expect("create compiler scratch root");
    let scratch = tempfile::tempdir_in(scratch_root).expect("own compiler scratch");
    fs::write(scratch.path().join("Cargo.toml"), format!(
        "[package]\nname = \"bluetooth-request-directions\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[lib]\npath = \"case.rs\"\n[dependencies]\noer-esp32s31-bluetooth-driver = {{ path = {:?} }}\noer-espressif-interrupt-table = {{ path = {:?}, features = [\"host-signal\"] }}\n",
        manifest, manifest.join("../../../espressif/interrupt-table")
    )).expect("write isolated compiler manifest");
    for case in ["exchange", "attach"] {
        let source =
            fs::read_to_string(manifest.join(format!("tests/ui/codegen/{case}_direction.rs")))
                .expect("read paired case");
        for (mode, cfg, accepted) in [("pass", "any()", true), ("reject", "all()", false)] {
            fs::write(
                scratch.path().join("case.rs"),
                source.replace("ui_reject", cfg),
            )
            .expect("write paired case");
            for command in ["check", "build"] {
                let result = Command::new("cargo")
                    .args([command, "--offline", "--lib", "--manifest-path"])
                    .arg(scratch.path().join("Cargo.toml"))
                    .arg("--target-dir")
                    .arg(scratch.path().join("build"))
                    .output()
                    .expect("run pinned compiler");
                fs::write(
                    scratch.path().join(format!("{case}-{mode}-{command}.log")),
                    &result.stderr,
                )
                .expect("save compiler diagnostics");
                let stderr = String::from_utf8_lossy(&result.stderr);
                assert_eq!(
                    result.status.success(),
                    command == "check" || accepted,
                    "{case}/{mode}/{command}: {stderr}"
                );
                if command == "build" && !accepted {
                    assert!(stderr.contains("error[E0080]"), "{stderr}");
                    assert!(
                        stderr.contains("a request handler must be above its executor"),
                        "{stderr}"
                    );
                    assert!(stderr.contains(case), "{stderr}");
                }
            }
        }
    }
}
