use super::*;

#[test]
fn coexistence_override_is_explicit_and_applies_only_to_ht40() {
    let lab = oer_hil_lab::config::LabConfig::for_test();
    let mut fixture = config();
    for policy in [
        oer_hil_lab::config::Coexistence::Respect,
        oer_hil_lab::config::Coexistence::ForceHt40,
    ] {
        fixture.coexistence = policy;
        for phy in [
            PhyExpectation::Ht20,
            PhyExpectation::Ht40,
            PhyExpectation::He20,
        ] {
            let output = render(&profile(&fixture, &lab.station, phy).unwrap());
            assert!(output.status.success());
            let config = String::from_utf8(output.stdout).unwrap();
            let force = phy == PhyExpectation::Ht40
                && policy == oer_hil_lab::config::Coexistence::ForceHt40;
            assert_eq!(
                field(&config, "noscan"),
                Some(if force { "1" } else { "0" })
            );
            assert_eq!(field(&config, "ht_coex"), Some("1"));
        }
    }
}

#[test]
fn startup_log_keeps_bounded_failure_context_and_redacts_credentials() {
    let lab = oer_hil_lab::config::LabConfig::for_test();
    let input = profile(&config(), &lab.station, PhyExpectation::Ht40).unwrap();
    let mut log = tempfile::NamedTempFile::new().unwrap();
    log.write_all(&vec![b'x'; 70000]).unwrap();
    writeln!(
        log,
        "\n20/40 MHz operation not permitted based on overlapping BSSes"
    )
    .unwrap();
    writeln!(
        log,
        "{} {}",
        lab.station.credentials().0,
        lab.station.credentials().1
    )
    .unwrap();
    let text = startup_log(log.path(), &input);
    assert!(text.len() <= 65536);
    assert!(text.contains("based on overlapping BSSes"));
    assert!(!text.contains(lab.station.credentials().0));
    assert!(!text.contains(lab.station.credentials().1));
    assert!(text.contains("[redacted]"));
    assert!(startup_log(&log.path().join("absent"), &input).contains("unavailable"));
}

#[test]
fn width_downgrade_reports_requested_and_actual_geometry() {
    let expected = geometry(&config(), PhyExpectation::Ht40);
    let observed = geometry(&config(), PhyExpectation::Ht20);
    let error = verify_geometry(expected, observed).unwrap_err().to_string();
    assert!(error.contains("expected primary=2472 MHz width=40 MHz center=2462 MHz"));
    assert!(error.contains("observed primary=2472 MHz width=20 MHz center=2472 MHz"));
    assert!(verify_geometry(expected, expected).is_ok());
}

fn config() -> LocalLinuxConfig {
    LocalLinuxConfig {
        interface: "wlan0".into(),
        phys: vec![PhyExpectation::He20],
        country: "DE".into(),
        channel: 13,
        ht40_above: false,
        address: "10.42.0.1".parse().unwrap(),
        prefix_length: 24,
        coexistence: oer_hil_lab::config::Coexistence::Respect,
    }
}

fn render(input: &str) -> std::process::Output {
    let helper = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../linux-net/open-radio-net");
    let mut child = oer_process::command("bash")
        .args([
            "-c",
            "source \"$1\" capabilities >/dev/null; read_ap_profile; write_ap_profile",
            "profile-test",
        ])
        .arg(helper)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    child.wait_with_output().unwrap()
}

#[test]
fn helper_generates_each_scenario_mode_from_the_typed_request() {
    let lab = oer_hil_lab::config::LabConfig::for_test();
    for (phy, ht, he) in [
        (PhyExpectation::Ht20, "[SHORT-GI-20]", "0"),
        (PhyExpectation::He20, "[SHORT-GI-20]", "1"),
        (
            PhyExpectation::Ht40,
            "[HT40-][SHORT-GI-20][SHORT-GI-40]",
            "0",
        ),
    ] {
        let input = profile(&config(), &lab.station, phy).unwrap();
        let output = render(&input);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result = String::from_utf8(output.stdout).unwrap();
        assert_eq!(field(&result, "channel"), Some("13"));
        assert_eq!(field(&result, "ht_capab"), Some(ht));
        assert_eq!(field(&result, "ieee80211ax"), Some(he));
        assert_eq!(
            field(&result, "wpa_passphrase"),
            Some(lab.station.credentials().1)
        );
        assert_eq!(field(&result, "ssid2"), input.lines().next());
        assert_eq!(field(&result, "wpa"), Some("2"));
    }
    let mut above = config();
    above.channel = 6;
    above.ht40_above = true;
    let output = render(&profile(&above, &lab.station, PhyExpectation::Ht40).unwrap());
    assert!(output.status.success());
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("[HT40+]")
    );
}

#[test]
fn helper_rejects_directive_injection_and_impossible_channel_combinations() {
    let lab = oer_hil_lab::config::LabConfig::for_test();
    let input = profile(&config(), &lab.station, PhyExpectation::He20).unwrap();
    for (line, invalid) in [
        (0, "ssid=other"),
        (2, "DE\nlogger_stdout=-1"),
        (3, "14"),
        (4, "HT40+"),
        (6, "/tmp/config"),
        (7, "1,2,3,4"),
    ] {
        let mut fields = input.lines().collect::<Vec<_>>();
        fields[line] = invalid;
        let output = render(&(fields.join("\n") + "\n"));
        assert!(!output.status.success(), "accepted invalid field {line}");
        assert!(
            output.stdout.is_empty(),
            "invalid profile must not be published"
        );
    }
}

#[test]
fn dhcp_pool_uses_the_configured_subnet_without_leasing_the_ap_address() {
    for (address, first, last) in [
        ("10.42.0.1", "10.42.0.2", "10.42.0.254"),
        ("10.42.0.254", "10.42.0.1", "10.42.0.253"),
    ] {
        let (start, end, mask) = dhcp_range(address.parse().unwrap(), 24).unwrap();
        assert_eq!(start.to_string(), first);
        assert_eq!(end.to_string(), last);
        assert_eq!(mask.to_string(), "255.255.255.0");
    }
    assert!(dhcp_range("10.42.0.0".parse().unwrap(), 24).is_err());
    assert!(dhcp_range("10.42.0.255".parse().unwrap(), 24).is_err());
    assert!(dhcp_range("10.42.0.1".parse().unwrap(), 32).is_err());
}

#[test]
fn helper_failure_retains_the_reason_without_credentials() {
    use std::os::unix::process::ExitStatusExt;
    let lab = oer_hil_lab::config::LabConfig::for_test();
    let input = profile(&config(), &lab.station, PhyExpectation::He20).unwrap();
    let (ssid, password) = lab.station.credentials();
    let output = std::process::Output {
        status: std::process::ExitStatus::from_raw(256),
        stdout: format!("ssid={ssid}\nssid2={}\n", input.lines().next().unwrap()).into_bytes(),
        stderr: format!("invalid setting wpa_passphrase={password}\nAP setup failed\n")
            .into_bytes(),
    };
    let text = diagnostic(&output, Some(&input));
    assert!(text.contains("AP setup failed"));
    assert!(!text.contains(ssid));
    assert!(!text.contains(password));
    assert!(!text.contains(input.lines().next().unwrap()));
}

#[test]
fn helper_owns_private_transient_profile_and_removes_it_on_stop() {
    let directory = tempfile::tempdir().unwrap();
    let helper = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../linux-net/open-radio-net");
    let lab = oer_hil_lab::config::LabConfig::for_test();
    let input = profile(&config(), &lab.station, PhyExpectation::He20).unwrap();
    let mut child = oer_process::command("bash")
        .args(["-c", include_str!("tests/start-stop.sh"), "helper-test"])
        .arg(helper)
        .arg(directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let arguments = String::from_utf8(output.stdout).unwrap();
    assert!(
        arguments
            .lines()
            .any(|line| line == "--dhcp-range=10.42.0.2,10.42.0.254,255.255.255.0,1h")
    );
}
