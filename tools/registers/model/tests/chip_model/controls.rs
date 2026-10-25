//! Fixed root relations use synthetic types, addresses and evidence only.

use super::*;
use oer_registers_model::chip_model::ControlKind;

const CLOCK_TYPE: &str = r#"
schema = 3
[type]
name = "CLOCK"
description = "Synthetic clock and reset controller."
sources = ["SRC_HEADER"]
[review]
sources = ["SRC_HEADER"]
provenance = "imported"
accuracy = "exact"
completeness = "complete"
naming = "vendor"

[[registers]]
name = "CONTROL_%s"
offset = 0x10
dim = { count = 2, stride = 4 }
[[registers.fields]]
name = "GATE"
bits = "0"
class = "plain"
[[registers.fields]]
name = "RESET"
bits = "1"
class = "plain"
[[registers.fields]]
name = "SELECT"
bits = "3:2"
class = "plain"
[[registers.fields]]
name = "STATUS"
bits = "4"
class = "read-only"
[[registers.fields]]
name = "ACK"
bits = "5"
class = "write-one-to-clear"

[[groups]]
name = "BANK%s"
offset = 0
count = 2
stride = 4
[[groups.registers]]
name = "CONFIG"
offset = 0
[[groups.registers.fields]]
name = "GATE"
bits = "0"
class = "plain"
"#;

const RELATIONS: &str = r#"
[[instances]]
name = "CR"
type = "CLOCK"
base = 0x6000b000
extent = { size = 0x20 }
sources = ["SRC_BASE"]

[[shared-controls]]
name = "COMMON_GATE"
field = "CR.CONTROL_1.GATE"
instances = ["TIMG0", "TIMG1"]
sources = ["SRC_HEADER"]
[[shared-controls]]
name = "COMMON_RESET"
field = "CR.CONTROL_1.RESET"
instances = ["TIMG0", "TIMG1"]
sources = ["SRC_HEADER"]
[[shared-controls]]
name = "COMMON_MUX"
field = "CR.CONTROL_1.SELECT"
instances = ["TIMG0", "TIMG1"]
sources = ["SRC_HEADER"]

[[gate-groups]]
name = "VENDOR_PAIR"
fields = ["CR.CONTROL_1.GATE", "CR.BANK1.CONFIG.GATE"]
sources = ["SRC_HEADER"]

[[clock-sources]]
name = "OSC"
sources = ["SRC_HEADER"]
[[clock-sources]]
name = "DIVIDED"
parent = "OSC"
frequency-hz = 32000
sources = ["SRC_HEADER"]
"#;

const LINKS: &str = r#"
clocks = [{ role = "bus", field = "CR.CONTROL_1.GATE", sources = ["SRC_HEADER"] }]
resets = [{ role = "group", field = "CR.CONTROL_1.RESET", sources = ["SRC_HEADER"] }]
muxes = [{ role = "timer", field = "CR.CONTROL_1.SELECT", inputs = { 0 = "OSC", 2 = "DIVIDED" }, sources = ["SRC_HEADER"] }]
"#;

fn fixture() -> TestResult<Fixture> {
    let fixture = Fixture::new()?;
    fixture.write("types/clock.toml", CLOCK_TYPE)?;
    fixture.edit(
        "root.toml",
        "\"types/ble.toml\"",
        "\"types/ble.toml\", \"types/clock.toml\"",
    )?;
    for instance in ["TIMG0", "TIMG1"] {
        let name = format!("name = \"{instance}\"\n");
        fixture.edit("root.toml", &name, &format!("{name}{LINKS}"))?;
    }
    let path = fixture.dir.path().join("root.toml");
    let root = fs::read_to_string(&path)?;
    fs::write(path, format!("{root}{RELATIONS}"))?;
    Ok(fixture)
}

#[test]
fn exposes_fixed_links_groups_sources_and_expanded_field_locations() -> TestResult {
    let model = fixture()?.load()?;
    assert_eq!(model.controls().len(), 6);
    for link in model.controls() {
        let (register, _) = link.field.identity.rsplit_once('.').ok_or("no register")?;
        assert_eq!(link.field.address, mmio(&model, register)?);
        let field = model.field(&link.field.identity).ok_or("no linked field")?;
        assert_eq!(
            (link.field.low, link.field.width, link.field.class),
            (field.low, field.width, field.class)
        );
        assert_eq!(link.sources, ["SRC_HEADER"]);
    }
    let mux = model
        .controls()
        .iter()
        .find(|link| link.kind == ControlKind::Mux)
        .ok_or("no mux")?;
    assert_eq!(mux.instance, "TIMG0");
    assert_eq!(mux.role, "timer");
    assert_eq!(mux.field.identity, "CR.CONTROL_1.SELECT");
    assert_eq!(
        mux.inputs
            .iter()
            .map(|(number, name)| (*number, name.as_str()))
            .collect::<Vec<_>>(),
        [(0, "OSC"), (2, "DIVIDED")]
    );
    assert_eq!(model.shared_controls().len(), 3);
    assert_eq!(model.shared_controls()[0].instances, ["TIMG0", "TIMG1"]);
    let group = &model.gate_groups()[0];
    assert_eq!(group.name, "VENDOR_PAIR");
    assert_eq!(group.fields[1].identity, "CR.BANK1.CONFIG.GATE");
    assert_eq!(group.fields[1].address, mmio(&model, "CR.BANK1.CONFIG")?);
    assert_eq!(group.sources, ["SRC_HEADER"]);
    assert_eq!(model.clock_sources()[0].frequency_hz, None);
    assert_eq!(model.clock_sources()[1].parent.as_deref(), Some("OSC"));
    assert_eq!(model.clock_sources()[1].frequency_hz, Some(32000));
    assert_eq!(model.interrupt_sources().count, 30);
    assert_eq!(
        model.interrupts().len() + model.unbound_interrupts().len(),
        30
    );
    assert_eq!(
        model.unbound_interrupts()[0].reason,
        "No owner in the synthetic fixture."
    );
    Ok(())
}

macro_rules! rejects_control {
    ($name:ident, $path:expr, $from:expr, $to:expr, $kind:pat) => {
        #[test]
        fn $name() -> TestResult {
            let fixture = fixture()?;
            fixture.edit($path, $from, $to)?;
            let error = fixture.rejection()?;
            assert!(matches!(error.kind, $kind), "{error}");
            assert!(error.path.ends_with("root.toml"), "{error}");
            Ok(())
        }
    };
}

rejects_control!(
    missing_field,
    "root.toml",
    "field = \"CR.CONTROL_1.GATE\"",
    "field = \"CR.CONTROL_1.NOPE\"",
    ErrorKind::UnknownField(_)
);
rejects_control!(
    template_is_not_a_field_identity,
    "root.toml",
    "field = \"CR.CONTROL_1.GATE\"",
    "field = \"CR.CONTROL_%s.GATE\"",
    ErrorKind::UnknownField(_)
);
rejects_control!(
    analog_field_is_not_a_mmio_control,
    "root.toml",
    "field = \"CR.CONTROL_1.GATE\"",
    "field = \"BBPLL.OC_DIV.DIV\"",
    ErrorKind::UnknownField(_)
);
rejects_control!(
    implicit_shared_gate,
    "root.toml",
    "[[shared-controls]]\nname = \"COMMON_GATE\"\nfield = \"CR.CONTROL_1.GATE\"\ninstances = [\"TIMG0\", \"TIMG1\"]\nsources = [\"SRC_HEADER\"]",
    "",
    ErrorKind::SharedControl(_)
);
rejects_control!(
    implicit_shared_reset,
    "root.toml",
    "[[shared-controls]]\nname = \"COMMON_RESET\"\nfield = \"CR.CONTROL_1.RESET\"\ninstances = [\"TIMG0\", \"TIMG1\"]\nsources = [\"SRC_HEADER\"]",
    "",
    ErrorKind::SharedControl(_)
);
rejects_control!(
    implicit_shared_mux,
    "root.toml",
    "[[shared-controls]]\nname = \"COMMON_MUX\"\nfield = \"CR.CONTROL_1.SELECT\"\ninstances = [\"TIMG0\", \"TIMG1\"]\nsources = [\"SRC_HEADER\"]",
    "",
    ErrorKind::SharedControl(_)
);
rejects_control!(
    shared_with_one_user,
    "root.toml",
    "instances = [\"TIMG0\", \"TIMG1\"]",
    "instances = [\"TIMG0\"]",
    ErrorKind::SharedControl(_)
);
rejects_control!(
    shared_with_duplicate_user,
    "root.toml",
    "instances = [\"TIMG0\", \"TIMG1\"]",
    "instances = [\"TIMG0\", \"TIMG0\"]",
    ErrorKind::SharedControl(_)
);
rejects_control!(
    shared_unknown_user,
    "root.toml",
    "instances = [\"TIMG0\", \"TIMG1\"]",
    "instances = [\"TIMG0\", \"NOPE\"]",
    ErrorKind::UnknownInstance(_)
);
rejects_control!(
    shared_users_must_match,
    "root.toml",
    "instances = [\"TIMG0\", \"TIMG1\"]",
    "instances = [\"TIMG0\", \"CR\"]",
    ErrorKind::SharedControl(_)
);
rejects_control!(
    gate_class,
    "root.toml",
    "field = \"CR.CONTROL_1.GATE\"",
    "field = \"CR.CONTROL_1.ACK\"",
    ErrorKind::ControlField {
        kind: "clock gate",
        ..
    }
);
rejects_control!(
    reset_class,
    "root.toml",
    "field = \"CR.CONTROL_1.RESET\"",
    "field = \"CR.CONTROL_1.STATUS\"",
    ErrorKind::ControlField { kind: "reset", .. }
);
rejects_control!(
    gate_width,
    "root.toml",
    "field = \"CR.CONTROL_1.GATE\"",
    "field = \"CR.CONTROL_0.SELECT\"",
    ErrorKind::ControlField {
        kind: "clock gate",
        ..
    }
);
rejects_control!(
    reset_width,
    "root.toml",
    "field = \"CR.CONTROL_1.RESET\"",
    "field = \"CR.CONTROL_0.SELECT\"",
    ErrorKind::ControlField { kind: "reset", .. }
);
rejects_control!(
    mux_class,
    "root.toml",
    "field = \"CR.CONTROL_1.SELECT\"",
    "field = \"CR.CONTROL_1.STATUS\"",
    ErrorKind::ControlField { kind: "mux", .. }
);
rejects_control!(
    mux_encoding_width,
    "root.toml",
    "2 = \"DIVIDED\"",
    "4 = \"DIVIDED\"",
    ErrorKind::Control(_)
);
rejects_control!(
    mux_encoding_canonical,
    "root.toml",
    "2 = \"DIVIDED\"",
    "02 = \"DIVIDED\"",
    ErrorKind::Control(_)
);
rejects_control!(
    mux_empty_inputs,
    "root.toml",
    "inputs = { 0 = \"OSC\", 2 = \"DIVIDED\" }",
    "inputs = {}",
    ErrorKind::Control(_)
);
rejects_control!(
    mux_unknown_input,
    "root.toml",
    "2 = \"DIVIDED\"",
    "2 = \"NOPE\"",
    ErrorKind::ClockSource(_)
);
rejects_control!(
    mux_maps_disagree,
    "root.toml",
    "2 = \"DIVIDED\"",
    "1 = \"DIVIDED\"",
    ErrorKind::Control(_)
);
rejects_control!(
    clock_unknown_parent,
    "root.toml",
    "parent = \"OSC\"",
    "parent = \"NOPE\"",
    ErrorKind::ClockSource(_)
);
rejects_control!(
    clock_parent_cycle,
    "root.toml",
    "name = \"OSC\"",
    "name = \"OSC\"\nparent = \"DIVIDED\"",
    ErrorKind::ClockSource(_)
);
rejects_control!(
    clock_zero_frequency,
    "root.toml",
    "frequency-hz = 32000",
    "frequency-hz = 0",
    ErrorKind::ClockSource(_)
);
rejects_control!(
    group_empty,
    "root.toml",
    "fields = [\"CR.CONTROL_1.GATE\", \"CR.BANK1.CONFIG.GATE\"]",
    "fields = []",
    ErrorKind::GateGroup(_)
);
rejects_control!(
    group_duplicate_field,
    "root.toml",
    "fields = [\"CR.CONTROL_1.GATE\", \"CR.BANK1.CONFIG.GATE\"]",
    "fields = [\"CR.CONTROL_1.GATE\", \"CR.CONTROL_1.GATE\"]",
    ErrorKind::GateGroup(_)
);
rejects_control!(
    group_missing_field,
    "root.toml",
    "\"CR.BANK1.CONFIG.GATE\"",
    "\"CR.BANK2.CONFIG.GATE\"",
    ErrorKind::UnknownField(_)
);
rejects_control!(
    group_class,
    "root.toml",
    "\"CR.BANK1.CONFIG.GATE\"",
    "\"CR.CONTROL_1.STATUS\"",
    ErrorKind::ControlField { .. }
);
rejects_control!(
    unbound_duplicate_number,
    "root.toml",
    "source = 1\nname = \"UNBOUND_1\"",
    "source = 0\nname = \"UNBOUND_1\"",
    ErrorKind::DuplicateSourceNumber(0)
);
rejects_control!(
    unbound_bound_overlap,
    "root.toml",
    "source = 0\nname = \"UNBOUND_0\"",
    "source = 10\nname = \"UNBOUND_0\"",
    ErrorKind::DuplicateSourceNumber(10)
);
rejects_control!(
    unbound_duplicate_name,
    "root.toml",
    "name = \"UNBOUND_1\"",
    "name = \"UNBOUND_0\"",
    ErrorKind::Duplicate(_)
);
rejects_control!(
    unbound_bound_name_overlap,
    "root.toml",
    "name = \"UNBOUND_0\"",
    "name = \"TG0_T0\"",
    ErrorKind::Duplicate(_)
);
rejects_control!(
    unbound_blank_reason,
    "root.toml",
    "reason = \"No owner in the synthetic fixture.\"",
    "reason = \" \"",
    ErrorKind::InterruptCoverage(_)
);
rejects_control!(
    unbound_missing_source,
    "root.toml",
    "[[unbound-interrupts]]\nsource = 0\nname = \"UNBOUND_0\"\nreason = \"No owner in the synthetic fixture.\"\nsources = [\"SRC_HEADER\"]",
    "",
    ErrorKind::MissingInterruptSource(0)
);
rejects_control!(
    bound_outside_source_range,
    "root.toml",
    "count = 30",
    "count = 29",
    ErrorKind::InterruptCoverage(_)
);
rejects_control!(
    unbound_outside_source_range,
    "root.toml",
    "source = 0\nname = \"UNBOUND_0\"",
    "source = 30\nname = \"UNBOUND_0\"",
    ErrorKind::InterruptCoverage(_)
);
rejects_control!(
    source_count_overflow,
    "root.toml",
    "count = 30",
    "count = 65537",
    ErrorKind::InterruptCoverage(_)
);
rejects_control!(
    link_empty_sources,
    "root.toml",
    "field = \"CR.CONTROL_1.GATE\", sources = [\"SRC_HEADER\"]",
    "field = \"CR.CONTROL_1.GATE\", sources = []",
    ErrorKind::InvalidSources
);
rejects_control!(
    root_rejects_power_domains,
    "root.toml",
    "chip = \"testchip\"",
    "chip = \"testchip\"\npower-domains = []",
    ErrorKind::Parse(_)
);
rejects_control!(
    instance_rejects_parents,
    "root.toml",
    "name = \"TIMG0\"",
    "name = \"TIMG0\"\nparents = [\"MODEM_SUBSYSTEM\"]",
    ErrorKind::Parse(_)
);
rejects_control!(
    instance_rejects_ordering,
    "root.toml",
    "name = \"TIMG0\"",
    "name = \"TIMG0\"\nsequence = []",
    ErrorKind::Parse(_)
);

rejects_control!(
    shared_duplicate_name,
    "root.toml",
    "name = \"COMMON_RESET\"",
    "name = \"COMMON_GATE\"",
    ErrorKind::SharedControl(_)
);
rejects_control!(
    shared_duplicate_field,
    "root.toml",
    "name = \"COMMON_RESET\"\nfield = \"CR.CONTROL_1.RESET\"",
    "name = \"COMMON_RESET\"\nfield = \"CR.CONTROL_1.GATE\"",
    ErrorKind::SharedControl(_)
);
rejects_control!(
    shared_invalid_name,
    "root.toml",
    "name = \"COMMON_GATE\"",
    "name = \"common-gate\"",
    ErrorKind::InvalidName(_)
);
rejects_control!(
    group_invalid_name,
    "root.toml",
    "name = \"VENDOR_PAIR\"",
    "name = \"vendor-pair\"",
    ErrorKind::InvalidName(_)
);
rejects_control!(
    group_duplicate_name,
    "root.toml",
    "[[clock-sources]]",
    "[[gate-groups]]\nname = \"VENDOR_PAIR\"\nfields = [\"CR.CONTROL_1.GATE\"]\nsources = [\"SRC_HEADER\"]\n\n[[clock-sources]]",
    ErrorKind::Duplicate(_)
);
rejects_control!(
    clock_duplicate_name,
    "root.toml",
    "name = \"DIVIDED\"",
    "name = \"OSC\"",
    ErrorKind::Duplicate(_)
);
rejects_control!(
    clock_invalid_name,
    "root.toml",
    "name = \"OSC\"",
    "name = \"osc\"",
    ErrorKind::InvalidName(_)
);
rejects_control!(
    invalid_role,
    "root.toml",
    "role = \"bus\"",
    "role = \"Bus\"",
    ErrorKind::Control(_)
);
rejects_control!(
    empty_role,
    "root.toml",
    "role = \"bus\"",
    "role = \"\"",
    ErrorKind::Control(_)
);
rejects_control!(
    duplicate_role,
    "root.toml",
    "clocks = [{ role = \"bus\"",
    "clocks = [{ role = \"bus\", field = \"CR.CONTROL_0.GATE\", sources = [\"SRC_HEADER\"] }, { role = \"bus\"",
    ErrorKind::Control(_)
);

rejects_control!(
    missing_source_inventory,
    "root.toml",
    "interrupt-sources = { count = 30, sources = [\"SRC_HEADER\"] }",
    "",
    ErrorKind::Parse(_)
);

#[test]
fn all_relation_source_lists_are_checked() -> TestResult {
    for (from, entity) in [
        (
            "interrupt-sources = { count = 30, sources = [\"SRC_HEADER\"] }",
            "interrupt-sources",
        ),
        (
            "field = \"CR.CONTROL_1.GATE\", sources = [\"SRC_HEADER\"]",
            "TIMG0.Clock.bus",
        ),
        (
            "field = \"CR.CONTROL_1.RESET\", sources = [\"SRC_HEADER\"]",
            "TIMG0.Reset.group",
        ),
        (
            "inputs = { 0 = \"OSC\", 2 = \"DIVIDED\" }, sources = [\"SRC_HEADER\"]",
            "TIMG0.Mux.timer",
        ),
        (
            "instances = [\"TIMG0\", \"TIMG1\"]\nsources = [\"SRC_HEADER\"]",
            "COMMON_GATE",
        ),
        (
            "fields = [\"CR.CONTROL_1.GATE\", \"CR.BANK1.CONFIG.GATE\"]\nsources = [\"SRC_HEADER\"]",
            "VENDOR_PAIR",
        ),
        ("name = \"OSC\"\nsources = [\"SRC_HEADER\"]", "OSC"),
        (
            "reason = \"No owner in the synthetic fixture.\"\nsources = [\"SRC_HEADER\"]",
            "UNBOUND_0",
        ),
    ] {
        for (sources, expected_unknown) in [
            ("[]", false),
            ("[\"SRC_HEADER\", \"SRC_HEADER\"]", false),
            ("[\"NOPE\"]", true),
        ] {
            let fixture = fixture()?;
            fixture.edit(
                "root.toml",
                from,
                &from.replace("[\"SRC_HEADER\"]", sources),
            )?;
            let error = fixture.rejection()?;
            if expected_unknown {
                assert!(matches!(error.kind, ErrorKind::UnknownSource(_)), "{error}");
            } else {
                assert!(matches!(error.kind, ErrorKind::InvalidSources), "{error}");
            }
            assert_eq!(error.entity, entity);
        }
    }
    Ok(())
}

#[test]
fn empty_interrupt_space_and_no_controls_load_without_inferred_facts() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write(
        "root.toml",
        r#"
schema = 4
chip = "testchip"
types = []
interrupt-sources = { count = 0, sources = ["SRC_HEADER"] }
"#,
    )?;
    let model = fixture.load()?;
    assert!(model.controls().is_empty());
    assert!(model.shared_controls().is_empty());
    assert!(model.gate_groups().is_empty());
    assert!(model.clock_sources().is_empty());
    assert!(model.interrupts().is_empty());
    assert!(model.unbound_interrupts().is_empty());
    Ok(())
}

#[test]
fn multiple_roles_of_one_instance_are_one_shared_control_user() -> TestResult {
    let fixture = fixture()?;
    fixture.edit(
        "root.toml",
        "clocks = [{ role = \"bus\"",
        "clocks = [{ role = \"other\", field = \"CR.CONTROL_1.GATE\", sources = [\"SRC_HEADER\"] }, { role = \"bus\"",
    )?;
    let model = fixture.load()?;
    let gate = &model.shared_controls()[0];
    assert_eq!(gate.instances, ["TIMG0", "TIMG1"]);
    assert_eq!(
        model
            .controls()
            .iter()
            .filter(|control| control.field.identity == gate.field)
            .count(),
        3
    );
    Ok(())
}

#[test]
fn controls_and_registers_share_the_chip_record_budget() -> TestResult {
    for list in ["clocks", "resets", "muxes"] {
        let fixture = fixture()?;
        fixture.edit(
            "types/ble.toml",
            "name = \"EVENT_STATUS\"\noffset = 0x10",
            "name = \"EVENT_STATUS_%s\"\noffset = 0x10\ndim = { count = 16000, stride = 4 }",
        )?;
        fixture.edit("root.toml", "size = 0x20,", "size = 0x10000,")?;
        fixture.edit("root.toml", "base = 0x6000b000", "base = 0x60020000")?;
        let entries = (0..17000)
            .map(|index| {
                let (field, inputs) = if list == "muxes" {
                    ("SELECT", ", inputs = { 0 = \"OSC\" }")
                } else {
                    ("GATE", "")
                };
                format!(
                    "{{ role = \"role-{index}\", field = \"CR.CONTROL_1.{field}\", sources = [\"SRC_HEADER\"]{inputs} }}"
                )
            })
            .collect::<Vec<_>>()
            .join(",\n");
        let original = LINKS
            .lines()
            .find(|line| line.starts_with(list))
            .ok_or("no list")?;
        fixture.edit("root.toml", original, &format!("{list} = [{entries}]"))?;
        let error = fixture.rejection()?;
        assert!(
            matches!(error.kind, ErrorKind::ExpansionLimit(_)),
            "{error}"
        );
        assert_eq!(error.entity, "TIMG0");
    }
    Ok(())
}

#[test]
fn copied_instance_names_share_the_chip_text_budget() -> TestResult {
    let fixture = fixture()?;
    let name = format!("TIMG_{}", "A".repeat(8192));
    let root = fs::read_to_string(fixture.dir.path().join("root.toml"))?;
    fixture.write("root.toml", &root.replace("TIMG0", &name))?;
    let entries = (0..8192)
        .map(|index| {
            format!("{{ role = \"role-{index}\", field = \"CR.CONTROL_1.GATE\", sources = [\"SRC_HEADER\"] }}")
        })
        .collect::<Vec<_>>()
        .join(",\n");
    fixture.edit(
        "root.toml",
        "clocks = [{ role = \"bus\", field = \"CR.CONTROL_1.GATE\", sources = [\"SRC_HEADER\"] }]",
        &format!("clocks = [{entries}]"),
    )?;
    let error = fixture.rejection()?;
    assert!(
        matches!(error.kind, ErrorKind::ExpansionLimit(_)),
        "{error}"
    );
    assert_eq!(error.entity, name);
    Ok(())
}

#[test]
fn root_relation_collections_are_bounded_before_validation() -> TestResult {
    for (from, to) in [
        (
            "fields = [\"CR.CONTROL_1.GATE\", \"CR.BANK1.CONFIG.GATE\"]",
            format!("fields = [{}]", "\"CR.CONTROL_1.GATE\",".repeat(65536)),
        ),
        (
            "instances = [\"TIMG0\", \"TIMG1\"]",
            format!("instances = [{}]", "\"TIMG0\",".repeat(65536)),
        ),
        (
            "[[clock-sources]]",
            "[[clock-sources]]\nname = \"OSC\"\nsources = [\"SRC_HEADER\"]\n".repeat(65536)
                + "[[clock-sources]]\n",
        ),
        (
            "inputs = { 0 = \"OSC\", 2 = \"DIVIDED\" }",
            format!(
                "inputs = {{ {} }}",
                (0..65536)
                    .map(|index| format!("{index} = \"OSC\""))
                    .collect::<Vec<_>>()
                    .join(",")
            ),
        ),
    ] {
        let fixture = fixture()?;
        fixture.edit("root.toml", from, &to)?;
        let error = fixture.rejection()?;
        assert!(
            matches!(error.kind, ErrorKind::ExpansionLimit(_)),
            "{error}"
        );
    }
    Ok(())
}

#[test]
fn full_width_mux_accepts_u32_max_and_rejects_larger_encodings() -> TestResult {
    for encoding in ["4294967295", "4294967296", "18446744073709551616"] {
        let fixture = Fixture::new()?;
        fixture.write(
            "types/clock.toml",
            r#"
schema = 3
[type]
name = "CLOCK"
description = "Synthetic full-width selector."
sources = ["SRC_HEADER"]
[review]
sources = ["SRC_HEADER"]
provenance = "imported"
accuracy = "exact"
completeness = "complete"
naming = "vendor"
[[registers]]
name = "CONTROL"
offset = 0
[[registers.fields]]
name = "SELECT"
bits = "31:0"
class = "plain"
"#,
        )?;
        fixture.write("root.toml", &format!(r#"
schema = 4
chip = "testchip"
types = ["types/clock.toml"]
interrupt-sources = {{ count = 0, sources = ["SRC_HEADER"] }}
[[instances]]
name = "CR"
type = "CLOCK"
base = 0
extent = {{ size = 4 }}
sources = ["SRC_BASE"]
muxes = [{{ role = "clock", field = "CR.CONTROL.SELECT", inputs = {{ {encoding} = "OSC" }}, sources = ["SRC_HEADER"] }}]
[[clock-sources]]
name = "OSC"
sources = ["SRC_HEADER"]
"#))?;
        if encoding == "4294967295" {
            let model = fixture.load()?;
            assert_eq!(
                model.controls()[0]
                    .inputs
                    .get(&u32::MAX)
                    .map(String::as_str),
                Some("OSC")
            );
        } else {
            let error = fixture.rejection()?;
            assert!(matches!(error.kind, ErrorKind::Control(_)), "{error}");
        }
    }
    Ok(())
}

#[test]
fn maximum_interrupt_source_is_checked_without_truncating_the_range() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write(
        "root.toml",
        "schema = 4\nchip = \"testchip\"\ntypes = []\ninterrupt-sources = { count = 65536, sources = [\"SRC_HEADER\"] }\n",
    )?;
    let error = fixture.rejection()?;
    assert!(
        matches!(error.kind, ErrorKind::MissingInterruptSource(0)),
        "{error}"
    );
    let mut root = fs::read_to_string(fixture.dir.path().join("root.toml"))?;
    for source in 0..65535 {
        root.push_str(&format!("[[unbound-interrupts]]\nsource = {source}\nname = \"IRQ_{source}\"\nreason = \"Synthetic source.\"\nsources = [\"SRC_HEADER\"]\n"));
    }
    fixture.write("root.toml", &root)?;
    let error = fixture.rejection()?;
    assert!(
        matches!(error.kind, ErrorKind::MissingInterruptSource(u16::MAX)),
        "{error}"
    );
    root.push_str("[[unbound-interrupts]]\nsource = 65535\nname = \"IRQ_LAST\"\nreason = \"Synthetic source.\"\nsources = [\"SRC_HEADER\"]\n");
    fixture.write("root.toml", &root)?;
    assert_eq!(fixture.load()?.unbound_interrupts().len(), 65536);
    Ok(())
}

#[test]
fn clock_parent_chains_share_checked_ancestors_and_reject_disconnected_cycles() -> TestResult {
    let fixture = Fixture::new()?;
    let mut root = String::from(
        "schema = 4\nchip = \"testchip\"\ntypes = []\ninterrupt-sources = { count = 0, sources = [\"SRC_HEADER\"] }\n",
    );
    for index in 0..1024 {
        root.push_str(&format!("[[clock-sources]]\nname = \"CLOCK_{index}\"\nparent = \"CLOCK_{}\"\nsources = [\"SRC_HEADER\"]\n", index + 1));
    }
    root.push_str("[[clock-sources]]\nname = \"CLOCK_1024\"\nsources = [\"SRC_HEADER\"]\n[[clock-sources]]\nname = \"BRANCH\"\nparent = \"CLOCK_512\"\nsources = [\"SRC_HEADER\"]\n");
    fixture.write("root.toml", &root)?;
    assert_eq!(fixture.load()?.clock_sources().len(), 1026);
    root.push_str(
        "[[clock-sources]]\nname = \"CYCLE\"\nparent = \"CYCLE\"\nsources = [\"SRC_HEADER\"]\n",
    );
    fixture.write("root.toml", &root)?;
    let error = fixture.rejection()?;
    assert!(matches!(error.kind, ErrorKind::ClockSource(_)), "{error}");
    assert_eq!(error.entity, "CYCLE");
    Ok(())
}
