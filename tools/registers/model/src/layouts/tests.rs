use std::{collections::BTreeMap, fs};

use crate::{LayoutKind, RegisterModel};

const DEVICE: &str = r#"schema = 3
chip = "fixture-chip"
address-space = "cpu"
fragments = ["radio.toml"]
layouts = ["layouts/node.toml"]

[device]
name = "device"
version = "1"
description = "device"
address-unit-bits = 8
width = 32
"#;

const FRAGMENT: &str = r#"schema = 2

[[peripherals]]
name = "RADIO"
baseAddress = 0x1000

[[peripherals.registers]]
[peripherals.registers.register]
name = "STATUS"
addressOffset = 0
size = 32
access = "read-write"

[[peripherals.registers.register.fields]]
name = "FLAG"
bitOffset = 3
bitWidth = 1

[[review]]
entity = "RADIO.STATUS"
sources = ["fixture"]
provenance = "reviewed"
accuracy = "exact"
completeness = "complete"
naming = "descriptive"

[[review]]
entity = "RADIO.STATUS.FLAG"
sources = ["fixture"]
provenance = "reviewed"
accuracy = "exact"
completeness = "complete"
naming = "descriptive"
"#;

/// A reviewed two-word node: a control word with a flag, an opaque bit and
/// a length, and a link to the next node.
const NODE: &str = r#"schema = 1

[layout]
name = "NODE"
description = "A node the fixture walker follows."
size = 8
align = 4
placement = ["dma-window"]

[[layout.words]]
name = "CONTROL"
offset = 0
access = "shared"

[[layout.words.fields]]
name = "OWNER"
bits = "31"
writer = "cpu"
description = "1: the walker may fill this node."

[[layout.words.fields]]
name = "BIT_29_OPAQUE"
bits = "29"
writer = "cpu"

[[layout.words.fields]]
name = "LENGTH"
bits = "27:14"
writer = "hardware"

[[layout.words]]
name = "NEXT"
offset = 4
access = "shared"
fields = [{ name = "ADDRESS", bits = "31:0", writer = "cpu", points-to = "NODE" }]

[[review]]
entity = "NODE"
sources = ["fixture"]
provenance = "reviewed"
accuracy = "exact"
completeness = "complete"

[[review]]
entity = "NODE.CONTROL"
sources = ["fixture"]
provenance = "reviewed"
accuracy = "exact"
completeness = "complete"
naming = "descriptive"

[[review]]
entity = "NODE.CONTROL.OWNER"
sources = ["fixture", "stand"]
provenance = "observed"
accuracy = "exact"
completeness = "complete"
naming = "descriptive"

[[review]]
entity = "NODE.CONTROL.BIT_29_OPAQUE"
sources = ["fixture"]
provenance = "observed"
accuracy = "exact"
completeness = "unknown"
naming = "opaque"

[[review]]
entity = "NODE.CONTROL.LENGTH"
sources = ["fixture"]
provenance = "reviewed"
accuracy = "exact"
completeness = "complete"
naming = "descriptive"

[[review]]
entity = "NODE.NEXT"
sources = ["fixture"]
provenance = "reviewed"
accuracy = "exact"
completeness = "complete"
naming = "descriptive"

[[review]]
entity = "NODE.NEXT.ADDRESS"
sources = ["fixture"]
provenance = "reviewed"
accuracy = "exact"
completeness = "complete"
naming = "descriptive"
"#;

/// A reviewed owned byte layout: a signed byte, a two-byte range with two
/// fields and a four-byte word.
const CPU_HEAD: &str = r#"schema = 1

[layout]
name = "HEAD"
description = "The word the fixture CPU fills in before a frame."
size = 8
align = 1

[[layout.bytes]]
name = "FLAGS"
offset = 4
size = 4
access = "owned"
fields = [
    { name = "READY", bits = "24", writer = "cpu" },
    { name = "STATE", bits = "7:4", writer = "cpu" },
    { name = "DONE", bits = "25", writer = "hardware" },
]
"#;

const PREFIX: &str = r#"schema = 1

[layout]
name = "PREFIX"
description = "Metadata the fixture hardware writes before a frame."
size = 8
align = 1

[[layout.bytes]]
name = "RSSI"
offset = 0
size = 1
access = "owned"
fields = [{ name = "VALUE", bits = "7:0", writer = "hardware" }]

[[layout.bytes]]
name = "RATE"
offset = 1
size = 2
access = "owned"
fields = [
    { name = "INDEX", bits = "4:0", writer = "hardware" },
    { name = "FORMAT", bits = "15:12", writer = "hardware" },
]

[[layout.bytes]]
name = "STAMP"
offset = 4
size = 4
access = "owned"
fields = [{ name = "VALUE", bits = "31:0", writer = "hardware" }]
"#;

fn complete_review(entity: &str) -> String {
    format!(
        "\n[[review]]\nentity = \"{entity}\"\nsources = [\"fixture\"]\nprovenance = \"reviewed\"\naccuracy = \"exact\"\ncompleteness = \"complete\"\n"
    )
}

fn named_review(entity: &str) -> String {
    complete_review(entity) + "naming = \"descriptive\"\n"
}

fn prefix_reviewed() -> String {
    let mut text = PREFIX.to_owned() + &complete_review("PREFIX");
    for entity in [
        "PREFIX.RSSI",
        "PREFIX.RSSI.VALUE",
        "PREFIX.RATE",
        "PREFIX.RATE.INDEX",
        "PREFIX.RATE.FORMAT",
        "PREFIX.STAMP",
        "PREFIX.STAMP.VALUE",
    ] {
        text += &named_review(entity);
    }
    text
}

/// Load a fixture model whose layouts are `files` (name, text).
fn load(files: &[(&str, &str)]) -> crate::Result<RegisterModel> {
    let directory = tempfile::tempdir().unwrap();
    let names = files
        .iter()
        .map(|(name, _)| format!("\"layouts/{name}\""))
        .collect::<Vec<_>>()
        .join(", ");
    fs::write(
        directory.path().join("device.toml"),
        DEVICE.replace("[\"layouts/node.toml\"]", &format!("[{names}]")),
    )
    .unwrap();
    fs::write(directory.path().join("radio.toml"), FRAGMENT).unwrap();
    fs::create_dir(directory.path().join("layouts")).unwrap();
    for (name, text) in files {
        fs::write(directory.path().join("layouts").join(name), text).unwrap();
    }
    RegisterModel::load_for_publication(&directory.path().join("device.toml"))
}

fn error(files: &[(&str, &str)]) -> String {
    match load(files) {
        Ok(_) => panic!("an invalid layout was accepted"),
        Err(error) => error.to_string(),
    }
}

fn node_with(from: &str, to: &str) -> String {
    assert!(NODE.contains(from), "{from}");
    NODE.replacen(from, to, 1)
}

#[test]
fn a_reviewed_word_layout_and_byte_layout_load() {
    let prefix = prefix_reviewed();
    let model = load(&[("node.toml", NODE), ("prefix.toml", &prefix)]).unwrap();
    let [node, prefix] = model.layouts() else {
        panic!("two layouts")
    };
    assert_eq!(node.kind, LayoutKind::Words);
    assert_eq!(
        node.units[0]
            .fields
            .iter()
            .map(|field| (field.name.as_str(), field.low, field.width))
            .collect::<Vec<_>>(),
        [
            ("LENGTH", 14, 14),
            ("BIT_29_OPAQUE", 29, 1),
            ("OWNER", 31, 1)
        ]
    );
    assert_eq!(node.units[1].fields[0].points_to.as_deref(), Some("NODE"));
    assert_eq!(prefix.kind, LayoutKind::Bytes);
    assert_eq!(prefix.units[1].size, 2);
}

#[test]
fn a_positional_review_keeps_a_field_opaque_without_claiming_its_meaning() {
    let node = NODE.replace(
        "accuracy = \"exact\"\ncompleteness = \"unknown\"",
        "accuracy = \"positional\"\ncompleteness = \"unknown\"",
    );
    let model = load(&[("node.toml", &node)]).expect("a positional opaque-field review is valid");
    let review = model.layouts()[0]
        .review
        .iter()
        .find(|review| review.entity == "NODE.CONTROL.BIT_29_OPAQUE")
        .expect("opaque field review retained");
    assert_eq!(
        review.accuracy,
        Some(oer_registers_contracts::FactAccuracy::Positional)
    );
    assert_eq!(
        review.completeness,
        Some(oer_registers_contracts::FactCompleteness::Unknown)
    );
    let rust = super::render(model.layouts(), &regions()).expect("render positional review");
    assert!(rust.contains("Opaque: meaning not established."));
}

#[test]
fn the_model_lists_its_layout_files_as_inputs() {
    let directory = tempfile::tempdir().unwrap();
    fs::write(directory.path().join("device.toml"), DEVICE).unwrap();
    let paths = RegisterModel::input_paths(&directory.path().join("device.toml")).unwrap();
    assert!(paths.ends_with(&[directory.path().join("layouts/node.toml")]));
}

#[test]
fn malformed_layouts_are_rejected_with_their_reason() {
    let cases = [
        (node_with("schema = 1", "schema = 2"), "requires schema = 1"),
        (node_with("size = 8", "size = 6"), "multiple of align"),
        (node_with("align = 4", "align = 3"), "power of two"),
        (
            node_with(
                "offset = 4\naccess = \"shared\"",
                "offset = 8\naccess = \"shared\"",
            )
            .replacen("size = 8", "size = 12", 1),
            "not declared",
        ),
        (
            node_with(
                "offset = 4\naccess = \"shared\"",
                "offset = 2\naccess = \"shared\"",
            ),
            "4-byte offset",
        ),
        (
            node_with("bits = \"27:14\"", "bits = \"30:14\""),
            "overlaps",
        ),
        (
            node_with("bits = \"27:14\"", "bits = \"32:14\""),
            "do not fit",
        ),
        (
            node_with("bits = \"27:14\"", "bits = \"14:27\""),
            "do not fit",
        ),
        (
            node_with("bits = \"27:14\"", "bits = \"27-14\""),
            "HIGH:LOW",
        ),
        (
            node_with("access = \"shared\"", "access = \"owned\""),
            "shared",
        ),
        (
            node_with("points-to = \"NODE\"", "points-to = \"OTHER\""),
            "not a word layout",
        ),
        (
            node_with(
                "placement = [\"dma-window\"]\n",
                "placement = [\"dma-window\", \"sram\"]\n",
            ),
            "dma-window region only",
        ),
        (
            node_with(
                "bits = \"27:14\"\nwriter = \"hardware\"",
                "bits = \"27:14\"\nwriter = \"hardware\"\nplacement = [\"dma-window\"]",
            ),
            "whole-word address",
        ),
        (
            node_with(
                "points-to = \"NODE\"",
                "points-to = \"NODE\", placement = [\"dma-window\"]",
            ),
            "takes that layout's placement",
        ),
        (
            node_with("name = \"OWNER\"", "name = \"Owner\""),
            "UPPER_SNAKE",
        ),
        (node_with("name = \"OWNER\"", "name = \"TYPE\""), "keyword"),
        (
            node_with("name = \"OWNER\"", "name = \"BITS\""),
            "generated method",
        ),
        (NODE.replace("NODE", "RADIO"), "name of a peripheral"),
        (
            node_with(
                "[[layout.words]]\nname = \"NEXT\"",
                "[[layout.bytes]]\nname = \"NEXT\"",
            ),
            "either",
        ),
    ];
    for (text, reason) in cases {
        let message = error(&[("node.toml", &text)]);
        assert!(message.contains(reason), "{reason}: {message}");
    }
}

#[test]
fn byte_ranges_are_one_two_or_four_owned_bytes_inside_the_layout() {
    let prefix = prefix_reviewed();
    let cases = [
        (prefix.replacen("size = 2", "size = 3", 1), "size 1, 2 or 4"),
        (
            prefix.replacen("offset = 4\nsize = 4", "offset = 6\nsize = 4", 1),
            "outside the layout",
        ),
        (
            prefix.replacen("offset = 4\nsize = 4", "offset = 2\nsize = 4", 1),
            "overlaps",
        ),
        (
            prefix.replacen("access = \"owned\"", "access = \"shared\"", 1),
            "owned",
        ),
        (
            prefix.replacen("bits = \"15:12\"", "bits = \"16:12\"", 1),
            "do not fit",
        ),
    ];
    for (text, reason) in cases {
        let message = error(&[("prefix.toml", &text)]);
        assert!(message.contains(reason), "{reason}: {message}");
    }
}

#[test]
fn publication_requires_a_complete_review_and_naming_for_every_entity() {
    let missing = NODE.replacen(
        "entity = \"NODE.CONTROL.LENGTH\"",
        "entity = \"NODE.NEXT.LENGTH\"",
        1,
    );
    assert!(error(&[("node.toml", &missing)]).contains("does not exist"));
    let hint = node_with(
        "sources = [\"fixture\", \"stand\"]\nprovenance = \"observed\"",
        "sources = [\"fixture\", \"stand\"]\nprovenance = \"hint\"",
    );
    assert!(error(&[("node.toml", &hint)]).contains("NODE.CONTROL.OWNER has no complete review"));
    let unnamed = node_with(
        "completeness = \"complete\"\nnaming = \"descriptive\"\n\n[[review]]\nentity = \"NODE.CONTROL.OWNER\"",
        "completeness = \"complete\"\n\n[[review]]\nentity = \"NODE.CONTROL.OWNER\"",
    );
    assert!(error(&[("node.toml", &unnamed)]).contains("no reviewed naming"));
    let layout_named = node_with(
        "completeness = \"complete\"\n\n[[review]]\nentity = \"NODE.CONTROL\"",
        "completeness = \"complete\"\nnaming = \"descriptive\"\n\n[[review]]\nentity = \"NODE.CONTROL\"",
    );
    load(&[("node.toml", &layout_named)]).expect("reviewed layout identity naming");
    assert!(error(&[("prefix.toml", PREFIX)]).contains("PREFIX has no complete review"));
}

#[test]
fn opaque_layout_names_are_their_position() {
    let misplaced = node_with("bits = \"29\"", "bits = \"28\"");
    assert!(error(&[("node.toml", &misplaced)]).contains("name it \"BIT_28_OPAQUE\""));
    let unmarked = node_with("naming = \"opaque\"", "naming = \"descriptive\"");
    assert!(error(&[("node.toml", &unmarked)]).contains("not reviewed with naming = \"opaque\""));
    let prefix = prefix_reviewed()
        .replace("RSSI", "BYTE_0000_OPAQUE")
        .replacen(
            "entity = \"PREFIX.BYTE_0000_OPAQUE\"\nsources = [\"fixture\"]\nprovenance = \"reviewed\"\naccuracy = \"exact\"\ncompleteness = \"complete\"\nnaming = \"descriptive\"",
            "entity = \"PREFIX.BYTE_0000_OPAQUE\"\nsources = [\"fixture\"]\nprovenance = \"reviewed\"\naccuracy = \"exact\"\ncompleteness = \"complete\"\nnaming = \"opaque\"",
            1,
        );
    load(&[("prefix.toml", &prefix)]).unwrap();
}

#[test]
fn duplicate_layouts_are_rejected() {
    let message = error(&[("node.toml", NODE), ("again.toml", NODE)]);
    assert!(
        message.contains("duplicate memory layout NODE"),
        "{message}"
    );
}

fn regions() -> BTreeMap<String, (u64, u64)> {
    BTreeMap::from([("dma-window".to_owned(), (0x3000_0000, 0x3008_0000))])
}

#[test]
fn a_word_layout_renders_cells_values_and_its_placement() {
    let model = load(&[("node.toml", NODE)]).unwrap();
    let rust = super::render(model.layouts(), &regions()).unwrap();
    for item in [
        "pub mod layouts {",
        "pub const DMA_WINDOW: core::ops::Range<u32> = 0x30000000..0x30080000;",
        "#[repr(C, align(4))]\n#[derive(bytemuck::Zeroable)]\npub struct Node {",
        "control: NodeControlCell,",
        "pub struct NodeControlCell(oer_mmio::VolatileCell<u32>);",
        "pub const PLACEMENT: &'static [core::ops::Range<u32>] = &[DMA_WINDOW];",
        "pub const fn zeroed() -> Self {",
        "pub const fn control(&self) -> &NodeControlCell {",
        "pub fn read(&self) -> NodeControl {",
        "pub fn write(&self, value: NodeControl) {",
        "pub const fn from_bits(bits: u32) -> Self",
        "pub const fn owner(self) -> bool",
        "pub const fn with_owner(self, value: bool) -> Self",
        "pub const fn length(self) -> u16",
        "pub const fn with_length(self, value: u16) -> Option<Self>",
        "pub const fn address(self) -> u32 { self.0 }",
        "pub const fn with_address(self, value: Option<DmaRef<Node>>) -> Self { Self(match value { Some(reference) => reference.bits(), None => 0 }) }",
        "impl oer_mmio::DmaWindow for DmaWindow {",
        "pub type DmaWindowAddress = oer_mmio::DmaWindowAddress<DmaWindow>;",
        "pub use oer_mmio::DmaRef;",
        "impl oer_mmio::DmaLayout for Node {\ntype Window = DmaWindow;\n}",
        "/// Opaque: meaning not established.",
        "/// Project-assigned name. 1: the walker may fill this node.",
        "/// SOURCE: fixture, stand.",
        "The address of a [`Node`], or zero.",
    ] {
        assert!(rust.contains(item), "{item}\n{rust}");
    }
}

#[test]
fn a_byte_layout_renders_a_fixed_size_decoder() {
    let prefix = prefix_reviewed();
    let model = load(&[("prefix.toml", &prefix)]).unwrap();
    let rust = super::render(model.layouts(), &BTreeMap::new()).unwrap();
    for item in [
        "pub struct Prefix<'a> {\nbytes: &'a [u8; 8],",
        "pub const fn new(bytes: &'a [u8; 8]) -> Self",
        "pub const fn rssi(self) -> PrefixRssi { PrefixRssi(u8::from_le_bytes([self.bytes[0]])) }",
        "pub const fn rate(self) -> PrefixRate { PrefixRate(u16::from_le_bytes([self.bytes[1], self.bytes[2]])) }",
        "pub const fn index(self) -> u8 { (self.0 & 0x1f) as u8 }",
        "pub const fn format(self) -> u8 { (self.0 >> 12) as u8 }",
        "pub const fn value(self) -> u32 { self.0 }",
    ] {
        assert!(rust.contains(item), "{item}\n{rust}");
    }
    assert!(
        !rust.contains("with_"),
        "an owned byte layout is decoded, not built"
    );
    assert!(
        !rust.contains("DMA_WINDOW"),
        "a decoder without a DMA region has no DMA types"
    );
}

#[test]
fn a_cpu_written_byte_layout_renders_a_typed_writer() {
    let mut text = CPU_HEAD.to_owned() + &complete_review("HEAD");
    for entity in [
        "HEAD.FLAGS",
        "HEAD.FLAGS.READY",
        "HEAD.FLAGS.STATE",
        "HEAD.FLAGS.DONE",
    ] {
        text += &named_review(entity);
    }
    let model = load(&[("prefix.toml", &text)]).unwrap();
    let rust = super::render(model.layouts(), &regions()).unwrap();
    for item in [
        "pub struct HeadMut<'a> {\nbytes: &'a mut [u8; 8],",
        "pub fn set_flags(&mut self, value: HeadFlags)",
        "pub const fn from_bits(bits: u32) -> Self { Self(bits) }",
        "pub const fn with_ready(self, value: bool) -> Self",
        "pub const fn with_state(self, value: u8) -> Option<Self>",
    ] {
        assert!(rust.contains(item), "{item}\n{rust}");
    }
    assert!(
        !rust.contains("with_done"),
        "a hardware-written field is read, never built"
    );
}

#[test]
fn rendering_needs_every_named_region() {
    let model = load(&[("node.toml", NODE)]).unwrap();
    let message = super::render(model.layouts(), &BTreeMap::new())
        .unwrap_err()
        .to_string();
    assert!(
        message.contains("dma-window is not a RAM region"),
        "{message}"
    );
}

#[test]
fn a_model_without_layouts_or_a_dma_region_renders_nothing() {
    assert_eq!(super::render(&[], &BTreeMap::new()).unwrap(), "");
}

fn twin() -> String {
    NODE.replace("NODE", "TWIN").replacen(
        "placement = [\"dma-window\"]",
        "placement = [\"dma-window\"]\nsame-geometry-as = \"NODE\"",
        1,
    )
}

#[test]
fn a_layout_repeating_another_s_geometry_is_cross_checked() {
    let renamed = twin().replace("OWNER", "TX_OWNED");
    let model = load(&[("node.toml", NODE), ("twin.toml", &renamed)]).unwrap();
    assert_eq!(model.layouts()[1].same_geometry_as.as_deref(), Some("NODE"));
    let cases = [
        (
            twin().replacen("bits = \"27:14\"", "bits = \"26:14\"", 1),
            "field CONTROL.LENGTH",
        ),
        (
            twin().replacen("align = 4", "align = 8", 1),
            "size, alignment or kind",
        ),
        (
            twin().replacen(
                "same-geometry-as = \"NODE\"",
                "same-geometry-as = \"OTHER\"",
                1,
            ),
            "not another layout",
        ),
        (
            twin().replacen("placement = [\"dma-window\"]", "placement = [\"sram\"]", 1),
            "placement",
        ),
    ];
    for (text, reason) in cases {
        let message = match load(&[("node.toml", NODE), ("twin.toml", &text)]) {
            Ok(_) => panic!("{reason}: accepted"),
            Err(error) => error.to_string(),
        };
        assert!(message.contains(reason), "{reason}: {message}");
    }
}

const DEFAULT_REVIEW: &str = r#"
[layout.review]
sources = ["fixture"]
provenance = "reviewed"
accuracy = "exact"
completeness = "complete"
naming = "descriptive"
"#;

const OPAQUE_REVIEW: &str = r#"
[layout.review.opaque]
sources = ["stand"]
provenance = "observed"
accuracy = "positional"
completeness = "unknown"
naming = "opaque"
"#;

fn default_node() -> String {
    NODE.split("[[review]]")
        .next()
        .expect("a valid layout review fixture")
        .replace(
            "name = \"BIT_29_OPAQUE\"",
            "name = \"BIT_29_OPAQUE\"\nopaque = true",
        )
        + DEFAULT_REVIEW
        + OPAQUE_REVIEW
}

#[test]
fn an_explicitly_unassigned_unit_retains_geometry_without_field_capabilities() {
    let source = format!(
        r#"schema = 1
[layout]
name = "EMPTY"
description = "Cited storage whose bits have no established use."
size = 4
align = 4
{DEFAULT_REVIEW}
{OPAQUE_REVIEW}
[[layout.words]]
name = "WORD_0000_OPAQUE"
opaque = true
offset = 0
access = "shared"
fields = []
"#
    );
    let model = load(&[("empty.toml", &source)]).unwrap();
    let layout = &model.layouts()[0];
    assert_eq!((layout.size, layout.align), (4, 4));
    assert_eq!(layout.units.len(), 1);
    assert!(layout.units[0].fields.is_empty());
    let rust = super::render(model.layouts(), &regions()).unwrap();
    assert!(rust.contains("pub struct EmptyWord0000Opaque(u32)"));
    assert!(!rust.contains("with_bits_0_31_opaque"));
    assert!(
        error(&[("empty.toml", &source.replace("offset = 0", "offset = 4"))])
            .contains("lies outside the layout")
    );
    assert!(
        error(&[("empty.toml", &source.replace("fields = []", ""))])
            .contains("missing field `fields`")
    );
}

#[test]
fn local_defaults_cover_layout_words_and_fields() {
    let model = load(&[("node.toml", &default_node())]).expect("a valid layout review fixture");
    let reviews = &model.layouts()[0].review;
    assert_eq!(reviews.len(), 7);
    for entity in [
        "NODE",
        "NODE.CONTROL",
        "NODE.CONTROL.LENGTH",
        "NODE.NEXT.ADDRESS",
    ] {
        let review = reviews
            .iter()
            .find(|review| review.entity == entity)
            .expect("a valid layout review fixture");
        assert_eq!(review.sources, ["fixture"]);
        assert_eq!(
            review.naming,
            Some(oer_registers_contracts::NameOrigin::Descriptive)
        );
    }
    let opaque = reviews
        .iter()
        .find(|review| review.entity == "NODE.CONTROL.BIT_29_OPAQUE")
        .expect("a valid layout review fixture");
    assert_eq!(opaque.sources, ["stand"]);
    assert_eq!(
        opaque.completeness,
        Some(oer_registers_contracts::FactCompleteness::Unknown)
    );
}

#[test]
fn an_explicit_review_replaces_every_default_key() {
    let text = default_node()
        + &named_review("NODE.CONTROL.OWNER")
            .replace("provenance = \"reviewed\"", "provenance = \"observed\"")
            .replace("sources = [\"fixture\"]", "sources = [\"override\"]");
    let model = load(&[("node.toml", &text)]).expect("a valid layout review fixture");
    let review = model.layouts()[0]
        .review
        .iter()
        .find(|r| r.entity == "NODE.CONTROL.OWNER")
        .expect("a valid layout review fixture");
    assert_eq!(review.sources, ["override"]);
    assert_eq!(
        review.provenance,
        Some(oer_registers_contracts::FactProvenance::Observed)
    );
    let omitted = text.replace("sources = [\"override\"]\n", "");
    assert!(
        error(&[("node.toml", &omitted)]).contains("NODE.CONTROL.OWNER has no complete review")
    );
}

#[test]
fn opaque_selection_is_explicit_and_local_to_one_entity() {
    let unmarked = default_node().replace("opaque = true\n", "");
    assert!(error(&[("node.toml", &unmarked)]).contains("not reviewed with naming = \"opaque\""));
    let misnamed = default_node().replace("BIT_29_OPAQUE", "STATUS");
    assert!(error(&[("node.toml", &misnamed)]).contains("does not end in"));
    let word = default_node().replace(
        "name = \"CONTROL\"",
        "name = \"WORD_0000_OPAQUE\"\nopaque = true",
    );
    let model = load(&[("node.toml", &word)]).expect("a valid layout review fixture");
    let reviews = &model.layouts()[0].review;
    assert_eq!(
        reviews
            .iter()
            .find(|r| r.entity == "NODE.WORD_0000_OPAQUE")
            .expect("a valid layout review fixture")
            .sources,
        ["stand"]
    );
    assert_eq!(
        reviews
            .iter()
            .find(|r| r.entity == "NODE.WORD_0000_OPAQUE.OWNER")
            .expect("a valid layout review fixture")
            .sources,
        ["fixture"]
    );
}

#[test]
fn defaults_apply_to_owned_ranges_and_do_not_leak_to_another_layout() {
    let prefix = PREFIX.to_owned() + DEFAULT_REVIEW;
    load(&[("prefix.toml", &prefix)]).expect("a valid layout review fixture");
    assert!(
        error(&[("node.toml", &default_node()), ("prefix.toml", PREFIX)])
            .contains("PREFIX has no complete review")
    );
}

#[test]
fn default_metadata_keeps_complete_review_and_source_validation() {
    for (from, to, reason) in [
        (
            "provenance = \"reviewed\"",
            "provenance = \"hint\"",
            "no complete review",
        ),
        (
            "sources = [\"fixture\"]",
            "sources = [\"fixture\", \"fixture\"]",
            "duplicate sources",
        ),
        (
            "sources = [\"fixture\"]",
            "sources = [\"\"]",
            "empty or duplicate sources",
        ),
        (
            "accuracy = \"exact\"\n",
            "",
            "must define provenance, accuracy and completeness together",
        ),
    ] {
        let text = default_node().replacen(from, to, 1);
        let message = error(&[("node.toml", &text)]);
        assert!(message.contains(reason), "{message}");
    }
}

#[test]
fn opaque_defaults_do_not_merge_with_ordinary_defaults() {
    let text = default_node().replace("sources = [\"stand\"]\n", "");
    assert!(
        error(&[("node.toml", &text)])
            .contains("NODE.CONTROL.BIT_29_OPAQUE has no complete review")
    );
}

#[test]
fn defaults_reject_unknown_keys_and_unknown_or_redundant_explicit_entities() {
    for text in [
        default_node().replace("[layout.review]\n", "[layout.review]\nsource = []\n"),
        default_node().replace(
            "[layout.review.opaque]\n",
            "[layout.review.opaque]\naccuraccy = \"exact\"\n",
        ),
        default_node().replace("opaque = true", "opaquee = true"),
    ] {
        assert!(error(&[("node.toml", &text)]).contains("unknown field"));
    }
    let unknown = default_node() + &named_review("NODE.MISSING");
    assert!(error(&[("node.toml", &unknown)]).contains("does not exist"));
    let redundant = default_node() + &named_review("NODE.CONTROL");
    assert!(
        error(&[("node.toml", &redundant)])
            .contains("repeats its applicable layout review default")
    );
    let redundant_opaque = default_node()
        + &OPAQUE_REVIEW.replace(
            "[layout.review.opaque]",
            "[[review]]\nentity = \"NODE.CONTROL.BIT_29_OPAQUE\"",
        );
    assert!(
        error(&[("node.toml", &redundant_opaque)])
            .contains("repeats its applicable layout review default")
    );
}

#[test]
fn default_resolution_preserves_rendered_layout_output() {
    let original = load(&[("node.toml", NODE)]).expect("a valid layout review fixture");
    let mut compact = default_node()
        .replace("sources = [\"stand\"]", "sources = [\"fixture\"]")
        .replace("accuracy = \"positional\"", "accuracy = \"exact\"");
    compact += NODE
        .split("[[review]]")
        .find(|block| block.starts_with("\nentity = \"NODE.CONTROL.OWNER\""))
        .map(|block| format!("[[review]]{block}"))
        .expect("a valid layout review fixture")
        .as_str();
    let compact = load(&[("node.toml", &compact)]).expect("a valid layout review fixture");
    let regions = BTreeMap::from([("dma-window".to_owned(), (0x2000, 0x4000))]);
    assert_eq!(
        super::render(original.layouts(), &regions).expect("a valid layout review fixture"),
        super::render(compact.layouts(), &regions).expect("a valid layout review fixture")
    );
}
