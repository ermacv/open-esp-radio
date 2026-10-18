//! Opt-in acceptance test of the installed Linux/OpenWrt fixture, without a DUT.
#![cfg(target_os = "linux")]

use oer_process::CommandExt as _;
use std::{path::Path, process::Command};

#[test]
#[ignore = "requires configured OpenWrt and installed Linux helper; never accesses ESP"]
fn fixture_check_exercises_monitor_capture_and_restores_without_a_serial_device() {
    // The host's stand file; the fixture check never opens the device.
    let output = Command::new(env!("CARGO_BIN_EXE_oer-hil-runner"))
        .args([
            "fixture",
            "check",
            "udp-bidirectional-ht40-rx-delivery-split",
        ])
        .supervised_output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(result["report"]["device_accessed"], false);
    assert_eq!(result["report"]["prepared"], true);
    assert_eq!(result["report"]["restored"], true);
    let artifacts = Path::new(result["artifacts"].as_str().unwrap());
    assert!(artifacts.join("fixture-monitor.json").is_file());
    assert!(artifacts.join("independent-air.pcapng").is_file());
}
