//! Compile the actual HAL with an internal caller. Private APIs stay private;
//! pass/reject fixtures and weakened-bound controls run in isolated copies.
use std::{
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

fn directories(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    fs::read_dir(path)?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect()
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

    let dependencies = build.join("debug/deps");
    let artifacts = directories(&dependencies).unwrap();
    let out = directories(&build.join("debug/build"))
        .unwrap()
        .into_iter()
        .map(|path| path.join("out"))
        .find(|path| path.join("interrupts.rs").is_file())
        .unwrap();
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
        let prefix = format!("lib{name}-");
        let matches = artifacts
            .iter()
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_string_lossy()
                    .starts_with(&prefix)
                    && path
                        .extension()
                        .is_some_and(|extension| extension == "rmeta")
            })
            .collect::<Vec<_>>();
        assert_eq!(
            matches.len(),
            1,
            "one metadata artifact for {name}: {matches:?}"
        );
        externs.push(format!("{name}={}", matches[0].display()));
    }
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
            .arg("-L")
            .arg(format!("dependency={}", dependencies.display()))
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
