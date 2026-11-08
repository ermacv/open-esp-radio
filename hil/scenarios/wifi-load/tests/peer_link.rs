//! The vendor receiver peer's link state machine
//! (`tools/lab/peers/esp32s31-wifi-udp-rx/main/link.c`) and its host test
//! (`test/link_test.c`), compiled for this host and run. The C compiler is
//! `$OER_HOST_CC` when set, otherwise `cc`; a missing compiler, a warning,
//! a failed compile or a failed case fails this test, never skips it.
#![forbid(unsafe_code)]

use std::{path::Path, process::Command};

#[test]
fn the_peer_s_link_state_machine_passes_its_host_test() {
    let peer =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../tools/lab/peers/esp32s31-wifi-udp-rx");
    let scratch = tempfile::tempdir().unwrap();
    let binary = scratch.path().join("link_test");
    let compiler = std::env::var_os("OER_HOST_CC").unwrap_or_else(|| "cc".into());
    let compiled = Command::new(&compiler)
        .args(["-std=c11", "-Wall", "-Wextra", "-Werror", "-I"])
        .arg(peer.join("main"))
        .arg(peer.join("test/link_test.c"))
        .arg(peer.join("main/link.c"))
        .arg("-o")
        .arg(&binary)
        .output()
        .unwrap_or_else(|error| {
            panic!(
                "the host C compiler `{}` cannot run ({error}); set OER_HOST_CC",
                compiler.to_string_lossy()
            )
        });
    assert!(
        compiled.status.success(),
        "the host test does not compile: {}{}",
        String::from_utf8_lossy(&compiled.stdout),
        String::from_utf8_lossy(&compiled.stderr)
    );
    let ran = Command::new(&binary).output().unwrap();
    assert!(
        ran.status.success(),
        "the link state machine fails its host test: {}{}",
        String::from_utf8_lossy(&ran.stdout),
        String::from_utf8_lossy(&ran.stderr)
    );
    assert!(String::from_utf8_lossy(&ran.stdout).contains("every case holds"));
}
