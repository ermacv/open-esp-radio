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
fields = [{ name = "ADDRESS", bits = "31:0", writer = "cpu", layout = "NODE" }]

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
    assert_eq!(node.units[1].fields[0].layout.as_deref(), Some("NODE"));
    assert_eq!(prefix.kind, LayoutKind::Bytes);
    assert_eq!(prefix.units[1].size, 2);
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
            node_with("layout = \"NODE\"", "layout = \"OTHER\""),
            "not a word layout",
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
                "layout = \"NODE\"",
                "layout = \"NODE\", placement = [\"dma-window\"]",
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
    assert!(error(&[("node.toml", &layout_named)]).contains("is a layout"));
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
        "#[repr(C, align(4))]\npub struct Node {",
        "control: NodeControlCell,",
        "unsafe impl bytemuck::Zeroable for Node {}",
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
        "pub const fn with_address(self, value: u32) -> Self { Self(value) }",
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
    let rust = super::render(model.layouts(), &regions()).unwrap();
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
        "only named regions are rendered"
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
fn a_model_without_layouts_renders_nothing() {
    assert_eq!(super::render(&[], &regions()).unwrap(), "");
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
