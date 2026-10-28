#![cfg(not(target_os = "none"))]
#![forbid(unsafe_code)]

use std::{fs, path::Path, process::Command};

// Both the policy and the ABI are checked through an external expansion:
// callers can forbid unsafe, but a direct constructor still needs a lint waiver.
#[test]
fn safe_table_exports_keep_their_abi_and_only_the_macro_may_construct_tokens() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch_root = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::create_dir_all(scratch_root).unwrap();
    let scratch = tempfile::tempdir_in(scratch_root).unwrap();
    fs::write(
        scratch.path().join("Cargo.toml"),
        format!(
            "[package]\nname = \"interrupt-exports\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[lib]\npath = \"case.rs\"\n[dependencies]\noer-espressif-interrupt-table = {{ path = {manifest:?} }}\n"
        ),
    )
    .unwrap();
    let source = r#"#![forbid(unsafe_code)]
#![deny(clippy::disallowed_methods)]
oer_espressif_interrupt_table::__fake_parts!();
pub fn on_timer(_: &mut TimerIrq) {}
pub fn on_radio(_: &mut RadioIrq) {}
oer_espressif_interrupt_table::interrupt_table! {
    lines: FAKE_LINES,
    handler_attributes: [#[inline(never)]];
    contexts { executor: 2; }
    radio: RadioToken = RadioIrq => on_radio, line 17, context executor, hart 0, Executor;
    timer: TimerToken = TimerIrq => on_timer, line 18, context executor, hart 0,
        Exempt(ExecutorLevel, "export regression");
    #[cfg(any())]
    absent: AbsentToken = AbsentIrq => missing_handler, line 19, context executor, hart 0, Hardware;
}
"#;
    let case = scratch.path().join("case.rs");
    let ir = scratch.path().join("exports.ll");
    let emit_ir = format!("--emit=llvm-ir={}", ir.display());
    fs::write(&case, source).unwrap();
    let run = |name: &str, args: &[&str]| {
        let export_args = ["--", emit_ir.as_str()];
        let output = Command::new("cargo")
            .args(args)
            .args(["--offline", "--lib", "--manifest-path"])
            .arg(scratch.path().join("Cargo.toml"))
            .arg("--target-dir")
            .arg(scratch.path().join("build"))
            .env("CLIPPY_CONF_DIR", manifest.ancestors().nth(3).unwrap())
            .args(if name == "exports" {
                &export_args[..]
            } else {
                &["--", "-D", "warnings"][..]
            })
            .output()
            .unwrap();
        fs::write(scratch.path().join(format!("{name}.log")), &output.stderr).unwrap();
        output
    };
    let pass = run("macro", &["clippy"]);
    assert!(
        pass.status.success(),
        "{}",
        String::from_utf8_lossy(&pass.stderr)
    );

    fs::write(
        &case,
        format!("{source}\npub fn forge() {{ let _ = oer_espressif_interrupt_table::__private::Unique::new(); }}\n"),
    )
    .unwrap();
    let reject = run("direct", &["clippy"]);
    assert_eq!(reject.status.code(), Some(101));
    let diagnostics = String::from_utf8_lossy(&reject.stderr);
    assert!(
        diagnostics.contains("use of a disallowed method"),
        "{diagnostics}"
    );
    assert!(
        diagnostics.contains("interrupt entry tokens come from"),
        "{diagnostics}"
    );

    fs::write(&case, source).unwrap();
    let compiled = run("exports", &["rustc"]);
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    let ir = fs::read_to_string(ir).unwrap();
    for symbol in [
        "@__OER_INTERRUPT_TABLE =",
        "@__OER_INTERRUPT_EXEMPT___oer_interrupt_h0_l18 =",
        "@__oer_interrupt_h0_l17()",
        "@__oer_interrupt_h0_l18()",
        "export regression",
    ] {
        assert!(ir.contains(symbol), "missing {symbol}");
    }
    assert!(!ir.contains("@__oer_interrupt_h0_l19"));
}
