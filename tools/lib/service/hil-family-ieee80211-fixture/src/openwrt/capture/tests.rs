use super::*;
use std::process::Command;

#[test]
fn managed_capture_cleanup_never_takes_interface_ownership() {
    let directory = tempfile::tempdir().unwrap();
    let capture_dir = directory.path().join("capture");
    fs::create_dir(&capture_dir).unwrap();
    let remote = RemoteInterface {
        create_interface: false,
        interface: "test0".into(),
        directory: capture_dir.to_str().unwrap().into(),
    };
    fs::write(remote.capture(), b"capture").unwrap();
    // Execute the actual cleanup with an iw sentinel. A managed interface is
    // borrowed; cleanup must not even attempt an interface operation.
    let script = format!("iw() {{ exit 90; }}; {}", remote.cleanup_script());
    assert!(
        Command::new("sh")
            .args(["-c", &script])
            .status()
            .unwrap()
            .success()
    );
    assert!(!capture_dir.exists());
    // A repeated cleanup does not gain authority over an existing interface.
    assert!(
        Command::new("sh")
            .args(["-c", &script])
            .status()
            .unwrap()
            .success()
    );
}

#[test]
fn independent_observer_refuses_busy_phy_and_cleans_failed_channel_setup() {
    use std::os::unix::fs::PermissionsExt;
    let temp = tempfile::tempdir().unwrap();
    let bin = temp.path().join("bin");
    fs::create_dir(&bin).unwrap();
    for (name, script) in [
        (
            "iw",
            r#"#!/bin/sh
case "$*" in
  'phy phy0 info') echo ' * monitor';;
  'dev') if [ "$OER_TEST_BUSY" = 1 ]; then printf 'phy#0\n\tInterface existing-ap\n'; fi;;
  'dev observe0 info') exit 1;;
  'phy phy0 interface add observe0 type monitor') touch "$OER_TEST_CREATED";;
  'dev observe0 set freq 2472 HT40-') exit 5;;
  'dev observe0 del') rm -f "$OER_TEST_CREATED";;
  *) exit 91;;
esac
"#,
        ),
        ("ip", "#!/bin/sh\nexit 0\n"),
        ("tcpdump", "#!/bin/sh\nexit 92\n"),
    ] {
        let path = bin.join(name);
        fs::write(&path, script).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    for busy in [true, false] {
        let remote = RemoteInterface {
            create_interface: true,
            interface: "observe0".into(),
            directory: temp
                .path()
                .join(if busy { "busy" } else { "failed" })
                .to_str()
                .unwrap()
                .into(),
        };
        let config = oer_hil_lab::config::AirObserverConfig {
            ssh_target: "unused".into(),
            phy: "phy0".into(),
            interface: "observe0".into(),
        };
        let script = remote
            .independent_script(
                &config,
                crate::channel::Geometry {
                    frequency: 2472,
                    width: 40,
                    center: 2462,
                },
                "type mgt",
                SnapshotLength::Headers,
                Duration::from_secs(1),
            )
            .unwrap();
        assert!(script.contains("-s 128"));
        let complete = remote
            .independent_script(
                &config,
                crate::channel::Geometry {
                    frequency: 2472,
                    width: 40,
                    center: 2462,
                },
                "type mgt",
                SnapshotLength::Complete,
                Duration::from_secs(1),
            )
            .unwrap();
        assert!(complete.contains("-s 2304"));
        let result = Command::new("sh")
            .args(["-c", &script])
            .env(
                "PATH",
                format!("{}:{}", bin.display(), std::env::var("PATH").unwrap()),
            )
            .env("OER_TEST_BUSY", if busy { "1" } else { "0" })
            .env("OER_TEST_CREATED", temp.path().join("created"))
            .output()
            .unwrap();
        assert!(!result.status.success());
        assert!(!temp.path().join("created").exists());
        assert_eq!(Path::new(&remote.directory).exists(), !busy);
    }
}

#[test]
fn summary_counts_ignore_surrounding_diagnostics() {
    let summary = "tcpdump: listening on egress0, link-type EN10MB\n\
                   7 packets captured\n8 packets received by filter\n\
                   0 packets dropped by kernel\n";
    assert_eq!(summary_value(summary, "packets captured"), Some(7));
    assert_eq!(summary_value(summary, "packets dropped by kernel"), Some(0));
    assert_eq!(summary_value(summary, "packets lost"), None);
}
