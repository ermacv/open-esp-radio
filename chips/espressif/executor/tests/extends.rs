#![cfg(not(target_os = "none"))]

// `Extends` is a safe trait any crate may implement; the rights it converts
// check their executor where they convert, at monomorphization, which a
// metadata-only UI check cannot observe.
#[test]
fn a_right_converts_only_to_a_set_of_its_own_executor() {
    use std::{fs, path::Path, process::Command};

    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch_root = Path::new(env!("CARGO_TARGET_TMPDIR"));
    fs::create_dir_all(scratch_root).expect("compiler scratch root");
    let scratch = tempfile::tempdir_in(scratch_root).expect("own compiler scratch");
    let source = fs::read_to_string(manifest.join("tests/ui/codegen/extends_executor.rs"))
        .expect("paired extends case")
        .replace("__SRC__", &manifest.join("src").to_string_lossy());
    fs::write(scratch.path().join("Cargo.toml"), format!(
        "[package]\nname = \"extends-executor\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[lib]\npath = \"case.rs\"\n[lints.rust]\nunexpected_cfgs = \"allow\"\n[features]\ndefault = [\"chip\"]\nchip = []\n[dependencies]\noer-espressif-interrupt-table = {{ path = {:?} }}\noer-espressif-platform-port = {{ path = {:?} }}\noer-riscv = {{ path = {:?} }}\n",
        manifest.join("../../../chips/espressif/interrupt-table"),
        manifest.join("../../../chips/espressif/platform-port"),
        manifest.join("../../../portable/riscv"),
    ))
    .expect("isolated compiler manifest");
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
                stderr.contains("a set extends only a set of its own executor"),
                "{stderr}"
            );
        }
    }
}
