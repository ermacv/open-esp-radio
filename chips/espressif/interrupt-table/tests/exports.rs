#![cfg(not(target_os = "none"))]
#![forbid(unsafe_code)]

use std::{fs, path::Path, process::Command};

// Both the policy and the ABI are checked through an external expansion:
// callers can forbid unsafe, but a direct constructor still needs a lint waiver.
#[test]
fn safe_table_exports_resolve_transitive_paths_and_restrict_token_construction() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch_root = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::create_dir_all(scratch_root).unwrap();
    let scratch = tempfile::tempdir_in(scratch_root).unwrap();
    let memory = manifest
        .join("../../../portable/memory")
        .canonicalize()
        .unwrap();
    // Macro expansions must resolve oer-riscv through interrupt-table, without
    // requiring the consumer to declare that transitive dependency.
    fs::write(
        scratch.path().join("Cargo.toml"),
        format!(
            "[package]\nname = \"interrupt-exports\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[lib]\npath = \"case.rs\"\n[dependencies]\noer-espressif-interrupt-table = {{ path = {manifest:?} }}\noer-memory = {{ path = {memory:?} }}\n"
        ),
    )
    .unwrap();
    // Keep every dependency resolution from the pinned workspace lock.
    // Only the isolated root package itself is added to the copied lock.
    let mut lock = fs::read_to_string(manifest.join("../../../Cargo.lock")).unwrap();
    lock.push_str(
        r#"
[[package]]
name = "interrupt-exports"
version = "0.0.0"
dependencies = ["oer-espressif-interrupt-table", "oer-memory"]
"#,
    );
    fs::write(scratch.path().join("Cargo.lock"), &lock).unwrap();
    // Cargo prunes workspace-only packages for the isolated root. Verify
    // that this normalization preserves every resolved dependency identity,
    // then forbid any further lock changes in every compiler invocation.
    let normalized = Command::new("cargo")
        .args([
            "metadata",
            "--offline",
            "--format-version=1",
            "--manifest-path",
        ])
        .arg(scratch.path().join("Cargo.toml"))
        .output()
        .unwrap();
    assert!(
        normalized.status.success(),
        "{}",
        String::from_utf8_lossy(&normalized.stderr)
    );
    let identities = |text: &str| -> Vec<String> {
        text.split("\n[[package]]\n")
            .skip(1)
            .map(|package| {
                package
                    .lines()
                    .filter(|line| {
                        ["name =", "version =", "source =", "checksum ="]
                            .iter()
                            .any(|key| line.starts_with(key))
                    })
                    .collect::<Vec<_>>()
                    .join("\n")
            })
            .collect()
    };
    let pinned = identities(&lock);
    let normalized = fs::read_to_string(scratch.path().join("Cargo.lock")).unwrap();
    for identity in identities(&normalized) {
        assert!(
            pinned.contains(&identity),
            "dependency changed from workspace lock: {identity}"
        );
    }
    let source = r#"#![forbid(unsafe_code)]
#![deny(clippy::disallowed_methods)]
oer_espressif_interrupt_table::__fake_parts!();
oer_espressif_interrupt_table::__fake_controller!();
pub fn on_timer(_: &mut TimerIrq) {}
oer_espressif_interrupt_table::interrupt_table! {
    lines: FAKE_LINES,
    handler_attributes: [#[inline(never)]];
    contexts { executor: 2; }
    radio: RadioToken = RadioIrq => OnExecutor, line 17, context executor, hart 0, Executor;
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
            .args(["--offline", "--locked", "--lib", "--manifest-path"])
            .arg(scratch.path().join("Cargo.toml"))
            .arg("--target-dir")
            .arg(scratch.path().join("build"))
            .env("CLIPPY_CONF_DIR", manifest.ancestors().nth(3).unwrap())
            .args(if args == ["rustc"] {
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

    fs::write(&case, format!("{source}\npub fn proof() {{ let _ = oer_espressif_interrupt_table::__private::EntryProof::<TimerToken>::new(TimerToken::PROOF.handler(), <TimerToken as oer_espressif_interrupt_table::Entry>::cell()); }}\nuse oer_espressif_interrupt_table::Entry;" )).unwrap();
    let reject = run("direct-proof", &["clippy"]);
    assert_eq!(reject.status.code(), Some(101));
    assert!(
        String::from_utf8_lossy(&reject.stderr)
            .contains("implementations and handler/cell proofs come from")
    );

    for (name, body, expected) in [
        (
            "wrong-signature",
            "fn wrong() -> u8 { 0 } oer_memory::link_fn! { define oer_memory::TraceClockSlot => fn __oer_trace_now_micros() -> u8 = wrong; }",
            "mismatched types",
        ),
        (
            "wrong-kind",
            r#"fn clock() -> u64 { 0 } oer_memory::link_fn! { @static oer_memory::TraceClockSlot, "__oer_trace_now_micros" => static BAD: fn() -> u64 = clock; }"#,
            "a function slot cannot define a static",
        ),
        (
            "wrong-name",
            "fn clock() -> u64 { 0 } oer_memory::link_fn! { define oer_memory::TraceClockSlot => fn memcpy() -> u64 = clock; }",
            "definition names another slot",
        ),
        (
            "wrong-abi",
            r#"oer_memory::link_fn! { @function oer_memory::TraceClockSlot, "__oer_trace_now_micros" => extern "C" fn bad() -> u64 { 0 } }"#,
            "mismatched types",
        ),
        (
            "free-name",
            r#"oer_memory::link_fn! { export "memcpy" => static BAD: u8 = 0; }"#,
            "no rules expected",
        ),
        (
            "missing-proof",
            r#"struct Forged;
impl oer_espressif_interrupt_table::Entry for Forged {
    type Part = TimerIrq;
    const LINE: u8 = 18;
    const LEVEL: u8 = 2;
    const HART: u8 = 0;
    const CLASS: oer_espressif_interrupt_table::Class = oer_espressif_interrupt_table::Class::Hardware;
    fn storage() -> &'static oer_espressif_interrupt_table::__private::StaticCell<TimerIrq> {
        <TimerToken as oer_espressif_interrupt_table::Entry>::storage()
    }
    fn cell() -> &'static oer_memory::HandlerCell<TimerIrq> {
        <TimerToken as oer_espressif_interrupt_table::Entry>::cell()
    }
}"#,
            "not all trait items implemented",
        ),
        (
            "foreign-proof",
            "const BAD: oer_espressif_interrupt_table::__private::EntryProof<RadioToken> = <TimerToken as oer_espressif_interrupt_table::Entry>::PROOF;",
            "mismatched types",
        ),
    ] {
        fs::write(&case, format!("{source}\n{body}\n")).unwrap();
        let reject = run(name, &["rustc"]);
        let stderr = String::from_utf8_lossy(&reject.stderr);
        assert_eq!(reject.status.code(), Some(101), "{name}: {stderr}");
        assert!(stderr.contains(expected), "{name}: {stderr}");
    }

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
