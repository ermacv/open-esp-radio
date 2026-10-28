use super::*;
use oer_time::Clock as _;

const OPEN: &str = r#"{"ssid_hex":"4c6162","security":"open","frequency_mhz":2437,"phy":"ht","ipv4":{"mode":"dhcp"}}"#;

#[test]
fn open_profile_preserves_ssid_bytes_without_psk_or_rsn() {
    let profile = Profile::parse(OPEN).expect("valid test fixture");
    let cfg = profile.supplicant().expect("valid test fixture");
    assert!(cfg.contains("ssid=4c6162\n"));
    assert!(cfg.contains("key_mgmt=NONE\n"));
    assert!(cfg.contains("auth_alg=OPEN\n"));
    assert!(cfg.contains("disabled=1\n"));
    assert!(!cfg.contains("psk="));
    assert!(!cfg.contains("proto=RSN"));
    assert_eq!(profile.setup(), "dhcp 2437\n");
    let shown = format!("{profile:?}");
    assert!(!shown.contains("4c6162"));
    assert!(!shown.contains("Lab"));
}
#[test]
fn wpa2_static_and_non_ht_remain_explicit_profiles() {
    let json = OPEN
        .replace("\"open\"", "\"wpa2\",\"passphrase\":\"made-up-password\"")
        .replace("\"ht\"", "\"non-ht\"")
        .replace("\"dhcp\"", "\"static\",\"address\":\"192.0.2.8/24\"");
    let p = Profile::parse(&json).expect("valid test fixture");
    let cfg = p.supplicant().expect("valid test fixture");
    assert!(cfg.contains("key_mgmt=WPA-PSK\n"));
    assert!(cfg.contains("proto=RSN\n"));
    assert!(cfg.contains("disable_ht=1\n"));
    assert!(cfg.contains("disable_he=1\n"));
    assert_eq!(p.setup(), "192.0.2.8/24 2437\n");
    assert!(!format!("{p:?}").contains("made-up"));
    let none = Profile::parse(&OPEN.replace("\"dhcp\"", "\"none\"")).expect("valid test fixture");
    assert_eq!(none.setup(), "none 2437\n");
}
#[test]
fn profiles_reject_ambiguous_security_unknown_fields_and_shell_input() {
    for json in [
        OPEN.replace("\"open\"", "\"wpa2\""),
        OPEN.replace("\"open\"", "\"open\",\"passphrase\":\"made-up-password\""),
        OPEN.replace("\"open\"", "\"sae\""),
        OPEN.replace("2437", "2438"),
        OPEN.replace("2437", "5180"),
        OPEN.replace("4c6162", ""),
        OPEN.replace("4c6162", "4c6"),
        OPEN.replace("4c6162", "$(id)"),
        OPEN.replace("4c6162", "4c6162\\nkey_mgmt=NONE"),
        OPEN.replace("\"dhcp\"", "\"static\",\"address\":\"127.0.0.1/8\""),
        OPEN.replace("\"dhcp\"", "\"dhcp\",\"address\":\"192.0.2.8/24\""),
        OPEN.replace("\"phy\"", "\"extra\":1,\"phy\""),
        format!("{OPEN}{OPEN}"),
    ] {
        let error = Profile::parse(&json).expect_err("invalid test profile");
        assert!(!error.contains("4c6162"));
        assert!(!error.contains("made-up"));
    }
    assert!(Profile::parse(&"x".repeat(2049)).is_err());
}
#[test]
fn credentials_are_quoted_only_in_the_private_config() {
    let p =
        Profile::parse(&OPEN.replace("\"open\"", r#""wpa2","passphrase":"made\\up\"password""#))
            .expect("valid test fixture");
    assert!(
        p.supplicant()
            .expect("valid test fixture")
            .contains(r#"psk="made\\up\"password""#)
    );
    assert!(!format!("{p:?}").contains("password"));
}
#[test]
fn dhcp_options_are_validated_before_applying_a_lease() {
    let good = Lease::from_options(
        "192.0.2.8",
        "255.255.255.0",
        "192.0.2.1",
        "192.0.2.1",
        "3600",
        100,
    )
    .expect("valid test fixture");
    assert_eq!(good.prefix, 24);
    assert_eq!(good.expires_at, 3700);
    for (ip, mask, router, server, seconds) in [
        ("192.0.2.8", "255.0.255.0", "192.0.2.1", "192.0.2.1", "3600"),
        ("192.0.2.8", "0.0.0.0", "192.0.2.1", "192.0.2.1", "3600"),
        (
            "192.0.2.8",
            "255.255.255.0",
            "192.0.2.1 192.0.2.2",
            "192.0.2.1",
            "3600",
        ),
        (
            "192.0.2.8",
            "255.255.255.0",
            "192.0.3.1",
            "192.0.2.1",
            "3600",
        ),
        (
            "192.0.2.0",
            "255.255.255.0",
            "192.0.2.1",
            "192.0.2.1",
            "3600",
        ),
        (
            "192.0.2.255",
            "255.255.255.0",
            "192.0.2.1",
            "192.0.2.1",
            "3600",
        ),
        (
            "192.0.2.8;id",
            "255.255.255.0",
            "192.0.2.1",
            "192.0.2.1",
            "3600",
        ),
        ("192.0.2.8", "255.255.255.0", "192.0.2.1", "0.0.0.0", "3600"),
        (
            "192.0.2.8",
            "255.255.255.0",
            "192.0.2.0",
            "192.0.2.1",
            "3600",
        ),
        (
            "192.0.2.8",
            "255.255.255.0",
            "192.0.2.255",
            "192.0.2.1",
            "3600",
        ),
        ("192.0.2.8", "255.255.255.0", "192.0.2.1", "192.0.2.1", "0"),
    ] {
        assert!(Lease::from_options(ip, mask, router, server, seconds, 100).is_err());
    }
    assert!(
        Lease::from_options(
            "192.0.2.8",
            "255.255.255.0",
            "192.0.2.1",
            "192.0.2.1",
            "3600",
            u64::MAX
        )
        .is_err()
    );
}
#[test]
fn a_renewal_requires_every_mandatory_dhcp_option() {
    let options = [
        ("ip", "192.0.2.8"),
        ("subnet", "255.255.255.0"),
        ("router", "192.0.2.1"),
        ("serverid", "192.0.2.1"),
        ("lease", "3600"),
    ];
    let lookup = |name: &str| {
        options
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| value.to_string())
            .ok_or("missing DHCP option")
    };
    assert!(Lease::from_hook_options(100, lookup).is_ok());
    for (missing, _) in options {
        assert!(
            Lease::from_hook_options(100, |name| {
                if name == missing {
                    Err("missing DHCP option")
                } else {
                    lookup(name)
                }
            })
            .is_err()
        );
    }
}
#[test]
fn lease_expiry_and_renewal_use_an_instance_virtual_clock() {
    let clock = oer_time_virtual::VirtualClock::<1>::new();
    let start = clock.now().as_micros() / 1_000_000;
    let first = Lease::from_options(
        "192.0.2.8",
        "255.255.255.0",
        "192.0.2.1",
        "192.0.2.1",
        "10",
        start,
    )
    .expect("valid test fixture");
    assert!(first.active(start));
    assert!(!first.active(start - 1));
    clock
        .advance(oer_time::Duration::from_secs(9))
        .expect("valid test fixture");
    assert!(first.active(clock.now().as_micros() / 1_000_000));
    let renewed = Lease::from_options(
        "192.0.2.8",
        "255.255.255.0",
        "192.0.2.1",
        "192.0.2.1",
        "10",
        clock.now().as_micros() / 1_000_000,
    )
    .expect("valid test fixture");
    clock
        .advance(oer_time::Duration::from_secs(1))
        .expect("valid test fixture");
    assert!(!first.active(clock.now().as_micros() / 1_000_000));
    assert!(renewed.active(clock.now().as_micros() / 1_000_000));
}
#[test]
fn hook_and_session_commands_are_finite_and_interface_scoped() {
    for action in ["bound", "renew", "deconfig", "leasefail", "nak"] {
        assert!(Hook::parse(action, "wlan0").is_ok());
        assert!(Hook::parse(action, "eth0").is_err());
    }
    assert!(Hook::parse("custom", "wlan0").is_err());
    for action in ["status\n", "ping\n", "disconnect\n"] {
        assert!(SessionCommand::parse(action).is_ok());
    }
    for action in [
        "ping 192.0.2.1",
        "ping;id",
        "status extra",
        "set-path /tmp/config",
        " ping",
    ] {
        assert!(SessionCommand::parse(action).is_err());
    }
}
#[test]
fn ping_counts_losses_and_refuses_garbage_or_contradictory_status() {
    assert_eq!(
        Ping::parse("100 packets transmitted, 100 received, 0% packet loss\n", 0)
            .expect("valid test fixture"),
        Ping {
            sent: 100,
            received: 100,
            interval_ms: 1000
        }
    );
    assert_eq!(
        Ping::parse("100 packets transmitted, 99 received, 1% packet loss\n", 0)
            .expect("valid test fixture")
            .received,
        99
    );
    for (text, status) in [
        ("", 0),
        ("100 packets transmitted, 101 received, 0% packet loss", 0),
        ("100 packets transmitted, 99 received, 1% packet loss", 1),
        ("100 packets transmitted, 0 received, 100% packet loss", 0),
        ("100 packets transmitted, 100 received, 0% packet loss", 1),
        ("100 packets transmitted, 100 received, 0% packet loss", 2),
        ("99 packets transmitted, 99 received, 0% packet loss", 0),
        (
            "100 packets transmitted, 100 received, 0% packet loss\n100 packets transmitted, 100 received, 0% packet loss",
            0,
        ),
    ] {
        assert!(Ping::parse(text, status).is_err());
    }
    assert_eq!(
        Ping::parse("100 packets transmitted, 0 received, 100% packet loss", 1)
            .expect("valid test fixture")
            .received,
        0
    );
}

#[test]
fn session_has_one_ping_a_finite_lifetime_and_a_terminal_disconnect() {
    let clock = oer_time_virtual::VirtualClock::<1>::new();
    let now = || clock.now().as_micros();
    let mut state = SessionState::new(now()).expect("valid test fixture");
    assert_eq!(
        state
            .command(SessionCommand::Status, now())
            .expect("valid test fixture"),
        240_000_000
    );
    assert!(state.command(SessionCommand::Ping, now()).is_ok());
    assert!(state.command(SessionCommand::Ping, now()).is_err());
    clock
        .advance(oer_time::Duration::from_secs(239))
        .expect("valid test fixture");
    assert_eq!(
        state.remaining(now()).expect("valid test fixture"),
        1_000_000
    );
    clock
        .advance(oer_time::Duration::from_secs(1))
        .expect("valid test fixture");
    assert!(state.remaining(now()).is_err());
    assert!(state.command(SessionCommand::Status, now()).is_err());
    assert!(state.command(SessionCommand::Disconnect, now()).is_ok());
    assert!(state.command(SessionCommand::Status, now()).is_err());
    assert!(state.command(SessionCommand::Disconnect, now()).is_err());
}
