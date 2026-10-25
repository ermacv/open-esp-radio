//! The block-type / chip-root model on synthetic fixtures: one chip that
//! uses every construct, and one rejection per load-time rule.

use std::{error::Error as StdError, fs};

use oer_registers_contracts::{FactAccuracy, NameOrigin};
use oer_registers_model::{
    RegisterEvidenceSet,
    analog::AnalogAddress,
    chip_model::{ChipModel, Error, ErrorKind, FieldClass, Location},
};

type TestResult<T = ()> = Result<T, Box<dyn StdError>>;

const EVIDENCE: &str = r#"
schema = 1

[[sources]]
id = "SRC_HEADER"
description = "Synthetic register header."

[[sources]]
id = "SRC_USE"
description = "Synthetic observed use."

[[sources]]
id = "SRC_BASE"
description = "Synthetic base address list."

[[sources]]
id = "SRC_BLOB"
description = "Synthetic vendor-binary observation."
"#;

const TIMG: &str = r#"
schema = 3

[type]
name = "TIMG"
description = "Synthetic timer group."
sources = ["SRC_HEADER"]

[[interrupt-outputs]]
name = "TIMER"
sources = ["SRC_HEADER"]

[[interrupt-outputs]]
name = "WDT"
sources = ["SRC_HEADER"]

[review]
sources = ["SRC_HEADER"]
provenance = "imported"
accuracy = "exact"
completeness = "complete"
naming = "vendor"

[review.opaque]
sources = ["SRC_USE"]
provenance = "observed"
accuracy = "positional"
completeness = "unknown"

[review.entity."INT_RAW.T0"]
accuracy = "bounded"

[[groups]]
name = "T%s"
offset = 0x00
count = 2
stride = 0x24

[[groups.registers]]
name = "CONFIG"
offset = 0x00
reset = { value = 0x80000000 }

[[groups.registers.fields]]
name = "EN"
bits = "31"
class = "plain"

[[groups.registers]]
name = "LOAD"
offset = 0x20

[[groups.registers.fields]]
name = "LOAD"
bits = "0"
class = "write-trigger"

[[registers]]
name = "INT_RAW"
offset = 0x70
reserved = { mask = 0xfffffffc, sources = ["SRC_HEADER"] }

[[registers.fields]]
name = "T0"
bits = "0"
class = "write-one-to-clear"

[[registers.fields]]
name = "T1"
bits = "1"
class = "write-one-to-clear"

[[registers]]
name = "WORD_0080_OPAQUE"
offset = 0x80

[[registers.fields]]
name = "BITS_0_7_OPAQUE"
bits = "7:0"
class = "plain"

[[registers]]
name = "REGCLK_%s"
offset = 0x90
dim = { count = 2, stride = 4 }

[[registers.fields]]
name = "DIV"
bits = "7:0"
class = "plain"
write-range = { min = 1, max = 255 }
values = [{ name = "BYPASS", value = 1 }]

[[registers]]
name = "CMD"
offset = 0xa0

[[registers.fields]]
name = "READ"
bits = "0"
class = "self-clearing"

[[registers.fields]]
name = "ERASE"
bits = "1"
class = "self-clearing"

[[registers.fields]]
name = "BUSY"
bits = "2"
class = "self-clearing"

[[registers.fields]]
name = "WHOLE_CHIP"
bits = "3"
class = "self-clearing"

[[registers.fields]]
name = "STATE"
bits = "7:4"
class = "read-only"

[[registers]]
name = "FIFO"
offset = 0xb0
fifo = { sources = ["SRC_HEADER"] }

[[registers.fields]]
name = "DATA"
bits = "7:0"
class = "plain"

[[registers]]
name = "DATA_PORT"
offset = 0xb4
data-port = { sources = ["SRC_HEADER"] }

[[registers.fields]]
name = "DATA"
bits = "31:0"
class = "hardware-written"

[[commands]]
register = "CMD"
commands = ["READ", "ERASE"]
qualifiers = ["WHOLE_CHIP"]
held = ["BUSY"]
sources = ["SRC_HEADER"]
"#;

const BLE: &str = r#"
schema = 3

[type]
name = "BLE"
description = "Synthetic block known only from vendor binaries."
sources = ["SRC_BLOB"]

[[interrupt-outputs]]
name = "EVENT"
sources = ["SRC_BLOB"]

[review]
sources = ["SRC_BLOB"]
provenance = "observed"
accuracy = "exact"
completeness = "partial"
naming = "descriptive"

[[registers]]
name = "EVENT_STATUS"
offset = 0x10

[[registers.fields]]
name = "PENDING"
bits = "0"
class = "read-only"
"#;

const BBPLL: &str = r#"
schema = 3

[type]
name = "BBPLL"
description = "Synthetic analog PLL block."
space = "analog-i2c"
sources = ["SRC_HEADER"]

[review]
sources = ["SRC_HEADER"]
provenance = "imported"
accuracy = "exact"
completeness = "complete"
naming = "vendor"

[[registers]]
name = "OC_DIV"
offset = 0x02
read-safe = { epoch = "calibrated", comparison-mask = 0x3f, sources = ["SRC_USE"] }

[[registers.fields]]
name = "DIV"
bits = "5:0"
class = "plain"

[[registers.views]]
name = "DIV_LOW"
field = "DIV"
bits = "2:0"
sources = ["SRC_USE"]
"#;

const LAYOUT: &str = r#"
schema = 1

[layout]
name = "RX_DESCRIPTOR"
description = "Synthetic DMA descriptor."
size = 8
align = 4

[layout.review]
sources = ["SRC_HEADER"]
provenance = "reviewed"
accuracy = "exact"
completeness = "complete"
naming = "descriptive"

[[layout.words]]
name = "CONTROL"
offset = 0x00
access = "shared"

[[layout.words.fields]]
name = "OWNER"
bits = "31"
writer = "cpu"
description = "Who holds the descriptor."

[[layout.words]]
name = "BUFFER"
offset = 0x04
access = "shared"

[[layout.words.fields]]
name = "ADDRESS"
bits = "31:0"
writer = "cpu"
description = "The buffer's address."
placement = ["dma-window"]
"#;

const ROOT: &str = r#"
schema = 4
chip = "testchip"
interrupt-sources = { count = 30, sources = ["SRC_HEADER"] }
types = ["types/timg.toml", "types/ble.toml", "types/bbpll.toml"]
layouts = ["layouts/rx-descriptor.toml"]

[[instances]]
name = "TIMG0"
type = "TIMG"
base = 0x60008000
extent = { size = 0x1000 }
sources = ["SRC_BASE"]

[[instances]]
name = "TIMG1"
type = "TIMG"
base = 0x60009000
extent = { size = 0x1000 }
sources = ["SRC_BASE"]

[[instances]]
name = "BLE"
type = "BLE"
base = 0x6000a000
extent = { size = 0x20, partial = true, sources = ["SRC_BLOB"] }
sources = ["SRC_BLOB"]

[[interrupts]]
name = "TG0_T0"
source = 10
instance = "TIMG0"
output = "TIMER"
sources = ["SRC_HEADER"]

[[interrupts]]
name = "TG1_WDT"
source = 11
instance = "TIMG1"
output = "WDT"
sources = ["SRC_HEADER"]

[[interrupts]]
name = "BLE_EVENT"
source = 20
instance = "BLE"
output = "EVENT"
sources = ["SRC_BLOB"]

[[interrupts]]
name = "BLE_EVENT_ALT"
source = 29
instance = "BLE"
output = "EVENT"
sources = ["SRC_BLOB"]

[[analog-i2c.instances]]
name = "BBPLL"
type = "BBPLL"
code = 0x66
sources = ["SRC_HEADER"]

[[analog-i2c.instances.routes]]
profile = "phy"
request-code = 0x66
host = 0
read-select = 0
read-mask-bits = 32
sources = ["SRC_HEADER"]

[[ram-regions]]
name = "dma-window"
base = 0x40800000
size = 0x10000
sources = ["SRC_HEADER"]

[[layout-placements]]
layout = "RX_DESCRIPTOR"
regions = ["dma-window"]
"#;

const TIMG0_BASE: u64 = 0x6000_8000;
const TIMG1_BASE: u64 = 0x6000_9000;
const GROUP_STRIDE: u64 = 0x24;
const REGCLK_OFFSET: u64 = 0x90;
const REGCLK_STRIDE: u64 = 4;
const BBPLL_CODE: u8 = 0x66;
const OC_DIV_INDEX: u8 = 0x02;

/// One fixture chip in its own temporary directory.
struct Fixture {
    dir: tempfile::TempDir,
}

impl Fixture {
    fn new() -> TestResult<Self> {
        let fixture = Self {
            dir: tempfile::tempdir()?,
        };
        for (path, text) in [
            ("evidence.toml", EVIDENCE),
            ("root.toml", ROOT),
            ("types/timg.toml", TIMG),
            ("types/ble.toml", BLE),
            ("types/bbpll.toml", BBPLL),
            ("layouts/rx-descriptor.toml", LAYOUT),
        ] {
            fixture.write(path, text)?;
        }
        let mut root = ROOT.to_owned();
        for source in (0..30).filter(|source| ![10, 11, 20, 29].contains(source)) {
            root.push_str(&format!(
                r#"
[[unbound-interrupts]]
source = {source}
name = "UNBOUND_{source}"
reason = "No owner in the synthetic fixture."
sources = ["SRC_HEADER"]
"#
            ));
        }
        fixture.write("root.toml", &root)?;
        Ok(fixture)
    }

    fn write(&self, path: &str, text: &str) -> TestResult {
        let path = self.dir.path().join(path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, text)?;
        Ok(())
    }

    /// Replace the first `from` in `path` with `to`; `from` must be there.
    fn edit(&self, path: &str, from: &str, to: &str) -> TestResult<&Self> {
        let full = self.dir.path().join(path);
        let text = fs::read_to_string(&full)?;
        if !text.contains(from) {
            return Err(format!("{path} has no {from:?}").into());
        }
        fs::write(full, text.replacen(from, to, 1))?;
        Ok(self)
    }

    fn evidence(&self) -> TestResult<RegisterEvidenceSet> {
        Ok(RegisterEvidenceSet::load_all(&[self
            .dir
            .path()
            .join("evidence.toml")])?)
    }

    fn load(&self) -> TestResult<ChipModel> {
        Ok(ChipModel::load(
            &self.dir.path().join("root.toml"),
            &self.evidence()?,
        )?)
    }

    fn rejection(&self) -> TestResult<Error> {
        match ChipModel::load(&self.dir.path().join("root.toml"), &self.evidence()?) {
            Ok(_) => Err("the fixture loaded".into()),
            Err(error) => Ok(error),
        }
    }
}

/// The fixture after one edit, rejected: its error.
fn rejected(path: &str, from: &str, to: &str) -> TestResult<Error> {
    let fixture = Fixture::new()?;
    fixture.edit(path, from, to)?;
    fixture.rejection()
}

fn mmio(model: &ChipModel, identity: &str) -> TestResult<u64> {
    match model.register(identity).map(|register| register.location) {
        Some(Location::Mmio { address }) => Ok(address),
        other => Err(format!("{identity} is at {other:?}").into()),
    }
}

#[test]
fn expands_two_instances_of_one_type_groups_and_dim() -> TestResult {
    let model = Fixture::new()?.load()?;
    assert_eq!(model.chip(), "testchip");
    assert_eq!(mmio(&model, "TIMG0.T0.CONFIG")?, TIMG0_BASE);
    assert_eq!(mmio(&model, "TIMG1.T1.CONFIG")?, TIMG1_BASE + GROUP_STRIDE);
    assert_eq!(
        mmio(&model, "TIMG0.REGCLK_1")?,
        TIMG0_BASE + REGCLK_OFFSET + REGCLK_STRIDE
    );
    let en = model.field("TIMG1.T1.CONFIG.EN").ok_or("no T1 enable")?;
    assert_eq!((en.low, en.width, en.class), (31, 1, FieldClass::Plain));
    let config = model.register("TIMG1.T1.CONFIG").ok_or("no T1 config")?;
    assert_eq!(config.key, "T%s.CONFIG");
    assert_eq!(config.instance, "TIMG1");
    let timg0 = model
        .registers()
        .iter()
        .filter(|register| register.instance == "TIMG0")
        .count();
    let timg1 = model
        .registers()
        .iter()
        .filter(|register| register.instance == "TIMG1")
        .count();
    // 2 group elements of 2 registers, INT_RAW, the opaque word, 2 REGCLK
    // elements, CMD, FIFO and DATA_PORT.
    assert_eq!((timg0, timg1), (11, 11));
    Ok(())
}

#[test]
fn keeps_classes_reserved_bits_and_commands() -> TestResult {
    let model = Fixture::new()?.load()?;
    let int_raw = model.register("TIMG0.INT_RAW").ok_or("no INT_RAW")?;
    assert!(
        int_raw
            .fields
            .iter()
            .all(|field| field.class == FieldClass::WriteOneToClear)
    );
    assert_ne!(int_raw.reserved, 0);
    let load = model.field("TIMG0.T0.LOAD.LOAD").ok_or("no trigger")?;
    assert_eq!(load.class, FieldClass::WriteTrigger);
    let command = &model.types().get("TIMG").ok_or("no TIMG")?.file.commands;
    assert_eq!(command.len(), 1);
    Ok(())
}

#[test]
fn reviews_by_exception() -> TestResult {
    let model = Fixture::new()?.load()?;
    let overridden = model.field("TIMG0.INT_RAW.T0").ok_or("no T0")?;
    assert_eq!(overridden.review.accuracy, Some(FactAccuracy::Bounded));
    assert_eq!(overridden.review.naming, Some(NameOrigin::Vendor));
    let default = model.field("TIMG1.INT_RAW.T1").ok_or("no T1")?;
    assert_eq!(default.review.accuracy, Some(FactAccuracy::Exact));
    assert_eq!(default.review.entity, "TIMG1.INT_RAW.T1");
    let opaque = model
        .register("TIMG0.WORD_0080_OPAQUE")
        .ok_or("no opaque word")?;
    assert_eq!(opaque.review.naming, Some(NameOrigin::Opaque));
    assert_eq!(opaque.review.sources, ["SRC_USE"]);
    Ok(())
}

#[test]
fn binds_interrupts_and_several_sources_to_one_output() -> TestResult {
    let model = Fixture::new()?.load()?;
    let ble = model
        .interrupts()
        .iter()
        .filter(|interrupt| {
            interrupt.instance == "BLE" && interrupt.output.as_deref() == Some("EVENT")
        })
        .count();
    assert_eq!(ble, 2);
    let instance = model
        .instances()
        .iter()
        .find(|instance| instance.name == "BLE")
        .ok_or("no BLE")?;
    assert!(instance.extent.partial);
    Ok(())
}

#[test]
fn addresses_analog_bytes_by_block_code() -> TestResult {
    let model = Fixture::new()?.load()?;
    let register = model.register("BBPLL.OC_DIV").ok_or("no OC_DIV")?;
    assert_eq!(
        register.location,
        Location::AnalogI2c(AnalogAddress {
            block: BBPLL_CODE,
            index: OC_DIV_INDEX,
        })
    );
    assert_eq!(register.width, 8);
    assert_eq!(model.analog_instances().len(), 1);
    Ok(())
}

#[test]
fn places_layouts_from_the_root() -> TestResult {
    let model = Fixture::new()?.load()?;
    let layout = model
        .layouts()
        .iter()
        .find(|layout| layout.name == "RX_DESCRIPTOR")
        .ok_or("no layout")?;
    assert_eq!(layout.placement, ["dma-window"]);
    assert!(layout.is_dma_target());
    Ok(())
}

#[test]
fn errors_name_the_file_and_the_entity() -> TestResult {
    let error = rejected(
        "types/timg.toml",
        "name = \"T1\"\nbits = \"1\"",
        "name = \"T1\"\nbits = \"0\"",
    )?;
    assert!(
        matches!(error.kind, ErrorKind::FieldOverlap(ref other) if other == "T0"),
        "{error}"
    );
    assert!(error.path.ends_with("types/timg.toml"), "{error}");
    assert_eq!(error.entity, "TIMG.INT_RAW.T1");
    Ok(())
}

macro_rules! rejects {
    ($name:ident, $path:expr, $from:expr, $to:expr, $kind:pat) => {
        #[test]
        fn $name() -> TestResult {
            let error = rejected($path, $from, $to)?;
            assert!(matches!(error.kind, $kind), "{error}");
            Ok(())
        }
    };
}

// Files and includes.
rejects!(
    type_schema,
    "types/timg.toml",
    "schema = 3",
    "schema = 2",
    ErrorKind::Schema { .. }
);
rejects!(
    root_schema,
    "root.toml",
    "schema = 4",
    "schema = 3",
    ErrorKind::Schema { .. }
);
rejects!(
    unknown_key,
    "types/timg.toml",
    "class = \"read-only\"",
    "class = \"read-only\"\naccess = \"read-write\"",
    ErrorKind::Parse(_)
);
rejects!(
    unknown_class,
    "types/timg.toml",
    "class = \"read-only\"",
    "class = \"read-write\"",
    ErrorKind::Parse(_)
);
rejects!(
    unsafe_include,
    "root.toml",
    "\"types/ble.toml\"",
    "\"../ble.toml\"",
    ErrorKind::UnsafePath(_)
);
rejects!(
    duplicate_include,
    "root.toml",
    "\"types/ble.toml\"",
    "\"types/timg.toml\"",
    ErrorKind::DuplicateInclude(_)
);
rejects!(
    missing_include,
    "root.toml",
    "\"types/ble.toml\"",
    "\"types/absent.toml\"",
    ErrorKind::Read(_)
);
rejects!(
    missing_description,
    "types/ble.toml",
    "description = \"Synthetic block known only from vendor binaries.\"",
    "description = \" \"",
    ErrorKind::MissingDescription
);

#[test]
fn duplicate_type() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write("types/timg-copy.toml", TIMG)?;
    fixture.edit(
        "root.toml",
        "\"types/ble.toml\"",
        "\"types/ble.toml\", \"types/timg-copy.toml\"",
    )?;
    let error = fixture.rejection()?;
    assert!(matches!(error.kind, ErrorKind::Duplicate(_)), "{error}");
    Ok(())
}

// Names.
rejects!(
    invalid_name,
    "types/timg.toml",
    "name = \"INT_RAW\"",
    "name = \"int_raw\"",
    ErrorKind::InvalidName(_)
);
rejects!(
    dim_without_index,
    "types/timg.toml",
    "name = \"REGCLK_%s\"",
    "name = \"REGCLK\"",
    ErrorKind::InvalidName(_)
);
rejects!(
    group_without_index,
    "types/timg.toml",
    "name = \"T%s\"",
    "name = \"TIMER\"",
    ErrorKind::InvalidName(_)
);
rejects!(
    duplicate_field,
    "types/timg.toml",
    "name = \"T1\"\nbits = \"1\"",
    "name = \"T0\"\nbits = \"1\"",
    ErrorKind::Duplicate(_)
);
rejects!(
    duplicate_instance,
    "root.toml",
    "name = \"TIMG1\"",
    "name = \"TIMG0\"",
    ErrorKind::Duplicate(_)
);
rejects!(
    group_element_shadows_register,
    "types/timg.toml",
    "name = \"FIFO\"",
    "name = \"T1\"",
    ErrorKind::Duplicate(_)
);

// Register and field geometry.
rejects!(
    field_outside_register,
    "types/timg.toml",
    "bits = \"7:4\"",
    "bits = \"35:4\"",
    ErrorKind::FieldBits { .. }
);
rejects!(
    field_bits_reversed,
    "types/timg.toml",
    "bits = \"7:4\"",
    "bits = \"4:7\"",
    ErrorKind::FieldBits { .. }
);
rejects!(
    field_overlap,
    "types/timg.toml",
    "bits = \"7:4\"",
    "bits = \"7:3\"",
    ErrorKind::FieldOverlap(_)
);
rejects!(
    reserved_overlaps_field,
    "types/timg.toml",
    "mask = 0xfffffffc",
    "mask = 0xfffffffe",
    ErrorKind::Reserved(_)
);
rejects!(
    reserved_wider_than_register,
    "types/bbpll.toml",
    "[[registers.fields]]",
    "reserved = { mask = 0x1c0, sources = [\"SRC_HEADER\"] }\n\n[[registers.fields]]",
    ErrorKind::Reserved(_)
);
rejects!(
    reset_outside_mask,
    "types/timg.toml",
    "reset = { value = 0x80000000 }",
    "reset = { value = 0x80000000, mask = 0x1 }",
    ErrorKind::Reset(_)
);
rejects!(
    value_wider_than_field,
    "types/timg.toml",
    "value = 1 }",
    "value = 256 }",
    ErrorKind::Values(_)
);
rejects!(
    write_range_on_read_only,
    "types/timg.toml",
    "class = \"read-only\"",
    "class = \"read-only\"\nwrite-range = { min = 0, max = 1 }",
    ErrorKind::Values(_)
);
rejects!(
    misaligned_register,
    "types/timg.toml",
    "offset = 0x70",
    "offset = 0x72",
    ErrorKind::RegisterGeometry(_)
);
rejects!(
    analog_register_not_a_byte,
    "types/bbpll.toml",
    "offset = 0x02",
    "offset = 0x02\nwidth = 32",
    ErrorKind::RegisterGeometry(_)
);
rejects!(
    analog_index_above_byte,
    "types/bbpll.toml",
    "offset = 0x02",
    "offset = 0x100",
    ErrorKind::RegisterGeometry(_)
);
rejects!(
    dim_stride_below_width,
    "types/timg.toml",
    "stride = 4 }",
    "stride = 2 }",
    ErrorKind::Dim(_)
);
rejects!(
    group_stride_below_span,
    "types/timg.toml",
    "stride = 0x24",
    "stride = 0x20",
    ErrorKind::Group(_)
);
rejects!(
    group_overlaps_register,
    "types/timg.toml",
    "offset = 0x70",
    "offset = 0x40",
    ErrorKind::WordOverlap { .. }
);
rejects!(
    registers_overlap,
    "types/timg.toml",
    "offset = 0xb0",
    "offset = 0xa0",
    ErrorKind::WordOverlap { .. }
);

// Register-level constructs.
rejects!(
    command_names_a_plain_field,
    "types/timg.toml",
    "commands = [\"READ\", \"ERASE\"]",
    "commands = [\"READ\", \"STATE\"]",
    ErrorKind::Command(_)
);
rejects!(
    command_leaves_a_self_clearing_bit,
    "types/timg.toml",
    "held = [\"BUSY\"]",
    "held = []",
    ErrorKind::Command(_)
);
rejects!(
    command_register_with_trigger,
    "types/timg.toml",
    "class = \"read-only\"",
    "class = \"write-trigger\"",
    ErrorKind::Command(_)
);
rejects!(
    command_unknown_register,
    "types/timg.toml",
    "register = \"CMD\"",
    "register = \"NOPE\"",
    ErrorKind::Command(_)
);
rejects!(
    fifo_write_one_to_clear,
    "types/timg.toml",
    "bits = \"7:0\"\nclass = \"plain\"\n\n[[registers]]\nname = \"DATA_PORT\"",
    "bits = \"7:0\"\nclass = \"write-one-to-clear\"\n\n[[registers]]\nname = \"DATA_PORT\"",
    ErrorKind::Port(_)
);
rejects!(
    data_port_read_only,
    "types/timg.toml",
    "class = \"hardware-written\"",
    "class = \"read-only\"",
    ErrorKind::Port(_)
);
rejects!(
    two_ports,
    "types/timg.toml",
    "fifo = { sources = [\"SRC_HEADER\"] }",
    "fifo = { sources = [\"SRC_HEADER\"] }\nread-side-effect = { sources = [\"SRC_HEADER\"] }",
    ErrorKind::Port(_)
);
rejects!(
    read_safe_outside_analog,
    "types/timg.toml",
    "offset = 0x70",
    "offset = 0x70\nread-safe = { epoch = \"boot\", sources = [\"SRC_USE\"] }",
    ErrorKind::AnalogOnly(_)
);
rejects!(
    read_safe_of_read_clears,
    "types/bbpll.toml",
    "class = \"plain\"",
    "class = \"read-clears\"",
    ErrorKind::ReadSafe(_)
);
rejects!(
    view_not_a_strict_subrange,
    "types/bbpll.toml",
    "bits = \"2:0\"",
    "bits = \"5:0\"",
    ErrorKind::View(_)
);

// Review.
rejects!(
    review_unknown_entity,
    "types/timg.toml",
    "[review.entity.\"INT_RAW.T0\"]",
    "[review.entity.\"INT_RAW.T9\"]",
    ErrorKind::ReviewUnknownEntity
);
rejects!(
    review_repeats_default,
    "types/timg.toml",
    "accuracy = \"bounded\"",
    "accuracy = \"exact\"",
    ErrorKind::ReviewRepeatsDefault("accuracy")
);
rejects!(
    review_incomplete,
    "types/ble.toml",
    "naming = \"descriptive\"\n",
    "",
    ErrorKind::ReviewIncomplete("naming")
);
rejects!(
    opaque_name_not_positional,
    "types/timg.toml",
    "name = \"WORD_0080_OPAQUE\"",
    "name = \"WORD_0084_OPAQUE\"",
    ErrorKind::Naming(_)
);
rejects!(
    descriptive_positional_name,
    "types/ble.toml",
    "name = \"PENDING\"",
    "name = \"BIT_0\"",
    ErrorKind::Naming(_)
);

// Sources.
rejects!(
    unknown_source,
    "root.toml",
    "sources = [\"SRC_BASE\"]",
    "sources = [\"SRC_ABSENT\"]",
    ErrorKind::UnknownSource(_)
);
rejects!(
    empty_sources,
    "types/timg.toml",
    "fifo = { sources = [\"SRC_HEADER\"] }",
    "fifo = { sources = [] }",
    ErrorKind::InvalidSources
);
rejects!(
    repeated_sources,
    "root.toml",
    "sources = [\"SRC_BASE\"]",
    "sources = [\"SRC_BASE\", \"SRC_BASE\"]",
    ErrorKind::InvalidSources
);
rejects!(
    partial_extent_without_sources,
    "root.toml",
    "partial = true, sources = [\"SRC_BLOB\"]",
    "partial = true",
    ErrorKind::InvalidSources
);

// Instances.
rejects!(
    unknown_type,
    "root.toml",
    "type = \"BLE\"",
    "type = \"BTDM\"",
    ErrorKind::UnknownType(_)
);
rejects!(
    analog_type_at_cpu_address,
    "root.toml",
    "type = \"BLE\"",
    "type = \"BBPLL\"",
    ErrorKind::WrongSpace { .. }
);
rejects!(
    misaligned_extent,
    "root.toml",
    "size = 0x20,",
    "size = 0x22,",
    ErrorKind::Extent(_)
);
rejects!(
    register_outside_extent,
    "root.toml",
    "size = 0x20,",
    "size = 0x10,",
    ErrorKind::OutsideExtent(_)
);
rejects!(
    instances_overlap,
    "root.toml",
    "base = 0x60009000",
    "base = 0x60008800",
    ErrorKind::AddressOverlap(_)
);
rejects!(
    ram_region_overlaps_instance,
    "root.toml",
    "base = 0x40800000",
    "base = 0x6000a000",
    ErrorKind::AddressOverlap(_)
);

// Interrupts.
rejects!(
    duplicate_source_number,
    "root.toml",
    "source = 29",
    "source = 10",
    ErrorKind::DuplicateSourceNumber(10)
);
rejects!(
    interrupt_unknown_instance,
    "root.toml",
    "instance = \"TIMG1\"",
    "instance = \"TIMG9\"",
    ErrorKind::UnknownInstance(_)
);
rejects!(
    interrupt_unknown_output,
    "root.toml",
    "output = \"WDT\"",
    "output = \"PWM\"",
    ErrorKind::Output(_)
);
rejects!(
    interrupt_without_output,
    "root.toml",
    "output = \"WDT\"\n",
    "",
    ErrorKind::Output(_)
);

// Analog-I2C space.
rejects!(
    duplicate_analog_code,
    "root.toml",
    "[[ram-regions]]",
    "[[analog-i2c.instances]]\nname = \"BBPLL2\"\ntype = \"BBPLL\"\ncode = 0x66\nsources = [\"SRC_HEADER\"]\n\n[[ram-regions]]",
    ErrorKind::DuplicateCode(0x66)
);
rejects!(
    invalid_route,
    "root.toml",
    "read-mask-bits = 32",
    "read-mask-bits = 16",
    ErrorKind::Route(_)
);

// Memory layouts.
rejects!(
    placement_unknown_region,
    "root.toml",
    "regions = [\"dma-window\"]",
    "regions = [\"sram\"]",
    ErrorKind::Placement(_)
);
rejects!(
    placement_unknown_layout,
    "root.toml",
    "layout = \"RX_DESCRIPTOR\"",
    "layout = \"TX_DESCRIPTOR\"",
    ErrorKind::Placement(_)
);
rejects!(
    layout_places_itself,
    "layouts/rx-descriptor.toml",
    "align = 4",
    "align = 4\nplacement = [\"dma-window\"]",
    ErrorKind::Placement(_)
);
#[test]
fn address_field_unknown_region() -> TestResult {
    let fixture = Fixture::new()?;
    fixture
        .edit("root.toml", "name = \"dma-window\"", "name = \"sram\"")?
        .edit(
            "root.toml",
            "regions = [\"dma-window\"]",
            "regions = [\"sram\"]",
        )?;
    let error = fixture.rejection()?;
    assert!(matches!(error.kind, ErrorKind::Placement(_)), "{error}");
    assert_eq!(error.entity, "RX_DESCRIPTOR");
    Ok(())
}
rejects!(
    invalid_layout,
    "layouts/rx-descriptor.toml",
    "size = 8",
    "size = 6",
    ErrorKind::Layouts(_)
);
rejects!(
    ram_region_empty,
    "root.toml",
    "size = 0x10000",
    "size = 0",
    ErrorKind::RamRegion(_)
);

// Uniqueness and counts not covered above.
rejects!(
    invalid_chip_name,
    "root.toml",
    "chip = \"testchip\"",
    "chip = \"TestChip\"",
    ErrorKind::InvalidName(_)
);
rejects!(
    duplicate_output,
    "types/timg.toml",
    "name = \"WDT\"",
    "name = \"TIMER\"",
    ErrorKind::Duplicate(_)
);
rejects!(
    duplicate_interrupt_name,
    "root.toml",
    "name = \"BLE_EVENT_ALT\"",
    "name = \"BLE_EVENT\"",
    ErrorKind::Duplicate(_)
);
rejects!(
    duplicate_command_register,
    "types/timg.toml",
    "[[commands]]",
    "[[commands]]\nregister = \"CMD\"\ncommands = [\"READ\"]\nqualifiers = [\"WHOLE_CHIP\"]\nheld = [\"BUSY\", \"ERASE\"]\nsources = [\"SRC_HEADER\"]\n\n[[commands]]",
    ErrorKind::Duplicate(_)
);
rejects!(
    dim_of_one,
    "types/timg.toml",
    "count = 2, stride = 4",
    "count = 1, stride = 4",
    ErrorKind::Dim(_)
);
rejects!(
    group_of_one,
    "types/timg.toml",
    "count = 2\nstride = 0x24",
    "count = 1\nstride = 0x24",
    ErrorKind::Group(_)
);
rejects!(
    write_values_only_without_values,
    "types/timg.toml",
    "class = \"read-only\"",
    "class = \"plain\"\nwrite-values-only = true",
    ErrorKind::Values(_)
);
rejects!(
    read_safe_zero_comparison_mask,
    "types/bbpll.toml",
    "comparison-mask = 0x3f",
    "comparison-mask = 0",
    ErrorKind::ReadSafe(_)
);
rejects!(
    ambiguous_route,
    "root.toml",
    "[[ram-regions]]",
    "[[analog-i2c.instances.routes]]\nprofile = \"phy\"\nrequest-code = 0x66\nhost = 1\nread-select = 0\nread-mask-bits = 32\nsources = [\"SRC_HEADER\"]\n\n[[ram-regions]]",
    ErrorKind::Route(_)
);
rejects!(
    duplicate_placement,
    "root.toml",
    "regions = [\"dma-window\"]",
    "regions = [\"dma-window\"]\n\n[[layout-placements]]\nlayout = \"RX_DESCRIPTOR\"\nregions = [\"dma-window\"]",
    ErrorKind::Duplicate(_)
);
rejects!(
    duplicate_ram_region,
    "root.toml",
    "[[layout-placements]]",
    "[[ram-regions]]\nname = \"dma-window\"\nbase = 0x40900000\nsize = 0x100\nsources = [\"SRC_HEADER\"]\n\n[[layout-placements]]",
    ErrorKind::Duplicate(_)
);

// Cross-review regressions: validate relationships after root placement.
#[test]
fn root_places_pointer_targets() -> TestResult {
    for compressed in [false, true] {
        let fixture = Fixture::new()?;
        fixture.edit(
            "layouts/rx-descriptor.toml",
            "placement = [\"dma-window\"]",
            "points-to = \"RX_DESCRIPTOR\"",
        )?;
        if compressed {
            fixture.edit(
                "layouts/rx-descriptor.toml",
                "bits = \"31:0\"",
                "bits = \"19:0\"\nencoding = \"ble-compressed\"",
            )?;
        }
        let model = fixture.load()?;
        assert!(model.layouts()[0].is_dma_target());
        assert_eq!(
            model.layouts()[0].units[1].fields[0].points_to.as_deref(),
            Some("RX_DESCRIPTOR")
        );
    }
    Ok(())
}

const RING_LAYOUT: &str = r#"
schema = 1
[layout]
name = "RX_RING"
description = "Synthetic descriptor ring."
size = 16
align = 4
[layout.array]
element = "RX_DESCRIPTOR"
count = 2
[layout.review]
sources = ["SRC_HEADER"]
provenance = "reviewed"
accuracy = "exact"
completeness = "complete"
naming = "descriptive"
"#;

#[test]
fn root_places_arrays_and_checks_element_placement() -> TestResult {
    for place_array in [false, true] {
        let fixture = Fixture::new()?;
        fixture.write("layouts/ring.toml", RING_LAYOUT)?;
        fixture.edit(
            "root.toml",
            "layouts = [\"layouts/rx-descriptor.toml\"]",
            "layouts = [\"layouts/rx-descriptor.toml\", \"layouts/ring.toml\"]",
        )?;
        fixture.edit(
            "layouts/rx-descriptor.toml",
            "placement = [\"dma-window\"]",
            "points-to = \"RX_RING\"",
        )?;
        if place_array {
            fixture.edit(
                "root.toml",
                "[[layout-placements]]",
                "[[layout-placements]]\nlayout = \"RX_RING\"\nregions = [\"dma-window\"]\n\n[[layout-placements]]",
            )?;
            let model = fixture.load()?;
            assert!(model.layouts().iter().all(|layout| layout.is_dma_target()));
        } else {
            let error = fixture.rejection()?;
            assert!(matches!(error.kind, ErrorKind::Layouts(_)), "{error}");
        }
    }
    Ok(())
}

#[test]
fn root_checks_same_geometry_after_placement() -> TestResult {
    for region in ["dma-window", "other-ram"] {
        let fixture = Fixture::new()?;
        fixture.write(
            "layouts/peer.toml",
            &LAYOUT.replace(
                "name = \"RX_DESCRIPTOR\"",
                "name = \"TX_DESCRIPTOR\"\nsame-geometry-as = \"RX_DESCRIPTOR\"",
            ),
        )?;
        fixture.edit(
            "root.toml",
            "layouts = [\"layouts/rx-descriptor.toml\"]",
            "layouts = [\"layouts/rx-descriptor.toml\", \"layouts/peer.toml\"]",
        )?;
        fixture.write(
            "root.toml",
            &format!(
                "{}\n[[ram-regions]]\nname = \"other-ram\"\nbase = 0x40900000\nsize = 0x10000\nsources = [\"SRC_HEADER\"]\n\n[[layout-placements]]\nlayout = \"TX_DESCRIPTOR\"\nregions = [\"{region}\"]\n",
                fs::read_to_string(fixture.dir.path().join("root.toml"))?
            ),
        )?;
        if region == "dma-window" {
            fixture.load()?;
        } else {
            let error = fixture.rejection()?;
            assert!(matches!(error.kind, ErrorKind::Layouts(_)), "{error}");
        }
    }
    Ok(())
}

rejects!(
    pointer_requires_included_target,
    "layouts/rx-descriptor.toml",
    "placement = [\"dma-window\"]",
    "points-to = \"MISSING\"",
    ErrorKind::Layouts(_)
);

#[test]
fn pointer_requires_dma_target_placement() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "layouts/rx-descriptor.toml",
        "placement = [\"dma-window\"]",
        "points-to = \"RX_DESCRIPTOR\"",
    )?;
    fixture.edit(
        "root.toml",
        "[[layout-placements]]\nlayout = \"RX_DESCRIPTOR\"\nregions = [\"dma-window\"]",
        "",
    )?;
    let error = fixture.rejection()?;
    assert!(matches!(error.kind, ErrorKind::Layouts(_)), "{error}");
    Ok(())
}

rejects!(
    group_offset_overflow_returns_geometry_error,
    "types/timg.toml",
    "name = \"T%s\"\noffset = 0x00",
    "name = \"T%s\"\noffset = 0xfffffffc",
    ErrorKind::RegisterGeometry(_)
);
rejects!(
    huge_analog_dim_is_rejected_before_expansion,
    "types/bbpll.toml",
    "name = \"OC_DIV\"\noffset = 0x02",
    "name = \"OC_DIV_%s\"\noffset = 0x02\ndim = { count = 4294967295, stride = 1 }",
    ErrorKind::RegisterGeometry(_)
);
rejects!(
    huge_mmio_dim_is_rejected_before_expansion,
    "types/timg.toml",
    "count = 2, stride = 4",
    "count = 4294967295, stride = 4",
    ErrorKind::ExpansionLimit(_)
);
rejects!(
    huge_mmio_group_is_rejected_before_expansion,
    "types/timg.toml",
    "count = 2\nstride = 0x24",
    "count = 4294967295\nstride = 0x24",
    ErrorKind::ExpansionLimit(_)
);

#[test]
fn huge_analog_group_is_rejected_before_expansion() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write(
        "types/bbpll.toml",
        &BBPLL.replace("[[registers", "[[groups.registers").replacen(
            "[[groups.registers]]",
            "[[groups]]\nname = \"BANK_%s\"\noffset = 0\ncount = 4294967295\nstride = 4\n\n[[groups.registers]]",
            1,
        ),
    )?;
    let error = fixture.rejection()?;
    assert!(
        matches!(error.kind, ErrorKind::RegisterGeometry(_)),
        "{error}"
    );
    Ok(())
}

#[test]
fn nested_repetitions_share_the_type_budget() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "types/timg.toml",
        "count = 2\nstride = 0x24",
        "count = 10000\nstride = 0x10000",
    )?;
    fixture.edit(
        "types/timg.toml",
        "name = \"LOAD\"\noffset = 0x20",
        "name = \"LOAD_%s\"\noffset = 0x20\ndim = { count = 10000, stride = 4 }",
    )?;
    let error = fixture.rejection()?;
    assert!(
        matches!(error.kind, ErrorKind::ExpansionLimit(_)),
        "{error}"
    );
    Ok(())
}

#[test]
fn long_names_share_the_expanded_text_budget() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "types/ble.toml",
        "name = \"EVENT_STATUS\"\noffset = 0x10",
        &format!(
            "name = \"EVENT_{}_%s\"\noffset = 0x10\ndim = {{ count = 4096, stride = 4 }}",
            "A".repeat(16384)
        ),
    )?;
    let error = fixture.rejection()?;
    assert!(
        matches!(error.kind, ErrorKind::ExpansionLimit(_)),
        "{error}"
    );
    Ok(())
}

#[test]
fn instances_share_the_chip_expansion_budget() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "types/ble.toml",
        "name = \"EVENT_STATUS\"\noffset = 0x10",
        "name = \"EVENT_STATUS_%s\"\noffset = 0x10\ndim = { count = 20000, stride = 4 }",
    )?;
    fixture.edit("root.toml", "size = 0x20,", "size = 0x20000,")?;
    fixture.edit(
        "root.toml",
        "[[interrupts]]",
        "[[instances]]\nname = \"BLE2\"\ntype = \"BLE\"\nbase = 0x60030000\nextent = { size = 0x20000 }\nsources = [\"SRC_BASE\"]\n\n[[interrupts]]",
    )?;
    let error = fixture.rejection()?;
    assert!(
        matches!(error.kind, ErrorKind::ExpansionLimit(_)),
        "{error}"
    );
    Ok(())
}

rejects!(
    misaligned_group_offset,
    "types/timg.toml",
    "name = \"T%s\"\noffset = 0x00",
    "name = \"T%s\"\noffset = 0x01",
    ErrorKind::Group(_)
);
rejects!(
    misaligned_group_stride,
    "types/timg.toml",
    "stride = 0x24",
    "stride = 0x25",
    ErrorKind::Group(_)
);

#[test]
fn group_alignment_preserves_members_with_dim_and_mixed_widths() -> TestResult {
    for (offset, stride) in [(1, 8), (4, 9), (4, 8)] {
        let fixture = Fixture::new()?;
        let group = format!(
            r#"
[[groups]]
name = "LANE_%s"
offset = {offset}
count = 2
stride = {stride}
[[groups.registers]]
name = "FLAGS"
offset = 0
width = 8
[[groups.registers.fields]]
name = "ACTIVE"
bits = "0"
class = "plain"
[[groups.registers]]
name = "CONTROL_%s"
offset = 4
width = 16
dim = {{ count = 2, stride = 2 }}
[[groups.registers.fields]]
name = "ENABLE"
bits = "0"
class = "plain"
"#
        );
        fixture.write("types/ble.toml", &format!("{BLE}{group}"))?;
        fixture.edit("types/ble.toml", "offset = 0x10", "offset = 0x1c")?;
        if offset == 4 && stride == 8 {
            fixture.load()?;
        } else {
            let error = fixture.rejection()?;
            assert!(matches!(error.kind, ErrorKind::Group(_)), "{error}");
        }
    }
    Ok(())
}

#[test]
fn read_safe_requires_readable_comparison_bits() -> TestResult {
    for class in ["write-only", "write-trigger", "unknown"] {
        for explicit_mask in [false, true] {
            let fixture = Fixture::new()?;
            fixture.edit(
                "types/bbpll.toml",
                "class = \"plain\"",
                &format!("class = \"{class}\""),
            )?;
            if !explicit_mask {
                fixture.edit("types/bbpll.toml", ", comparison-mask = 0x3f", "")?;
            }
            let error = fixture.rejection()?;
            assert!(matches!(error.kind, ErrorKind::ReadSafe(_)), "{error}");
        }
    }
    Ok(())
}

#[test]
fn read_safe_comparison_can_exclude_unreadable_lanes() -> TestResult {
    for comparison in ["0x3f", "0x7f"] {
        let fixture = Fixture::new()?;
        fixture.write(
            "types/bbpll.toml",
            &format!(
                r#"{BBPLL}
[[registers.fields]]
name = "WRITE_CONTROL"
bits = "6"
class = "write-only"
"#
            ),
        )?;
        fixture.edit(
            "types/bbpll.toml",
            "comparison-mask = 0x3f",
            &format!("comparison-mask = {comparison}"),
        )?;
        if comparison == "0x3f" {
            fixture.load()?;
        } else {
            let error = fixture.rejection()?;
            assert!(matches!(error.kind, ErrorKind::ReadSafe(_)), "{error}");
        }
    }
    Ok(())
}

rejects!(
    review_hint_is_not_an_assertion,
    "types/ble.toml",
    "provenance = \"observed\"",
    "provenance = \"hint\"",
    ErrorKind::ReviewHint
);
rejects!(
    opaque_review_hint_is_not_an_assertion,
    "types/timg.toml",
    "provenance = \"observed\"",
    "provenance = \"hint\"",
    ErrorKind::ReviewHint
);
rejects!(
    review_override_hint_is_not_an_assertion,
    "types/timg.toml",
    "accuracy = \"bounded\"",
    "accuracy = \"bounded\"\nprovenance = \"hint\"",
    ErrorKind::ReviewHint
);

#[test]
fn unknown_class_is_neither_read_nor_written() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "types/ble.toml",
        "class = \"read-only\"",
        "class = \"unknown\"",
    )?;
    let model = fixture.load()?;
    let pending = model
        .field("BLE.EVENT_STATUS.PENDING")
        .ok_or("no PENDING")?;
    assert_eq!(pending.class, FieldClass::Unknown);
    assert!(!pending.class.readable() && !pending.class.written());
    assert_eq!(pending.class.label(), "unknown");
    Ok(())
}

// A class is required wherever a construct reads or writes the field.
rejects!(
    unknown_class_in_a_command_register,
    "types/timg.toml",
    "name = \"STATE\"\nbits = \"7:4\"\nclass = \"read-only\"",
    "name = \"STATE\"\nbits = \"7:4\"\nclass = \"unknown\"",
    ErrorKind::Command(_)
);
rejects!(
    unknown_class_as_a_fifo,
    "types/timg.toml",
    "fifo = { sources = [\"SRC_HEADER\"] }\n\n[[registers.fields]]\nname = \"DATA\"\nbits = \"7:0\"\nclass = \"plain\"",
    "fifo = { sources = [\"SRC_HEADER\"] }\n\n[[registers.fields]]\nname = \"DATA\"\nbits = \"7:0\"\nclass = \"unknown\"",
    ErrorKind::Port(_)
);
rejects!(
    unknown_class_as_a_data_port,
    "types/timg.toml",
    "class = \"hardware-written\"",
    "class = \"unknown\"",
    ErrorKind::Port(_)
);
rejects!(
    unknown_class_with_a_write_constraint,
    "types/timg.toml",
    "class = \"plain\"\nwrite-range",
    "class = \"unknown\"\nwrite-range",
    ErrorKind::Values(_)
);

#[test]
fn overridden_review_default_sources_are_validated() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write(
        "types/ble.toml",
        &format!(
            r#"{}
[review.entity."EVENT_STATUS"]
sources = ["SRC_BLOB"]
[review.entity."EVENT_STATUS.PENDING"]
sources = ["SRC_BLOB"]
"#,
            BLE.replace(
                "[review]\nsources = [\"SRC_BLOB\"]",
                "[review]\nsources = [\"NOT_IN_CATALOG\"]"
            )
        ),
    )?;
    let error = fixture.rejection()?;
    assert!(matches!(error.kind, ErrorKind::UnknownSource(_)), "{error}");
    Ok(())
}

#[test]
fn unused_opaque_review_sources_are_validated() -> TestResult {
    for sources in ["[\"NOT_IN_CATALOG\"]", "[]", "[\"SRC_BLOB\", \"SRC_BLOB\"]"] {
        let fixture = Fixture::new()?;
        fixture.write(
            "types/ble.toml",
            &format!("{BLE}\n[review.opaque]\nsources = {sources}\n"),
        )?;
        let error = fixture.rejection()?;
        assert!(
            matches!(
                error.kind,
                ErrorKind::UnknownSource(_) | ErrorKind::InvalidSources
            ),
            "{error}"
        );
    }
    Ok(())
}

#[test]
fn included_layout_may_be_unplaced_without_being_a_dma_target() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "root.toml",
        "[[layout-placements]]\nlayout = \"RX_DESCRIPTOR\"\nregions = [\"dma-window\"]",
        "",
    )?;
    let model = fixture.load()?;
    assert!(model.layouts()[0].placement.is_empty());
    assert!(!model.layouts()[0].is_dma_target());
    Ok(())
}

#[test]
fn byte_decoder_may_be_unplaced() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.write(
        "layouts/rx-descriptor.toml",
        &LAYOUT
            .replace("[[layout.words]]", "[[layout.bytes]]")
            .replace("[[layout.words.fields]]", "[[layout.bytes.fields]]")
            .replace("access = \"shared\"", "size = 4\naccess = \"owned\"")
            .replace("placement = [\"dma-window\"]", ""),
    )?;
    fixture.edit(
        "root.toml",
        "[[layout-placements]]\nlayout = \"RX_DESCRIPTOR\"\nregions = [\"dma-window\"]",
        "",
    )?;
    let model = fixture.load()?;
    assert!(model.layouts()[0].placement.is_empty());
    assert!(!model.layouts()[0].is_dma_target());
    Ok(())
}

#[test]
fn expanded_field_names_share_the_chip_text_budget() -> TestResult {
    let fixture = Fixture::new()?;
    fixture.edit(
        "types/ble.toml",
        "name = \"EVENT_STATUS\"\noffset = 0x10",
        "name = \"EVENT_STATUS_%s\"\noffset = 0x10\ndim = { count = 4096, stride = 4 }",
    )?;
    fixture.edit(
        "types/ble.toml",
        "name = \"PENDING\"",
        &format!("name = \"PENDING_{}\"", "A".repeat(8192)),
    )?;
    fixture.edit("root.toml", "size = 0x20,", "size = 0x5000,")?;
    let error = fixture.rejection()?;
    assert!(
        matches!(error.kind, ErrorKind::ExpansionLimit(_)),
        "{error}"
    );
    assert_eq!(error.entity, "BLE");
    Ok(())
}

#[test]
fn included_types_share_the_materialization_budget() -> TestResult {
    let fixture = Fixture::new()?;
    let repeated = BLE.replace(
        "name = \"EVENT_STATUS\"\noffset = 0x10",
        "name = \"EVENT_STATUS_%s\"\noffset = 0x10\ndim = { count = 20000, stride = 4 }",
    );
    fixture.write("types/ble.toml", &repeated)?;
    fixture.write(
        "types/copy.toml",
        &repeated.replace("name = \"BLE\"", "name = \"BLE_COPY\""),
    )?;
    fixture.edit(
        "root.toml",
        "\"types/ble.toml\"",
        "\"types/ble.toml\", \"types/copy.toml\"",
    )?;
    let error = fixture.rejection()?;
    assert!(
        matches!(error.kind, ErrorKind::ExpansionLimit(_)),
        "{error}"
    );
    assert!(error.entity.starts_with("BLE_COPY."));
    Ok(())
}
#[path = "chip_model/controls.rs"]
mod controls;
#[path = "chip_model/facts.rs"]
mod facts;
