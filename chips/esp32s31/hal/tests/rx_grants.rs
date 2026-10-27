//! Compile the actual HAL with an internal caller. Private APIs stay private;
//! pass/reject fixtures and weakened-bound controls run in isolated copies.
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

fn copy_tree(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let path = entry.path();
        let destination = destination.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, &destination)?;
        } else {
            fs::copy(&path, destination)?;
        }
    }
    Ok(())
}

#[test]
fn private_grants_follow_the_actual_ring_boundaries() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch = tempfile::Builder::new()
        .prefix("rx-grant-ui-")
        .tempdir_in(env!("CARGO_TARGET_TMPDIR"))
        .unwrap();
    let build = scratch.path().join("build");
    let mut cargo = Command::new(env!("CARGO"));
    cargo
        .args([
            "check",
            "--quiet",
            "--offline",
            "--locked",
            "--lib",
            "--message-format=json",
            "--manifest-path",
        ])
        .arg(manifest.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(&build);
    if cfg!(feature = "bluetooth-engine") {
        cargo.args(["--features", "bluetooth-engine"]);
    }
    let output = cargo.output().unwrap();
    fs::write(scratch.path().join("dependencies.log"), &output.stderr).unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );

    // Cargo's configured build-dir may place intermediates outside target-dir.
    // Its messages name the artifacts and build-script output actually used.
    let mut artifacts: BTreeMap<String, Vec<PathBuf>> = BTreeMap::new();
    let mut out = None;
    for line in output
        .stdout
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        let message: serde_json::Value = serde_json::from_slice(line).unwrap();
        match message["reason"].as_str() {
            Some("compiler-artifact") => {
                let name = message["target"]["name"].as_str().unwrap();
                for file in message["filenames"].as_array().unwrap() {
                    let path = PathBuf::from(file.as_str().unwrap());
                    if path
                        .extension()
                        .is_some_and(|extension| extension == "rmeta")
                    {
                        artifacts.entry(name.to_owned()).or_default().push(path);
                    }
                }
            }
            Some("build-script-executed") => {
                let path = PathBuf::from(message["out_dir"].as_str().unwrap());
                if path.join("interrupts.rs").is_file() {
                    assert!(out.replace(path).is_none());
                }
            }
            _ => {}
        }
    }
    let out = out.expect("cargo reports the HAL build-script output");
    let source = scratch.path().join("src");
    copy_tree(&manifest.join("src"), &source).unwrap();
    let root = source.join("lib.rs");
    let caller = source.join("ieee80211/dma/rx_storage.rs");
    let mut text = fs::read_to_string(&caller).unwrap();
    text.push_str("\nmod grant_fixture {\n");
    text.push_str(include_str!("internal/rx_grants.rs"));
    text.push_str("\n}\n");
    fs::write(&caller, text).unwrap();

    let manifest_text = fs::read_to_string(manifest.join("Cargo.toml")).unwrap();
    let dependency_names = manifest_text
        .split("[dependencies]\n")
        .nth(1)
        .unwrap()
        .split("\n[")
        .next()
        .unwrap()
        .lines()
        .filter_map(|line| {
            line.split_once(" = ")
                .map(|(name, _)| name.replace('-', "_"))
        })
        .collect::<Vec<_>>();
    let mut externs = Vec::new();
    for name in dependency_names {
        let matches = artifacts.get(&name).expect("cargo reports each dependency");
        assert_eq!(
            matches.len(),
            1,
            "one metadata artifact for {name}: {matches:?}"
        );
        let artifact = &matches[0];
        externs.push(format!("{name}={}", artifact.display()));
    }
    let dependencies = artifacts
        .values()
        .flatten()
        .map(|artifact| artifact.parent().unwrap())
        .collect::<std::collections::BTreeSet<_>>();
    let compile = |mode: &str| -> Output {
        let mut rustc = Command::new("rustc");
        rustc
            .args([
                "--edition=2024",
                "--crate-type=lib",
                "--crate-name=oer_esp32s31_hal",
                "--emit=metadata",
                "-A",
                "warnings",
            ])
            .arg(&root)
            .arg("-o")
            .arg(scratch.path().join(format!("{mode}.rmeta")))
            .arg("--cfg")
            .arg(mode)
            .env("OUT_DIR", &out)
            .env("CARGO_MANIFEST_DIR", manifest);
        if cfg!(feature = "bluetooth-engine") {
            rustc.args(["--cfg", "feature=\"bluetooth-engine\""]);
        }
        for dependency in &externs {
            rustc.arg("--extern").arg(dependency);
        }
        for dependency in &dependencies {
            rustc
                .arg("-L")
                .arg(format!("dependency={}", dependency.display()));
        }
        let output = rustc.output().unwrap();
        fs::write(scratch.path().join(format!("{mode}.log")), &output.stderr).unwrap();
        output
    };
    let accepted = compile("grant_accept");
    assert!(
        accepted.status.success(),
        "pass fixture: {}",
        String::from_utf8_lossy(&accepted.stderr)
    );
    let escapes = [
        "escape_half",
        "escape_prefix",
        "escape_unit",
        "escape_released",
        "escape_proven",
        "escape_prepare",
        "escape_reserved",
    ];
    for (mode, expected) in escapes.iter().map(|mode| (*mode, "E0521")).chain([
        ("foreign_ring", "lifetime may not live long enough"),
        ("forge_grant", "E0624"),
        ("raw_bytes", "E0624"),
        ("raw_completed", "E0624"),
        ("raw_detach", "E0624"),
        ("duplicate_grant", "E0382"),
        ("read_rearm", "E0308"),
    ]) {
        let rejected = compile(mode);
        let diagnostics = String::from_utf8_lossy(&rejected.stderr);
        assert!(
            !rejected.status.success() && diagnostics.contains(expected),
            "{mode}: {diagnostics}"
        );
    }
    // Sensitivity control: relaxing the real methods' callback bounds must
    // make these same escape fixtures compile. This changes scratch only.
    let ring = source.join("ieee80211/dma/rx_ring.rs");
    let strict = fs::read_to_string(&ring).unwrap();
    let relaxed = strict
        .replace(
            "for<'grant> FnMut(RecycleGrant<'id, 'grant>)",
            "FnMut(RecycleGrant<'id, 'id>)",
        )
        .replace(
            "for<'grant> FnOnce(RecycleGrant<'id, 'grant, Rearm, true>)",
            "FnOnce(RecycleGrant<'id, 'id, Rearm, true>)",
        );
    assert_ne!(strict, relaxed, "actual callback bounds were found");
    fs::write(&ring, relaxed).unwrap();
    for mode in escapes {
        let output = compile(mode);
        assert!(
            output.status.success(),
            "relaxed {mode}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
