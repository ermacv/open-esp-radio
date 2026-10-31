use super::profile_rules::parse;

const OPEN: &str =
    "ssid = \"lab\"\nsecurity = \"open\"\ncountry = \"DE\"\nchannel = 1\ncap_dbm = 10\n";

#[test]
fn the_committed_ci_profile_is_an_open_station_without_credentials() {
    let generated = parse(include_str!("../profiles/open-ci.toml")).unwrap();
    assert!(generated.contains("crate::profile::Security::Open"));
    assert!(!generated.contains("passphrase"));
    assert!(generated.contains("channel: 1"));
    assert!(generated.contains("cap_dbm: 10"));
}

#[test]
fn a_wpa2_profile_keeps_its_passphrase_only_in_the_generated_constant() {
    let wpa2 = "ssid = \"lab\"\nsecurity = \"wpa2-personal\"\npassphrase = \"correct horse\"\ncountry = \"DE\"\nchannel = 6\ncap_dbm = 10\n";
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
