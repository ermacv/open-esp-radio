use super::profile_rules::{composition, parse};

const OPEN: &str = "ssid = \"lab\"\nsecurity = \"open\"\ncountry = \"DE\"\nchannel = 1\ncap_dbm = 10\necho_port = 7\n";

#[test]
fn the_committed_ci_profile_is_an_open_station_without_credentials() {
    let generated = parse(include_str!("../profiles/open-ci.toml")).unwrap();
    assert!(generated.contains("crate::profile::Security::Open"));
    assert!(!generated.contains("passphrase"));
    assert!(generated.contains("channel: 1"));
    assert!(generated.contains("cap_dbm: 10"));
    assert!(generated.contains("echo_port: 7"));
}

#[test]
fn a_wpa2_profile_keeps_its_passphrase_only_in_the_generated_constant() {
    let wpa2 = "ssid = \"lab\"\nsecurity = \"wpa2-personal\"\npassphrase = \"correct horse\"\ncountry = \"DE\"\nchannel = 6\ncap_dbm = 10\necho_port = 7\n";
    let generated = parse(wpa2).unwrap();
    assert!(generated.contains("Security::Wpa2Personal"));
    assert!(
        !generated.contains("correct horse"),
        "the bytes, not the text"
    );
}

#[test]
fn an_invalid_profile_is_refused_without_naming_its_values() {
    for (profile, field) in [
        (OPEN.replace("\"open\"", "\"wep\""), "security"),
        (OPEN.replace("ssid = \"lab\"\n", ""), "ssid"),
        (OPEN.replace("channel = 1", "channel = 14"), "channel"),
        (OPEN.replace("\"DE\"", "\"de\""), "country"),
        (OPEN.replace("cap_dbm = 10", "cap_dbm = 30"), "cap_dbm"),
        (OPEN.replace("echo_port = 7", "echo_port = 0"), "echo_port"),
        (OPEN.replace("echo_port = 7\n", ""), "echo_port"),
        (
            format!("{OPEN}passphrase = \"secret-pass\"\n"),
            "passphrase",
        ),
        (format!("{OPEN}bssid = \"02:00:00:00:00:01\"\n"), "bssid"),
        (OPEN.replace("\"open\"", "\"wpa2-personal\""), "passphrase"),
    ] {
        let reason = parse(&profile).unwrap_err();
        assert!(reason.contains(field), "{reason}");
        assert!(!reason.contains("secret-pass") && !reason.contains("lab\""));
    }
    // A missing security never falls back to Open.
    assert!(parse(&OPEN.replace("security = \"open\"\n", "")).is_err());
}

#[test]
fn a_country_without_a_supported_domain_fails_the_build() {
    // Well-formed, but the regulatory table has no domain for it: the image
    // would refuse it at start, so the profile is refused at build.
    let reason = parse(&OPEN.replace("\"DE\"", "\"US\"")).unwrap_err();
    assert!(reason.contains("country") && reason.contains("regulatory domain"));
    assert!(
        !reason.contains("US"),
        "the reason names the field, not its value"
    );
}

#[test]
fn a_profile_and_the_station_feature_come_together() {
    assert_eq!(composition(true, true), Ok(()));
    assert_eq!(composition(false, false), Ok(()));
    assert!(
        composition(true, false)
            .unwrap_err()
            .contains("--features station")
    );
    assert!(
        composition(false, true)
            .unwrap_err()
            .contains("OER_WIFI_STATION_PROFILE")
    );
}
