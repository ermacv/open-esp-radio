//! Execute the real fixture owners and remote shell programs with local command
//! substitutes. Environment changes are confined to isolated test processes.

use std::{fs, path::PathBuf, time::Duration};

const HARNESS: &str = "test_support::fixture_lifecycle_harness";

#[test]
fn preparation_and_monitor_recover_from_partial_setup() {
    use std::os::unix::fs::PermissionsExt;
    for case in [
        "monitor-existing",
        "monitor-error",
        "monitor-cancel",
        "monitor-drop",
        "monitor-discovery-drop",
        "ap-ssh-error",
        "ap-command-error",
        "cleanup-success",
        "cleanup-absent",
        "cleanup-nft-error",
        "cleanup-nft-noop",
        "cleanup-iw-error",
        "cleanup-iw-noop",
        "cleanup-inspection-error",
        "cleanup-process-success",
        "cleanup-process-mismatch",
        "cleanup-process-error",
        "cleanup-process-noop",
    ] {
        let root = std::env::temp_dir().join(format!("oer-fixture-{}-{case}", std::process::id()));
        fs::create_dir_all(root.join("bin")).unwrap();
        fs::write(
            root.join("remote.sh"),
            include_str!("test_support/remote.sh"),
        )
        .unwrap();
        let ssh = root.join("bin/ssh");
        fs::write(
            &ssh,
            "#!/bin/sh\nif test \"$OER_TEST_CASE\" = ap-ssh-error; then echo 'injected SSH transport failure' >&2; exit 255; fi\nfor script do :; done\n. \"$OER_TEST_STATE/remote.sh\"\neval \"$script\"\n",
        )
        .unwrap();
        fs::set_permissions(&ssh, fs::Permissions::from_mode(0o700)).unwrap();
        let inherited_path = std::env::var_os("PATH").unwrap();
        let paths = std::iter::once(root.join("bin")).chain(std::env::split_paths(&inherited_path));
        let output = oer_process::output(
            oer_process::command(std::env::current_exe().unwrap())
                .args(["--exact", HARNESS, "--nocapture"])
                .env("PATH", std::env::join_paths(paths).unwrap())
                .env("OER_TEST_STATE", &root)
                .env("OER_TEST_CASE", case),
            Some(Duration::from_secs(15)),
        )
        .unwrap();
        assert!(
            output.status.success(),
            "{case}: {}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        fs::remove_dir_all(root).unwrap();
    }
}

#[test]
fn fixture_lifecycle_harness() {
    let Ok(case) = std::env::var("OER_TEST_CASE") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("OER_TEST_STATE").unwrap());
    let _signals = oer_process::install_signal_handlers().unwrap();
    let scope = oer_hil_workload::fixture::cleanup::Scope::new(&root);
    let lab = oer_hil_lab::config::LabConfig::for_test();
    let oer_hil_lab::config::StationFixtureConfig::OpenWrt(config) = &lab.station_fixture else {
        panic!("OpenWrt test lab required");
    };
    if case.starts_with("ap-") {
        let error = super::controlled_ap::ControlledAp::start(
            &lab.station,
            &lab.station_fixture,
            oer_hil_scenario_catalog::link::PhyExpectation::Ht40,
            oer_hil_scenario_catalog::link::ManagementFrameProtection::Disabled,
            oer_hil_scenario_catalog::link::AccessPointSecurity::Wpa2Personal,
            None,
        )
        .err()
        .expect("injected fixture failure");
        assert_eq!(
            oer_hil_workload::failure::classify(&*error).kind,
            oer_hil_run_bundle_format::run::FailureKind::Infrastructure
        );
        scope.finish().unwrap();
        return;
    }
    if case.starts_with("cleanup-") {
        if case != "cleanup-absent" {
            for resource in ["client", "nat", "chain"] {
                fs::write(root.join(resource), "owned").unwrap();
            }
        }
        fs::write(root.join("foreign"), "external").unwrap();
        if case.starts_with("cleanup-process-") {
            fs::write(root.join("process"), "running").unwrap();
        }
        oer_hil_workload::fixture::cleanup::record("restore OpenWrt client", || {
            super::openwrt::client::restore(config)
        });
        let records = scope.finish().unwrap();
        let success = matches!(
            case.as_str(),
            "cleanup-success" | "cleanup-absent" | "cleanup-process-success"
        );
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].failure.is_none(), success);
        if success {
            for resource in ["client", "nat", "chain", "process"] {
                assert!(!root.join(resource).exists(), "owned {resource} leaked");
            }
            // Recovery is idempotent after successful deletion.
            super::openwrt::client::restore(config).unwrap();
        } else {
            assert!(super::openwrt::client::restore(config).is_err());
        }
        if case == "cleanup-process-mismatch" {
            assert!(root.join("process").exists());
            assert!(
                !root.join("kill-attempted").exists(),
                "foreign process was signalled"
            );
        }
        assert_eq!(
            fs::read_to_string(root.join("foreign")).unwrap(),
            "external"
        );
        return;
    }
    let canceller = case.ends_with("cancel").then(|| {
        let ready = root.join("ready");
        std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            while !ready.exists() {
                assert!(
                    std::time::Instant::now() < deadline,
                    "setup did not reach cancellation boundary"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
            // This isolated process installed a SIGTERM handler above.
            rustix::process::kill_process(rustix::process::getpid(), rustix::process::Signal::TERM)
                .unwrap();
        })
    });
    {
        if case == "monitor-existing" {
            fs::write(root.join("monitor"), "external").unwrap();
        }
        let result: crate::Result<Box<dyn std::any::Any>> = if case == "monitor-discovery-drop" {
            super::openwrt::tx_monitor::OpenWrtManagementCapture::start(
                config,
                &root,
                Duration::from_secs(3),
            )
            .map(|owner| Box::new(owner) as Box<dyn std::any::Any>)
        } else {
            super::openwrt::tx_monitor::OpenWrtTxMonitorCapture::start(
                config,
                "192.0.2.2".parse().unwrap(),
                4323,
                Duration::from_secs(3),
                &root,
            )
            .map(|owner| Box::new(owner) as Box<dyn std::any::Any>)
        };
        assert_eq!(
            result.is_ok(),
            matches!(case.as_str(), "monitor-drop" | "monitor-discovery-drop")
        );
        if case == "monitor-discovery-drop" {
            let arguments = fs::read_to_string(root.join("capture-arguments")).unwrap();
            let arguments: Vec<_> = arguments.lines().collect();
            assert!(
                arguments.contains(&"--immediate-mode"),
                "short capture must not wait for packet-block timeout"
            );
            assert!(
                arguments.contains(&"type mgt or type ctl"),
                "broadcast discovery frames and control ACKs must reach capture"
            );
        }
        if case == "monitor-error" || case == "monitor-existing" {
            assert_eq!(
                oer_hil_workload::failure::classify(&**result.as_ref().err().unwrap()).kind,
                oer_hil_run_bundle_format::run::FailureKind::Infrastructure
            );
        }
        if case.ends_with("cancel") {
            assert!(oer_process::is_cancelled(&*result.err().unwrap()));
        } else {
            drop(result);
        }
        if case == "monitor-existing" {
            assert_eq!(
                fs::read_to_string(root.join("monitor")).unwrap(),
                "external"
            );
        } else {
            assert!(!root.join("monitor").exists(), "owned monitor leaked");
        }
        if let Ok(directory) = fs::read_to_string(root.join("remote-directory")) {
            assert!(
                !std::path::Path::new(directory.trim()).exists(),
                "remote ownership directory leaked"
            );
        }
    }
    if let Some(canceller) = canceller {
        canceller.join().unwrap();
    }
    let records = scope.finish().unwrap();
    assert_eq!(records.len(), 1);
    assert!(!records.iter().any(|r| r.failure.is_some()));
}
