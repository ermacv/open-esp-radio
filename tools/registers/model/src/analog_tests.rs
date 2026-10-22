use super::*;

fn fixture() -> String {
    r#"schema = 1
kind = "analog-i2c"
[[blocks]]
name = "PLL"
code = 0x66
sources = ["HEADER"]
[blocks.review]
sources = ["HEADER"]
provenance = "reviewed"
accuracy = "exact"
completeness = "unknown"
naming = "descriptive"
[[blocks.routes]]
profile = "libphy-normal"
request-code = 0x66
host = 1
read-select = 0x80
read-mask-bits = 32
sources = ["ROUTE"]
[[blocks.registers]]
name = "CONTROL"
index = 1
reserved = { mask = 4, sources = ["RESERVED"] }
[[blocks.registers.fields]]
name = "ENABLE"
bitOffset = 0
bitWidth = 1
[[blocks.registers.fields]]
name = "BIT_1_OPAQUE"
bitOffset = 1
bitWidth = 1
[[review]]
entity = "PLL.CONTROL.BIT_1_OPAQUE"
sources = ["DIRECT_USE"]
provenance = "reviewed"
accuracy = "positional"
completeness = "unknown"
naming = "opaque"
"#
    .to_owned()
}

fn parse(text: &str) -> Result<AnalogSpace> {
    AnalogSpace::parse(text, NamingRequirement::Complete)
}

#[test]
fn four_states_and_unknown_reset_do_not_invent_hardware_knowledge() {
    let space = parse(&fixture()).unwrap();
    let geometry = space.geometry("synthetic").unwrap();
    let byte = geometry.first().unwrap();
    assert_eq!(
        byte.address,
        AnalogAddress {
            block: 0x66,
            index: 1
        }
    );
    assert_eq!(byte.identity, "register:synthetic/analog-i2c/0x6601/8");
    assert_eq!(
        byte.fields.first().unwrap().identity,
        "register-field:synthetic/analog-i2c/0x6601/8/0/1"
    );
    assert_eq!(
        byte.states,
        BitStates {
            described: 1,
            opaque: 2,
            reserved: 4,
            absent: 0xf8
        }
    );
    assert_eq!(
        (
            byte.reset_value,
            byte.reset_known_mask,
            byte.read_safe_epoch.as_ref()
        ),
        (None, None, None)
    );
    assert!(space.sources().any(|s| s == "DIRECT_USE"));
}

#[test]
fn invalid_geometry_and_misplaced_reserved_or_unknown_keys_are_rejected() {
    for (from, to) in [
        ("bitWidth = 1", "bitWidth = 9"),
        ("bitOffset = 1", "bitOffset = 0"),
        ("mask = 4", "mask = 1"),
        ("mask = 4", "mask = 256"),
        ("mask = 4", "mask = 0"),
        (
            "bitWidth = 1",
            "bitWidth = 1\nreserved = { mask = 2, sources = [\"NO\"] }",
        ),
        ("bitWidth = 1", "bitWidth = 1\nbitWidht = 1"),
        (
            "name = \"ENABLE\"",
            "name = \"ENABLE\"\nderivedFrom = \"OTHER\"",
        ),
    ] {
        assert!(
            parse(&fixture().replacen(from, to, 1)).is_err(),
            "{from} -> {to}"
        );
    }
}

#[test]
fn reset_knowledge_requires_a_cited_value_and_its_exact_known_mask() {
    let with = |extra: &str| fixture().replace("index = 1", &format!("index = 1\n{extra}"));
    let space = parse(&with("reset-value = 3\nreset-known-mask = 15")).unwrap();
    let byte = space.geometry("synthetic").unwrap().remove(0);
    assert_eq!(
        (byte.reset_value, byte.reset_known_mask),
        (Some(3), Some(15))
    );
    for invalid in [
        "reset-value = 0",
        "reset-known-mask = 15",
        "reset-value = 0\nreset-known-mask = 0",
        "reset-value = 16\nreset-known-mask = 15",
    ] {
        assert!(parse(&with(invalid)).is_err(), "{invalid}");
    }
}

#[test]
fn safety_is_separate_from_readable_access_and_cannot_hide_read_actions() {
    let base = fixture().replace("index = 1", "index = 1\naccess = \"read-only\"");
    assert!(
        parse(&base).unwrap().geometry("synthetic").unwrap()[0]
            .read_safe_epoch
            .is_none()
    );
    let safe = base.replace("index = 1", "index = 1\nread-safe = { epoch = \"cold-idle-before-calibration\", sources = [\"SAFETY\"] }");
    assert!(
        parse(&safe).unwrap().geometry("synthetic").unwrap()[0]
            .read_safe_epoch
            .is_some()
    );
    assert!(parse(&safe.replace("access = \"read-only\"", "access = \"write-only\"")).is_err());
    assert!(parse(&safe.replace("index = 1", "index = 1\nread-action = \"clear\"")).is_err());
    assert!(parse(&safe.replace("bitWidth = 1", "bitWidth = 1\nreadAction = \"clear\"")).is_err());
    assert!(parse(&safe.replace("cold-idle-before-calibration", "")).is_err());
}

#[test]
fn aliases_are_routes_to_one_physical_block_and_ambiguous_requests_fail() {
    let route = "[[blocks.routes]]\nprofile = \"libphy-normal\"\nrequest-code = 0x62\nhost = 0\nread-select = 0\nread-mask-bits = 32\nsources = [\"ALIAS\"]\n";
    let text = fixture().replace(
        "[[blocks.registers]]",
        &format!("{route}[[blocks.registers]]"),
    );
    let space = parse(&text).unwrap();
    assert_eq!(space.blocks.len(), 1);
    assert_eq!(space.blocks[0].routes.len(), 2);
    assert_eq!(space.geometry("synthetic").unwrap().len(), 1);
    assert!(parse(&text.replace("request-code = 0x62", "request-code = 0x66")).is_err());
    assert!(parse(&text.replace("read-select = 0x80", "read-select = 0x81")).is_err());
    assert!(parse(&text.replace("host = 1", "host = 2")).is_err());
}

#[test]
fn byte_identities_and_capture_masks_do_not_invent_data_semantics_or_reset() {
    let mut text = fixture()
        .split("[[blocks.registers.fields]]")
        .next()
        .unwrap()
        .to_owned();
    text = text.replace("reserved = { mask = 4, sources = [\"RESERVED\"] }", "access = \"read-only\"\nread-safe = { epoch = \"qualified-idle\", comparison-mask = 255, sources = [\"CAPTURE\"] }");
    let geometry = parse(&text).unwrap().geometry("synthetic").unwrap();
    let byte = &geometry[0];
    assert_eq!(
        (
            byte.states.described,
            byte.states.opaque,
            byte.states.reserved
        ),
        (0, 0, 0)
    );
    assert_eq!(byte.states.absent, 255);
    assert_eq!(byte.reset_value, None);
    assert_eq!(byte.comparison_mask, Some(255));
    assert!(parse(&text.replace("comparison-mask = 255", "comparison-mask = 0")).is_err());
}

#[test]
fn access_views_preserve_the_hardware_field_and_require_a_sourced_strict_subrange() {
    let text = fixture().replace("name = \"ENABLE\"\nbitOffset = 0\nbitWidth = 1", "name = \"ENABLE\"\nbitOffset = 0\nbitWidth = 2")
        .replace("[[blocks.registers.fields]]\nname = \"BIT_1_OPAQUE\"\nbitOffset = 1\nbitWidth = 1", "[[blocks.registers.views]]\nname = \"LOW_ENABLE\"\nfield = \"ENABLE\"\nbit-offset = 0\nbit-width = 1\nsources = [\"MASKED_USE\"]")
        .replace("PLL.CONTROL.BIT_1_OPAQUE", "PLL.CONTROL.LOW_ENABLE")
        .replace("naming = \"opaque\"", "naming = \"descriptive\"");
    let geometry = parse(&text).unwrap().geometry("synthetic").unwrap();
    assert_eq!(geometry[0].fields.len(), 1);
    assert_eq!(geometry[0].views.len(), 1);
    assert_eq!(geometry[0].states.described, 3);
    for (from, to) in [
        ("field = \"ENABLE\"", "field = \"MISSING\""),
        ("bit-width = 1", "bit-width = 2"),
        ("bit-offset = 0", "bit-offset = 2"),
        ("sources = [\"MASKED_USE\"]", "sources = []"),
    ] {
        assert!(parse(&text.replace(from, to)).is_err(), "{from} -> {to}");
    }
}

#[test]
fn review_and_source_failures_never_publish_uncited_fields() {
    for (from, to) in [
        ("sources = [\"HEADER\"]", "sources = []"),
        (
            "sources = [\"HEADER\"]",
            "sources = [\"HEADER\",\"HEADER\"]",
        ),
        ("naming = \"opaque\"", "naming = \"descriptive\""),
        (
            "entity = \"PLL.CONTROL.BIT_1_OPAQUE\"",
            "entity = \"PLL.CONTROL.MISSING\"",
        ),
        ("completeness = \"unknown\"", ""),
        ("provenance = \"reviewed\"", "provenance = \"hint\""),
    ] {
        assert!(
            parse(&fixture().replacen(from, to, 1)).is_err(),
            "{from} -> {to}"
        );
    }
}
