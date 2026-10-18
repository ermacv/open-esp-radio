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
    let model = oer_tool_data_register_model::RegisterModel::load(&path("device.toml")).unwrap();
    let memory = oer_tool_data_register_model::memory::Memory::load(&path("memory.toml")).unwrap();
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
            "DmaWindowAddress",
            "DmaRef",
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
    let model = oer_tool_data_register_model::RegisterModel::load(&path("device.toml")).unwrap();
    let evidence = oer_tool_data_register_model::RegisterEvidenceSet {
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
        let memory =
            oer_tool_data_register_model::memory::Memory::load(&path("memory.toml")).unwrap();
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

#[test]
fn the_linker_fragment_names_the_dma_window_and_the_address_width() {
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
    let model = oer_tool_data_register_model::RegisterModel::load(&path("device.toml")).unwrap();
    let memory = oer_tool_data_register_model::memory::Memory::load(&path("memory.toml")).unwrap();
    let (svd, _) = model.render_svd().unwrap();
    let fragment = oer_tool_data_register_model::render_dma_linker_fragment(
        &svd,
        &memory,
        Some("RADIO.NEXT.ADDRESS_LOW"),
    )
    .unwrap();
    for line in [
        "DMA_WINDOW_ORIGIN = 0x30000000;",
        "DMA_WINDOW_LENGTH = 0x00080000;",
        "DMA_ADDRESS_LOW_BITS = 20;",
        "\"runtime DMA storage lies outside the DMA window\");",
    ] {
        assert!(fragment.contains(line), "{line}\n{fragment}");
    }
    // Without a DMA address field no window is known: the link refuses any
    // DMA storage.
    let none =
        oer_tool_data_register_model::render_dma_linker_fragment(&svd, &memory, None).unwrap();
    assert!(none.contains("No DMA window known"), "{none}");
    assert!(
        none.contains("ASSERT(__runtime_dma_bss_end <= __runtime_dma_data_start,"),
        "{none}"
    );
    assert!(!none.contains("DMA_WINDOW_ORIGIN"), "{none}");
    // The field must be a low field of its register, and must exist.
    for (field, reason) in [
        ("RADIO.STATUS.FLAG", "bits N-1:0"),
        ("RADIO.NEXT.MISSING", "unknown field"),
        ("RADIO.NEXT", "PERIPHERAL.REGISTER.FIELD"),
    ] {
        let error =
            oer_tool_data_register_model::render_dma_linker_fragment(&svd, &memory, Some(field))
                .unwrap_err()
                .to_string();
        assert!(error.contains(reason), "{field}: {error}");
    }
    // A memory map without the DMA region has no fragment.
    fs::write(
        path("memory.toml"),
        MEMORY.replace("\"dma-window\"", "\"sram\""),
    )
    .unwrap();
    let memory = oer_tool_data_register_model::memory::Memory::load(&path("memory.toml")).unwrap();
    let error = oer_tool_data_register_model::render_dma_linker_fragment(
        &svd,
        &memory,
        Some("RADIO.NEXT.ADDRESS_LOW"),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("no RAM region dma-window"), "{error}");
}
