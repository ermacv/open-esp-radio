#![cfg(not(target_os = "none"))]

#[test]
fn contexts_and_directions_are_checked_at_compile_time() {
    oer_test_ui_cases::run(env!("CARGO_MANIFEST_DIR"), env!("CARGO_TARGET_TMPDIR"), &[]);
}

// Static initializers exercise metadata-only checks above. Runtime calls
// must also reject these bindings when their inline const is monomorphized.
#[test]
fn executor_level_raisers_are_checked_during_codegen() {
    use std::{fs, path::Path, process::Command};

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch_root = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::create_dir_all(scratch_root).expect("create compiler scratch root");
    let scratch = tempfile::tempdir_in(scratch_root).expect("own compiler scratch");
    fs::write(
        scratch.path().join("Cargo.toml"),
        format!(
            "[package]\nname = \"signal-executor-level\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[lib]\npath = \"case.rs\"\n[dependencies]\noer-espressif-interrupt-table = {{ path = {manifest:?}, features = [\"host-signal\"] }}\n"
        ),
    )
    .expect("write isolated compiler manifest");
    for (case, expected) in [
        (
            "class",
            "a same-level signal handler must have class Exempt(ExecutorLevel)",
        ),
        (
            "executor",
            "a same-level signal handler must have class Exempt(ExecutorLevel)",
        ),
        (
            "waker",
            "a same-level signal handler must have class Exempt(ExecutorLevel)",
        ),
        (
            "level",
            "a same-level signal handler must be on its receiving executor's level",
        ),
        ("hart", "a signal and its raiser must share a hart"),
        ("handler", "a signal handler must be above its executor"),
    ] {
        let source =
            fs::read_to_string(manifest.join(format!("tests/ui/signal_executor_level_{case}.rs")))
                .expect("read paired signal case");
        let initializer = "const SIGNAL: IrqSignal<Deferred> = make();";
        assert!(
            source.contains(initializer),
            "case must exercise construction"
        );
        let source = source
            .replace(
                "include!(\"support/",
                &format!("include!(\"{}/tests/ui/support/", manifest.display()),
            )
            .replace(
                initializer,
                "pub fn runtime() -> IrqSignal<Deferred> { make() }",
            );
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
            fs::write(
                scratch.path().join(format!("{case}-{mode}.log")),
                &result.stderr,
            )
            .expect("save compiler diagnostics");
            let stderr = String::from_utf8_lossy(&result.stderr);
            assert_eq!(result.status.success(), accepted, "{case}/{mode}: {stderr}");
            if !accepted {
                assert!(stderr.contains("error[E0080]"), "{stderr}");
                assert!(stderr.contains(expected), "{stderr}");
            }
        }
    }
}
