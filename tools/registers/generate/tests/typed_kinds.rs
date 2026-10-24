//! Run the typed renderer's write-trigger field and FIFO port over host
//! memory: the policy declares both kinds, the rendered block is compiled
//! against `oer-mmio` in its own directory and its tests observe the words.
use std::fs;

/// `RADIO`: `CONFIG` holds ordinary fields and two write-trigger fields, one
/// write-only and one the SVD calls read-write; `DATA` is a FIFO port.
const SVD: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<device schemaVersion="1.3"><name>TEST</name><addressUnitBits>8</addressUnitBits>
<width>32</width><size>32</size><access>read-write</access><peripherals>
<peripheral><name>RADIO</name><baseAddress>0x60000000</baseAddress><registers>
<register><name>CONFIG</name><addressOffset>0x0</addressOffset><size>32</size><access>read-write</access><fields>
<field><name>ENABLE</name><bitOffset>0</bitOffset><bitWidth>1</bitWidth></field>
<field><name>LEVEL</name><bitOffset>1</bitOffset><bitWidth>3</bitWidth></field>
<field><name>RESET</name><bitOffset>30</bitOffset><bitWidth>1</bitWidth></field>
<field><name>UPDATE</name><bitOffset>31</bitOffset><bitWidth>1</bitWidth><access>write-only</access></field>
</fields></register>
<register><name>DATA</name><addressOffset>0x4</addressOffset><size>32</size><access>read-write</access><fields>
<field><name>BYTE</name><bitOffset>0</bitOffset><bitWidth>8</bitWidth></field>
</fields></register>
</registers></peripheral>
</peripherals></device>"#;

const POLICY: &str = r#"schema = 5

[options]
typed-peripherals = ["RADIO"]

[[write-trigger-fields]]
peripheral = "RADIO"
register = "CONFIG"
field = "UPDATE"
sources = ["FIXTURE"]

[[write-trigger-fields]]
peripheral = "RADIO"
register = "CONFIG"
field = "RESET"
sources = ["FIXTURE"]

[[fifo-ports]]
peripheral = "RADIO"
register = "DATA"
sources = ["FIXTURE"]
"#;

/// The fixture's tests; the bit positions are the fixture SVD's own.
const TESTS: &str = r#"
const ENABLE: u32 = 1 << 0;
const RESET: u32 = 1 << 30;
const UPDATE: u32 = 1 << 31;
const CONFIG: usize = 0;
const DATA: usize = 1;

fn memory() -> Vec<u32> {
    vec![u32::MAX; radio::SIZE / 4]
}

#[test]
fn a_read_modify_write_writes_zero_to_every_trigger() {
    let mut memory = memory();
    let mut block = radio::Radio::host(&mut memory).expect("memory holds the block");
    block.modify_config(|writer| {
        writer.set_enable(false);
    });
    assert!(block.read_config().level().get() == 0b111, "other fields keep the value read");
    drop(block);
    assert_eq!(memory[CONFIG], u32::MAX & !(ENABLE | RESET | UPDATE));
    assert_eq!(memory[DATA], u32::MAX, "the FIFO port is untouched");
}

#[test]
fn an_explicit_trigger_sets_its_field_and_no_other_trigger() {
    let mut memory = memory();
    let mut block = radio::Radio::host(&mut memory).expect("memory holds the block");
    block.trigger_config_update();
    drop(block);
    assert_eq!(memory[CONFIG], u32::MAX & !RESET);

    let mut memory = vec![ENABLE; radio::SIZE / 4];
    let mut block = radio::Radio::host(&mut memory).expect("memory holds the block");
    block.trigger_config_reset();
    drop(block);
    assert_eq!(memory[CONFIG], ENABLE | RESET, "ordinary fields keep the value read");
}

#[test]
fn a_fifo_port_pushes_from_zero_and_pops_its_field() {
    let mut memory = memory();
    let mut block = radio::Radio::host(&mut memory).expect("memory holds the block");
    block.push_data(0x5a);
    drop(block);
    assert_eq!(memory[DATA], 0x5a, "a push writes zero beside the element");
    memory[DATA] = 0xffff_ff21;
    let mut block = radio::Radio::host(&mut memory).expect("memory holds the block");
    assert_eq!(block.pop_data(), 0x21);
    drop(block);
    assert_eq!(memory[CONFIG], u32::MAX, "the other register is untouched");
}
"#;

#[test]
fn write_trigger_fields_and_fifo_ports_behave_over_host_memory() {
    let directory = tempfile::tempdir().expect("typed-kind fixture directory");
    let path = |name| directory.path().join(name);
    fs::write(path("api.toml"), POLICY).unwrap();
    let policy = oer_registers_model::PacApiPack::load(&path("api.toml")).unwrap();
    let block = policy.render_typed_rust(SVD, &Default::default()).unwrap();
    let mmio = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../portable/mmio");
    fs::write(
        path("Cargo.toml"),
        format!(
            r#"[package]
name = "typed-kind-fixture"
version = "0.0.0"
edition = "2024"
[dependencies]
oer-mmio = {{ path = {:?} }}
[workspace]
"#,
            mmio
        ),
    )
    .unwrap();
    fs::write(
        path("rust-toolchain.toml"),
        include_str!("../../../../rust-toolchain.toml"),
    )
    .unwrap();
    fs::create_dir(path("src")).unwrap();
    fs::write(path("src/lib.rs"), format!("{block}\n{TESTS}")).unwrap();
    let output = oer_os_toolchain::cargo_in(directory.path())
        .args(["test", "--offline", "--quiet"])
        .env("CARGO_TARGET_DIR", path("target"))
        .output()
        .expect("pinned Cargo typed-kind fixture");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
