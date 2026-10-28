#![allow(clippy::disallowed_macros)]

//! Inline const geometry must reject ordinary constructor calls at codegen,
//! including constructors that are not used in a const initializer.
use std::{fs, path::Path, process::Command};

#[test]
fn every_constructor_rejects_zero_capacity_during_codegen() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let scratch_root = manifest.join("../../target/w1c");
    fs::create_dir_all(&scratch_root).unwrap();
    let scratch = tempfile::tempdir_in(scratch_root).unwrap();
    fs::write(scratch.path().join("Cargo.toml"), format!(
        "[package]\nname = \"clock-capacity\"\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\n[lib]\npath = \"case.rs\"\n[dependencies]\noer-time-virtual = {{ path = {manifest:?} }}\n"
    )).unwrap();
    for constructor in [
        "VirtualClock::new()",
        "VirtualClock::starting_at(oer_time_virtual::START)",
        "VirtualClock::default()",
    ] {
        for capacity in [1, 0] {
            fs::write(scratch.path().join("case.rs"), format!(
                "use oer_time_virtual::VirtualClock;\npub fn make() -> VirtualClock<{capacity}> {{ {constructor} }}\n"
            )).unwrap();
            let output = Command::new("cargo")
                .args(["build", "--offline", "--lib", "--manifest-path"])
                .arg(scratch.path().join("Cargo.toml"))
                .arg("--target-dir")
                .arg(scratch.path().join("build"))
                .output()
                .unwrap();
            let stderr = String::from_utf8_lossy(&output.stderr);
            assert_eq!(
                output.status.success(),
                capacity != 0,
                "{constructor}/{capacity}: {stderr}"
            );
            if capacity == 0 {
                assert!(stderr.contains("error[E0080]"), "{stderr}");
                assert!(
                    stderr.contains("a virtual clock needs at least one waker slot"),
                    "{stderr}"
                );
            }
        }
    }
}
