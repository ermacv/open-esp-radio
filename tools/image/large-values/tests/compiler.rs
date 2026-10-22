//! Fixtures run only the pinned toolchain, in each test's own directory.
use std::{collections::HashSet, path::Path, process::Command};

use serde_json::Value;

const TARGET: &str = "riscv32imafc-unknown-none-elf";
const DRIVER: &str = env!("CARGO_BIN_EXE_oer-tool-image-large-values");

fn compile(directory: &Path, source: &str, name: &str, extra: &[&str]) -> Value {
    let source_path = directory.join(format!("{name}.rs"));
    let manifest = directory.join("Cargo.toml");
    std::fs::write(&manifest, "[package]\nname='fixture'\nversion='0.1.0'\n").unwrap();
    std::fs::write(&source_path, source).unwrap();
    let output = Command::new(DRIVER)
        .arg("rustc")
        .arg(&source_path)
        .args(["--crate-name", name, "--crate-type=lib", "--edition=2024"])
        .args(["--target", TARGET, "--emit=link", "--out-dir"])
        .arg(directory)
        .args([
            "-Copt-level=3",
            "-Cpanic=abort",
            "-Ccodegen-units=16",
            "-Alarge-assignments",
        ])
        .args(extra)
        .env("OER_LARGE_VALUES_TARGET", TARGET)
        .env("OER_LARGE_VALUES_LIMIT", "4096")
        .env("CARGO_MANIFEST_DIR", directory)
        .env(
            "OER_LARGE_VALUES_WORKSPACE",
            serde_json::to_string(&[manifest]).unwrap(),
        )
        .env("OER_LARGE_VALUES_DRIVER_ID", "fixture")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(
        &std::fs::read(directory.join(format!("{name}.large-values.json"))).unwrap(),
    )
    .unwrap()
}

#[test]
fn reports_concrete_values_and_call_operands_with_source_locations() {
    let directory = tempfile::tempdir().unwrap();
    let source = include_str!("fixtures/values.rs");
    let report = compile(directory.path(), source, "values", &[]);
    assert_eq!(report["errors"], serde_json::json!([]));
    let records = report["records"].as_array().unwrap();
    for (function, role) in [
        ("local", "local"),
        ("consume_copy", "argument"),
        ("consume_move", "argument"),
        ("direct_copy", "call-copy"),
        ("direct_move", "call-move"),
        ("indirect_copy", "call-copy"),
        ("indirect_move", "call-move"),
        ("return_value", "return"),
        ("call_destination", "local"),
    ] {
        assert!(
            records.iter().any(|record| {
                record["instance"]
                    .as_str()
                    .unwrap()
                    .ends_with(&format!("::{function}"))
                    && record["role"] == role
                    && record["bytes"] == 8192
            }),
            "missing {function}/{role}: {report:#}"
        );
    }
    assert!(records.iter().any(|record| {
        record["instance"]
            .as_str()
            .unwrap()
            .contains("generic::<values::CopyBig>")
            && record["bytes"] == 8192
    }));
    assert!(records.iter().any(|record| {
        record["ty"].as_str().unwrap().contains("future")
            && record["bytes"].as_u64().unwrap() > 8192
    }));
    for record in records {
        let name = record["instance"].as_str().unwrap();
        assert!(!name.ends_with("::static_reference") && !name.ends_with("::at_limit"));
        assert!(record["span"].as_str().unwrap().contains("values.rs:"));
    }
    let mut seen = HashSet::new();
    assert!(records.iter().all(|record| seen.insert(record.to_string())));
    let macro_line = source
        .lines()
        .position(|line| line.contains("large_local!();"))
        .unwrap()
        + 1;
    assert!(
        records
            .iter()
            .filter(|record| record["instance"]
                .as_str()
                .unwrap()
                .ends_with("::macro_local"))
            .all(|record| record["span"]
                .as_str()
                .unwrap()
                .contains(&format!("values.rs:{macro_line}:")))
    );
}

#[test]
fn sees_a_capped_foreign_generic_instantiated_by_a_workspace_crate() {
    let directory = tempfile::tempdir().unwrap();
    let source = "#![no_std]\n#[inline(never)] pub fn generic<T: Copy>(x: &T) { let y = *x; core::hint::black_box(&y); }";
    let report = compile(directory.path(), source, "foreign", &["--cap-lints=allow"]);
    assert!(report["records"].as_array().unwrap().is_empty());
    let external = format!(
        "foreign={}",
        directory.path().join("libforeign.rlib").display()
    );
    let report = compile(
        directory.path(),
        "#![no_std]\n#[inline(never)] pub fn caller(value: &[u8; 8192]) { foreign::generic(value); }",
        "consumer",
        &["--extern", &external],
    );
    assert!(
        report["records"].as_array().unwrap().iter().any(|record| {
            record["instance"] == "foreign::generic::<[u8; 8192]>" && record["bytes"] == 8192
        }),
        "{report:#}"
    );
}
