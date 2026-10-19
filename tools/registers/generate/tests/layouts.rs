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
    model
        .render_layouts_rust(&memory)
        .map_err(|error| error.to_string())
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
        "{ name = \"ADDRESS\", bits = \"23:4\", writer = \"phased\", phases = [{ name = \"arm\", writer = \"cpu\" }, { name = \"complete\", writer = \"hardware\" }], points-to = \"NODE\", encoding = \"ble-compressed\" }",
    )
}

/// Compile and execute the generated types, with only their two storage
/// dependencies supplied locally. This checks the actual generated methods.
fn run_generated(rust: &str, checks: &str) {
    let directory = tempfile::tempdir().expect("valid generated-layout fixture");
    let source = directory.path().join("layout.rs");
    let executable = directory.path().join("layout-test");
    fs::write(
        &source,
        format!(
            r#"
extern crate self as vcell;
extern crate self as bytemuck;
pub struct VolatileCell<T>(std::cell::UnsafeCell<T>);
impl<T: Copy> VolatileCell<T> {{
    pub const fn new(value: T) -> Self {{ Self(std::cell::UnsafeCell::new(value)) }}
    pub fn get(&self) -> T {{ unsafe {{ *self.0.get() }} }}
    pub fn set(&self, value: T) {{ unsafe {{ *self.0.get() = value; }} }}
}}
pub unsafe trait Zeroable {{}}
{rust}
fn main() {{ {checks} }}
"#
        ),
    )
    .expect("valid generated-layout fixture");
    let output = std::process::Command::new("rustc")
        .arg("--edition=2024")
        .arg(&source)
        .arg("-o")
        .arg(&executable)
        .output()
        .expect("valid generated-layout fixture");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output = std::process::Command::new(executable)
        .output()
        .expect("valid generated-layout fixture");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn ble_links_round_trip_preserve_other_fields_and_reject_invalid_references() {
    let rust = render_fixture(&ble_node(), ARRAY).expect("valid generated-layout fixture");
    assert!(rust.contains("Phase `arm`: the CPU. Phase `complete`: the hardware."));
    run_generated(
        &rust,
        r#"
        use layouts::*;
        // Synthetic references never escape this host model or access memory.
        let reference = |address| unsafe {
            DmaRef::<Node>::new_unchecked(DmaWindowAddress::new_unchecked(address))
        };
        let original = NodeBuffer::from_bits(u32::MAX);
        for address in [0x2f00_0004, 0x2f00_1340, 0x2f07_fffc] {
            let value = original.with_address(Some(reference(address))).expect("valid generated-layout fixture");
            assert_eq!(value.address_address(), Some(address));
            assert_eq!(value.with_address(None).expect("valid generated-layout fixture"), original.with_address(None).expect("valid generated-layout fixture"));
        }
        let empty = original.with_address(None).expect("valid generated-layout fixture");
        assert_eq!(empty.address_address(), None);
        assert_ne!(empty.bits(), 0, "neighboring fields survive clearing a link");
        for address in [0x2f00_0000, 0x2f00_0001, 0x3000_0100, 0x2f40_0004] {
            assert!(original.with_address(Some(reference(address))).is_none());
        }
        let table = Table::zeroed();
        table.entry(11).expect("valid generated-layout fixture").control().write(NodeControl::ZERO.with_owner(true));
        assert!(table.entry(11).expect("valid generated-layout fixture").control().read().owner());
        assert!(!table.entry(0).expect("valid generated-layout fixture").control().read().owner());
        assert!(table.entry(12).is_none());
        assert!(table.entry(usize::MAX).is_none());
    "#,
    );
}

#[test]
fn invalid_pointer_encodings_and_phase_declarations_are_rejected() {
    let node = ble_node();
    for (input, reason) in [
        (node.replace("bits = \"23:4\"", "bits = \"22:4\""), "20-bit ble-compressed"),
        (node.replace("points-to = \"NODE\", ", ""), "encoding requires points-to"),
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
fn arrays_require_shared_elements_and_matching_geometry() {
    for (input, reason) in [
        (ARRAY.replace("count = 12", "count = 0"), "nonzero count"),
        (
            ARRAY.replace("count = 12", "count = 11"),
            "array size, alignment or placement",
        ),
        (
            ARRAY.replace("element = \"NODE\"", "element = \"TABLE\""),
            "not a word layout",
        ),
        (
            ARRAY.replace("element = \"NODE\"", "element = \"PREFIX\""),
            "not a word layout",
        ),
        (
            ARRAY.replace("element = \"NODE\"", "element = \"ABSENT\""),
            "not a word layout",
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
fn a_pointer_can_target_an_array_layout() {
    let node = ble_node().replace("points-to = \"NODE\"", "points-to = \"TABLE\"");
    let rust = render_fixture(&node, ARRAY).expect("valid generated-layout fixture");
    run_generated(
        &rust,
        r#"
        use layouts::*;
        let reference = unsafe {
            DmaRef::<Table>::new_unchecked(DmaWindowAddress::new_unchecked(0x2f00_0100))
        };
        let value = NodeBuffer::ZERO.with_address(Some(reference)).expect("valid generated-layout fixture");
        assert_eq!(value.address_address(), Some(reference.bits()));
    "#,
    );
}

#[test]
fn an_owned_range_written_during_a_cpu_phase_gets_a_writer() {
    let prefix = PREFIX.replace(
        "writer = \"hardware\"",
        "writer = \"phased\", phases = [{ name = \"arm\", writer = \"cpu\" }, { name = \"complete\", writer = \"hardware\" }]",
    );
    let rust = render_fixture(NODE, ARRAY).expect("valid layout fixture");
    let phased = render_fixture_with_prefix(NODE, ARRAY, &prefix).expect("valid phased layout");
    assert!(!rust.contains("pub struct PrefixMut"));
    run_generated(
        &phased,
        r#"
        use layouts::*;
        let mut bytes = [0; 8];
        let stamp = PrefixStamp::from_bits(0).with_value(123);
        PrefixMut::new(&mut bytes).set_stamp(stamp);
        assert_eq!(Prefix::new(&bytes).stamp().value(), 123);
    "#,
    );
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
    let model = oer_registers_model::RegisterModel::load(&path("device.toml")).unwrap();
    let memory = oer_registers_model::memory::Memory::load(&path("memory.toml")).unwrap();
    let (svd, _) = model.render_svd().unwrap();
    let fragment = oer_registers_model::render_dma_linker_fragment(
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
    let none = oer_registers_model::render_dma_linker_fragment(&svd, &memory, None).unwrap();
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
        let error = oer_registers_model::render_dma_linker_fragment(&svd, &memory, Some(field))
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
    let memory = oer_registers_model::memory::Memory::load(&path("memory.toml")).unwrap();
    let error = oer_registers_model::render_dma_linker_fragment(
        &svd,
        &memory,
        Some("RADIO.NEXT.ADDRESS_LOW"),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("no RAM region dma-window"), "{error}");
}
