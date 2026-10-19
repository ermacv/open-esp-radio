//! Chip ownership twins checked as metadata, so a rejected borrow cannot
//! pass merely because a no_std executable would fail to link.
use std::{collections::BTreeMap, path::Path, process::Command};

/// Build `packages` with their chip features, then check each source's pass
/// and reject twins with rustc metadata only. Mark every expected rejection
/// with the same `ui-error` markers as the host cases.
///
/// # Panics
/// On a failed dependency build, a rejected pass twin, or an error that does
/// not match a reject twin's expected code and primary source line.
pub fn run_chip(
    root: &Path,
    scratch: &Path,
    target: &str,
    packages: &[(&str, &str)],
    sources: &[&str],
) {
    std::fs::create_dir_all(scratch).expect("create the chip UI scratch directory");
    let mut cargo = Command::new("cargo");
    cargo.current_dir(root).args([
        "check",
        "--lib",
        "--target",
        target,
        "--message-format=json",
    ]);
    for (package, feature) in packages {
        cargo.args(["-p", package, "--features", &format!("{package}/{feature}")]);
    }
    let built = cargo.output().expect("build chip ownership dependencies");
    std::fs::write(scratch.join("cargo.log"), &built.stderr).expect("save chip build diagnostics");
    assert!(
        built.status.success(),
        "chip UI dependencies: {}",
        String::from_utf8_lossy(&built.stderr)
    );
    let mut artifacts = BTreeMap::new();
    for line in built.stdout.split(|byte| *byte == b'\n') {
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(line) else {
            continue;
        };
        if value["reason"] != "compiler-artifact" {
            continue;
        }
        let Some(name) = value["target"]["name"].as_str() else {
            continue;
        };
        if let Some(files) = value["filenames"].as_array() {
            for file in files.iter().filter_map(serde_json::Value::as_str) {
                let path = Path::new(file);
                if path
                    .extension()
                    .is_some_and(|extension| extension == "rmeta")
                    && path.components().any(|part| part.as_os_str() == target)
                {
                    artifacts.insert(name.replace('-', "_"), path.to_owned());
                }
            }
        }
    }
    assert!(
        !artifacts.is_empty(),
        "cargo emitted the chip library metadata"
    );
    for source in sources {
        let source_path = root.join(source);
        let original = std::fs::read_to_string(&source_path).expect("read chip ownership twin");
        let expected = super::markers(&original).expect("chip twins mark every expected error");
        assert!(original.contains(super::CFG), "chip twins use ui_reject");
        let stem = source_path.file_stem().expect("a case file has a name");
        for (mode, cfg) in [("pass", "any()"), ("reject", "all()")] {
            let directory = scratch.join(mode);
            std::fs::create_dir_all(&directory).expect("create chip twin output");
            let path = directory.join(stem).with_extension("rs");
            std::fs::write(&path, original.replace(super::CFG, cfg))
                .expect("write chip ownership twin");
            let mut rustc = Command::new("rustc");
            rustc
                .current_dir(root)
                .args([
                    "--edition=2024",
                    "--crate-type=lib",
                    "--emit=metadata",
                    "--target",
                    target,
                ])
                .arg("--out-dir")
                .arg(&directory)
                .arg(&path);
            // Chip metadata also references proc macros built for the host.
            rustc.arg("-L").arg(format!(
                "dependency={}",
                root.join("target/debug/deps").display()
            ));
            for (name, artifact) in &artifacts {
                rustc
                    .arg("--extern")
                    .arg(format!("{name}={}", artifact.display()));
                rustc.arg("-L").arg(format!(
                    "dependency={}",
                    artifact
                        .parent()
                        .expect("metadata has a directory")
                        .display()
                ));
            }
            std::fs::write(path.with_extension("command"), format!("{rustc:?}"))
                .expect("save chip compiler invocation");
            let output = rustc.output().expect("type-check a chip ownership twin");
            let diagnostics = String::from_utf8_lossy(&output.stderr);
            std::fs::write(path.with_extension("log"), output.stderr.as_slice())
                .expect("save chip ownership diagnostics");
            if mode == "pass" {
                assert!(
                    output.status.success(),
                    "{} pass twin: {diagnostics}",
                    source_path.display()
                );
            } else {
                assert!(
                    !output.status.success(),
                    "{} reject twin compiled",
                    source_path.display()
                );
                let reported = super::reported_errors(&diagnostics, &path.to_string_lossy());
                assert_eq!(
                    reported.len(),
                    expected.len(),
                    "{} unexpected errors: {diagnostics}",
                    source_path.display()
                );
                for error in &expected {
                    error.check(&reported).unwrap_or_else(|error| {
                        panic!("{}: {error}\n{diagnostics}", source_path.display())
                    });
                }
            }
        }
    }
}
