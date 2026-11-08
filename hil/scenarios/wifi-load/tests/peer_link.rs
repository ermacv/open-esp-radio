//! The vendor receiver peer's link state machine
//! (`tools/lab/peers/esp32s31-wifi-udp-rx/main/link.c`) and its host test
//! (`test/link_test.c`), compiled for this host and run in the test's own
//! temporary directory. The C11 compiler is `$OER_HOST_CC` when set,
//! otherwise `cc`; its path and version are logged. A missing compiler, a
//! warning, a failed compile or a failed case fails this test, never skips
//! it (the CLAUDE.md exception for this test).
#![forbid(unsafe_code)]

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    process::Command,
};

/// Where `compiler` runs from: itself when it names a path, otherwise the
/// first file of that name on `PATH`.
fn resolve(compiler: &OsString) -> Option<PathBuf> {
    let named = Path::new(compiler);
    if named.components().count() > 1 {
        return named.is_file().then(|| named.to_owned());
    }
    std::env::split_paths(&std::env::var_os("PATH")?)
        .map(|directory| directory.join(named))
        .find(|candidate| candidate.is_file())
}

#[test]
fn the_peer_s_link_state_machine_passes_its_host_test() {
    let peer =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tools/lab/peers/esp32s31-wifi-udp-rx");
    let compiler = std::env::var_os("OER_HOST_CC").unwrap_or_else(|| "cc".into());
    let path = resolve(&compiler).unwrap_or_else(|| {
        panic!(
            "no host C compiler `{}` (set OER_HOST_CC to a C11 compiler)",
            compiler.to_string_lossy()
        )
    });
    let version = Command::new(&path)
        .arg("--version")
        .output()
        .unwrap_or_else(|error| {
            panic!("the host C compiler {} cannot run: {error}", path.display())
        });
    assert!(
        version.status.success(),
        "{} --version failed: {}",
        path.display(),
        String::from_utf8_lossy(&version.stderr)
    );
    let first = String::from_utf8_lossy(&version.stdout)
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned();
    println!("host C compiler: {} ({first})", path.display());
    let scratch = tempfile::tempdir().unwrap();
    let binary = scratch.path().join("link_test");
    let compiled = Command::new(&path)
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-I"])
        .arg(peer.join("main"))
        .arg(peer.join("test/link_test.c"))
        .arg(peer.join("main/link.c"))
        .arg("-o")
        .arg(&binary)
        .current_dir(scratch.path())
        .output()
        .unwrap_or_else(|error| {
            panic!("the host C compiler {} cannot run: {error}", path.display())
        });
    assert!(
        compiled.status.success(),
        "the host test does not compile with {} ({first}): {}{}",
        path.display(),
        String::from_utf8_lossy(&compiled.stdout),
        String::from_utf8_lossy(&compiled.stderr)
    );
    let ran = Command::new(&binary)
        .current_dir(scratch.path())
        .output()
        .unwrap();
    assert!(
        ran.status.success(),
        "the link state machine fails its host test: {}{}",
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );
    assert!(String::from_utf8_lossy(&ran.stdout).contains("every case holds"));
}
