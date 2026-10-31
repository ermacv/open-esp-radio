use super::*;

const SCHEMA_1: &str = r#"# The stand file.
schema = 1

[stand]
id = "made-up-lab"
air = "exclusive"

[[hub]]
id = "top"
usb2 = "3-8"
# Port 2 reports `off` but keeps its power.
keeps-power = [2]

[access_point]
ssid = "made-up-ap-ssid"
channel = 13
# How wide.
channel_width = "mhz40-below"
target_address = "10.43.0.1/24"
client_address = "10.43.0.2/24"

[station_fixture]
kind = "open-wrt"
radio = "radio0"
ap_section = "default_radio0"
channel = 13
ssh_target = "made-up-router"
wireless_interface = "phy0-ap0"
ingress_interface = "lan2"
phys = ["legacy", "ht20"]
independent_laptop_monitor = true
"#;

#[test]
fn the_router_and_the_access_point_keep_their_keys_and_allow_the_run_channel() {
    let converted = from_schema_1(SCHEMA_1).unwrap();
    assert!(
        converted
            .text
            .contains("# Port 2 reports `off` but keeps its power.")
    );
    assert!(
        converted
            .text
            .contains("# How wide.\nwidths = [\"mhz40-below\"]")
    );
    let stand = crate::Stand::parse(&converted.text).unwrap();
    let router = stand.router.unwrap();
    assert_eq!(router.kind, "open-wrt");
    assert_eq!(router.ssh_target.expose(), "made-up-router");
    assert_eq!(router.channels, [13]);
    assert_eq!(router.phys, ["legacy", "ht20"]);
    assert_eq!(router.independent_laptop_monitor, Some(true));
    let access_point = stand.access_point.unwrap();
    assert_eq!(access_point.channels, [13]);
    assert_eq!(access_point.widths, ["mhz40-below"]);
    assert_eq!(converted.notes.len(), 3);
    // The credential the file holds stays where it was, never in a note.
    assert!(converted.text.contains("ssid = \"made-up-ap-ssid\""));
    assert!(!converted.notes.concat().contains("made-up"));
    let document = converted.text.parse::<DocumentMut>().unwrap();
    assert!(keys(document["router"].as_table().unwrap()).contains(&"channels"));
}

#[test]
fn a_key_with_no_place_or_another_schema_is_refused_without_its_value() {
    let unknown = SCHEMA_1.replace("phys =", "made_up_key = \"made-up-router\"\nphys =");
    let error = from_schema_1(&unknown).unwrap_err();
    assert!(
        error.contains("made_up_key") && !error.contains("made-up-router"),
        "{error}"
    );
    let converted = from_schema_1(SCHEMA_1).unwrap();
    assert!(from_schema_1(&converted.text).is_err());
    let schema_1 = crate::Stand::parse(SCHEMA_1).unwrap_err();
    assert!(schema_1.contains("stand-file convert"), "{schema_1}");
}

#[test]
fn a_local_station_becomes_its_own_section() {
    let local = "schema = 1\n[station_fixture]\nkind = \"local\"\ncountry = \"DE\"\n";
    let stand = crate::Stand::parse(&from_schema_1(local).unwrap().text).unwrap();
    assert_eq!(stand.local_station.unwrap().country.as_deref(), Some("DE"));
    assert!(stand.router.is_none());
}

#[test]
fn the_example_stand_files_are_schema_2() {
    for example in [
        include_str!("../../stand.example.toml"),
        include_str!("../../../router/stand/stand.example.toml"),
    ] {
        let stand = crate::Stand::parse(example).unwrap();
        assert_eq!(stand.router.unwrap().channels, [13]);
        assert_eq!(stand.access_point.unwrap().widths, ["mhz40-below"]);
    }
}
