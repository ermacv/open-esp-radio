//! Memory layouts render as Rust the PAC can compile.
use std::fs;

const DEVICE: &str = r#"schema = 3
chip = "fixture-chip"
address-space = "cpu"
fragments = ["radio.toml"]
layouts = ["node.toml", "prefix.toml"]

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

[[peripherals.registers]]
[peripherals.registers.register]
name = "NEXT"
addressOffset = 4
size = 32
access = "read-only"

[[peripherals.registers.register.fields]]
name = "ADDRESS_LOW"
bitOffset = 0
bitWidth = 20
"#;

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
fields = [
    { name = "OWNER", bits = "31", writer = "cpu" },
    { name = "LENGTH", bits = "27:14", writer = "hardware" },
    { name = "SIZE", bits = "13:0", writer = "cpu" },
]

[[layout.words]]
name = "BUFFER"
offset = 4
access = "shared"
fields = [{ name = "ADDRESS", bits = "31:0", writer = "cpu", placement = ["dma-window"] }]
"#;

const PREFIX: &str = r#"schema = 1

[layout]
name = "PREFIX"
description = "Metadata the fixture hardware writes before a frame."
size = 8
align = 1

[[layout.bytes]]
name = "RATE"
offset = 1
size = 2
access = "owned"
fields = [
    { name = "INDEX", bits = "4:0", writer = "hardware" },
    { name = "SINGLE", bits = "8", writer = "hardware" },
]

[[layout.bytes]]
name = "STAMP"
offset = 4
size = 4
access = "owned"
fields = [{ name = "VALUE", bits = "31:0", writer = "hardware" }]
"#;

const MEMORY: &str = r#"schema = 1
default-address-space = "cpu"

[[address-spaces]]
id = "cpu"
address-width = 32
endianness = "little"

[[regions]]
name = "radio"
address-space = "cpu"
kind = "mmio"
start = 0x1000
end-exclusive = 0x2000
permissions = "rw"

[[regions]]
name = "dma-window"
address-space = "cpu"
kind = "ram"
start = 0x30000000
end-exclusive = 0x30080000
permissions = "rw"
"#;

fn render_fixture(node: &str, array: &str) -> Result<String, String> {
    render_fixture_with_prefix(node, array, PREFIX)
}

fn render_fixture_with_prefix(node: &str, array: &str, prefix: &str) -> Result<String, String> {
    let (model, memory) = load_fixture(node, array, prefix)?;
    model
        .render_layouts_rust(&memory)
        .map_err(|error| error.to_string())
}

fn load_fixture(
    node: &str,
    array: &str,
    prefix: &str,
) -> Result<
    (
        oer_registers_model::RegisterModel,
        oer_registers_model::memory::Memory,
    ),
    String,
> {
    let directory = tempfile::tempdir().expect("valid generated-layout fixture");
    let path = |name: &str| directory.path().join(name);
    let device = DEVICE.replace(
        "\"node.toml\", \"prefix.toml\"",
        "\"node.toml\", \"prefix.toml\", \"array.toml\"",
    );
    for (name, text) in [
        ("device.toml", device.as_str()),
        ("radio.toml", FRAGMENT),
        ("node.toml", node),
        ("prefix.toml", prefix),
        ("array.toml", array),
        ("memory.toml", MEMORY),
    ] {
        fs::write(path(name), text).expect("valid generated-layout fixture");
    }
    let model = oer_registers_model::RegisterModel::load(&path("device.toml"))
        .map_err(|error| error.to_string())?;
    let memory = oer_registers_model::memory::Memory::load(&path("memory.toml"))
        .expect("valid generated-layout fixture");
    Ok((model, memory))
}

const ARRAY: &str = r#"schema = 1
[layout]
name = "TABLE"
description = "Twelve walker nodes."
size = 96
align = 4
placement = ["dma-window"]
[layout.array]
element = "NODE"
count = 12
"#;

fn ble_node() -> String {
    NODE.replace(
        "{ name = \"ADDRESS\", bits = \"31:0\", writer = \"cpu\", placement = [\"dma-window\"] }",
        "{ name = \"LOW_TAG\", bits = \"3:0\", writer = \"cpu\" },\n{ name = \"ADDRESS\", bits = \"23:4\", writer = \"phased\", phases = [{ name = \"arm\", writer = \"cpu\" }, { name = \"complete\", writer = \"hardware\" }], points-to = \"NODE\", encoding = \"ble-compressed\" },\n{ name = \"HIGH_TAG\", bits = \"31:24\", writer = \"cpu\" }",
    )
}

#[test]
fn invalid_pointer_encodings_and_phase_declarations_are_rejected() {
    let node = ble_node();
    for (input, reason) in [
        (node.replace("bits = \"23:4\"", "bits = \"22:4\""), "20-bit ble-compressed"),
        (node.replace("points-to = \"NODE\", ", ""), "encoding requires placement or points-to"),
        (node.replace("ble-compressed", "unknown"), "unknown variant"),
        (node.replace("writer = \"phased\", phases", "writer = \"cpu\", phases"), "only a phased writer"),
        (node.replace("{ name = \"complete\", writer = \"hardware\" }", "{ name = \"arm\", writer = \"hardware\" }"), "unique"),
        (node.replace("{ name = \"complete\", writer = \"hardware\" }", "{ name = \"\", writer = \"hardware\" }"), "nonempty"),
        (node.replace("{ name = \"complete\", writer = \"hardware\" }", "{ name = \"complete\", writer = \"phased\" }"), "cpu or hardware"),
        (node.replace("phases = [{ name = \"arm\", writer = \"cpu\" }, { name = \"complete\", writer = \"hardware\" }]", "phases = []"), "nonempty phases"),
    ] {
        let error = render_fixture(&input, ARRAY).unwrap_err();
        assert!(error.contains(reason), "{reason}: {error}");
    }
}

#[test]
fn arrays_require_word_or_byte_elements_and_matching_geometry() {
    for (input, reason) in [
        (ARRAY.replace("count = 12", "count = 0"), "nonzero count"),
        (
            ARRAY.replace("count = 12", "count = 11"),
            "array size, alignment or placement",
        ),
        (
            ARRAY.replace("element = \"NODE\"", "element = \"TABLE\""),
            "not a word or byte layout",
        ),
        (
            ARRAY.replace("element = \"NODE\"", "element = \"PREFIX\""),
            "array size, alignment or placement",
        ),
        (
            ARRAY.replace("element = \"NODE\"", "element = \"ABSENT\""),
            "not a word or byte layout",
        ),
        (
            ARRAY.replace("align = 4", "align = 8"),
            "array size, alignment or placement",
        ),
        (
            ARRAY.replace("placement = [\"dma-window\"]", "placement = []"),
            "array size, alignment or placement",
        ),
    ] {
        let error = render_fixture(NODE, &input).unwrap_err();
        assert!(error.contains(reason), "{reason}: {error}");
    }
}

#[test]
fn rendered_layouts_are_rust_items() {
    let directory = tempfile::tempdir().unwrap();
    let path = |name: &str| directory.path().join(name);
    for (name, text) in [
        ("device.toml", DEVICE),
        ("radio.toml", FRAGMENT),
        ("node.toml", NODE),
        ("prefix.toml", PREFIX),
        ("memory.toml", MEMORY),
    ] {
        fs::write(path(name), text).unwrap();
    }
    // A draft: the fixture's entities are unreviewed.
    let model = oer_registers_model::RegisterModel::load(&path("device.toml")).unwrap();
    let memory = oer_registers_model::memory::Memory::load(&path("memory.toml")).unwrap();
    let rust = model.render_layouts_rust(&memory).unwrap();
    let file = syn::parse_file(&rust).unwrap();
    let [syn::Item::Mod(layouts)] = file.items.as_slice() else {
        panic!("one layouts module")
    };
    let names = layouts
        .content
        .as_ref()
        .unwrap()
        .1
        .iter()
        .filter_map(|item| match item {
            syn::Item::Struct(item) => Some(item.ident.to_string()),
            syn::Item::Const(item) => Some(item.ident.to_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        names,
        [
            "DMA_WINDOW",
            "DmaWindow",
            "Node",
            "_",
            "NodeControlCell",
            "NodeControl",
            "NodeBufferCell",
            "NodeBuffer",
            "Prefix",
            "PrefixRate",
            "PrefixStamp",
        ]
    );
}

#[test]
fn a_layout_is_placed_only_in_ram_regions_of_the_memory_map() {
    let directory = tempfile::tempdir().unwrap();
    let path = |name: &str| directory.path().join(name);
    for (name, text) in [
        ("device.toml", DEVICE),
        ("radio.toml", FRAGMENT),
        ("node.toml", NODE),
        ("prefix.toml", PREFIX),
    ] {
        fs::write(path(name), text).unwrap();
    }
    let model = oer_registers_model::RegisterModel::load(&path("device.toml")).unwrap();
    let evidence = oer_registers_model::RegisterEvidenceSet {
        sources: vec![],
        ranges: vec![],
    };
    let owned = ["radio".to_owned()];
    for (memory, valid) in [
        (MEMORY.to_owned(), true),
        (MEMORY.replace("kind = \"ram\"", "kind = \"mmio\""), false),
        (MEMORY.replace("\"dma-window\"", "\"sram\""), false),
    ] {
        fs::write(path("memory.toml"), memory).unwrap();
        let memory = oer_registers_model::memory::Memory::load(&path("memory.toml")).unwrap();
        let result = memory.validate(&model, &owned, &evidence);
        assert_eq!(result.is_ok(), valid, "{result:?}");
        if !valid {
            assert!(
                result
                    .unwrap_err()
                    .to_string()
                    .contains("layout NODE is placed in dma-window, which is not a RAM region")
            );
        }
    }
}

const BYTE_ARRAY: &str = r#"schema = 1
[layout]
name = "TABLE"
description = "Three owned byte-prefix elements."
size = 24
align = 1
[layout.array]
element = "PREFIX"
count = 3
"#;

#[test]
fn a_byte_array_is_a_buffer_even_when_it_names_the_dma_region() {
    let array = BYTE_ARRAY.replace("align = 1", "align = 1\nplacement = [\"dma-window\"]");
    let prefix = PREFIX.replace("align = 1", "align = 1\nplacement = [\"dma-window\"]");
    let node = NODE.replace("placement = [\"dma-window\"] }", "points-to = \"TABLE\" }");
    let error = render_fixture_with_prefix(&node, &array, &prefix)
        .expect_err("bytes are not a typed pointer target");
    assert!(error.contains("not a word layout"), "{error}");
}
