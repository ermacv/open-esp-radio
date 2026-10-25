//! Run the typed renderer's write-trigger field, FIFO port, shared block,
//! command field and data port over host memory: the policy declares the kinds, the rendered blocks are compiled
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
<peripheral><name>COUNTER</name><baseAddress>0x60001000</baseAddress><registers>
<register><name>OP</name><addressOffset>0x0</addressOffset><size>32</size><access>read-write</access><fields>
<field><name>VALID</name><bitOffset>29</bitOffset><bitWidth>1</bitWidth><access>read-only</access></field>
<field><name>UPDATE</name><bitOffset>30</bitOffset><bitWidth>1</bitWidth><access>write-only</access></field>
</fields></register>
<register><name>VALUE</name><addressOffset>0x4</addressOffset><size>32</size><access>read-only</access><fields>
<field><name>LOW</name><bitOffset>0</bitOffset><bitWidth>32</bitWidth></field>
</fields></register>
<register><name>RAISE</name><addressOffset>0x8</addressOffset><size>32</size><access>read-write</access><fields>
<field><name>LEVEL</name><bitOffset>0</bitOffset><bitWidth>1</bitWidth></field>
</fields></register>
</registers></peripheral>
<peripheral><name>CACHE</name><baseAddress>0x60002000</baseAddress><registers>
<register><name>CTRL</name><addressOffset>0x0</addressOffset><size>32</size><access>read-write</access><fields>
<field><name>INVALIDATE</name><bitOffset>0</bitOffset><bitWidth>1</bitWidth></field>
<field><name>CLEAN</name><bitOffset>1</bitOffset><bitWidth>1</bitWidth></field>
<field><name>GO</name><bitOffset>2</bitOffset><bitWidth>1</bitWidth></field>
<field><name>DONE</name><bitOffset>4</bitOffset><bitWidth>1</bitWidth><access>read-only</access></field>
<field><name>RGID</name><bitOffset>5</bitOffset><bitWidth>4</bitWidth></field>
</fields></register>
<register><name>W%s</name><addressOffset>0x4</addressOffset><size>32</size><access>read-write</access>
<dim>4</dim><dimIncrement>4</dimIncrement><fields>
<field><name>BUF</name><bitOffset>0</bitOffset><bitWidth>32</bitWidth></field>
</fields></register>
<register><name>TIE</name><addressOffset>0x14</addressOffset><size>32</size><access>write-only</access><fields>
<field><name>LOW</name><bitOffset>0</bitOffset><bitWidth>4</bitWidth></field>
<field><name>HIGH</name><bitOffset>4</bitOffset><bitWidth>4</bitWidth></field>
</fields></register>
</registers></peripheral>
</peripherals></device>"#;

const POLICY: &str = r#"schema = 5

[options]
typed-peripherals = ["RADIO", "COUNTER", "CACHE"]

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

[[write-trigger-fields]]
peripheral = "COUNTER"
register = "OP"
field = "UPDATE"
sources = ["FIXTURE"]

[[command-fields]]
peripheral = "CACHE"
register = "CTRL"
field = "INVALIDATE"
exclusive = true
sources = ["FIXTURE"]

[[command-fields]]
peripheral = "CACHE"
register = "CTRL"
field = "CLEAN"
exclusive = true
sources = ["FIXTURE"]

[[command-fields]]
peripheral = "CACHE"
register = "CTRL"
field = "GO"
sources = ["FIXTURE"]

[[data-ports]]
peripheral = "CACHE"
register = "W%s"
sources = ["FIXTURE"]

[[write-trigger-fields]]
peripheral = "CACHE"
register = "TIE"
field = "LOW"
sources = ["FIXTURE"]

[[write-trigger-fields]]
peripheral = "CACHE"
register = "TIE"
field = "HIGH"
sources = ["FIXTURE"]

[[shared-blocks]]
peripheral = "COUNTER"
description = "A counter and a raise line every context reaches."
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

#[test]
fn a_shared_block_strobes_and_stores_whole_words_through_shared_borrows() {
    fn sync<T: Sync>() {}
    sync::<counter::Counter>();
    let mut memory = vec![u32::MAX; counter::SIZE / 4];
    let block = counter::Counter::host(&mut memory).expect("memory holds the block");
    let (task, handler) = (&block, &block);
    task.trigger_op_update();
    handler.store_raise(false);
    assert!(!task.read_op().valid(), "the strobe wrote zero beside its trigger");
    assert_eq!(handler.read_value().low(), u32::MAX);
    drop(block);
    assert_eq!(memory, [1 << 30, u32::MAX, 0]);
}

const INVALIDATE: u32 = 1 << 0;
const CLEAN: u32 = 1 << 1;
const GO: u32 = 1 << 2;
const RGID: u32 = 0b1111 << 5;
const CTRL: usize = 0;

#[test]
fn a_read_modify_write_never_replays_a_command() {
    let mut memory = vec![u32::MAX; cache::SIZE / 4];
    let mut block = cache::Cache::host(&mut memory).expect("memory holds the block");
    block.modify_ctrl(|writer| {
        writer.set_rgid(oer_mmio::Bits::constant::<0b0101>());
    });
    drop(block);
    assert_eq!(
        memory[CTRL],
        u32::MAX & !(INVALIDATE | CLEAN | GO) & !(0b1010 << 5),
        "every command is zero, every other bit keeps the value read"
    );
}

#[test]
fn a_start_sets_only_its_command_and_its_busy_reads_it() {
    let mut memory = vec![RGID | INVALIDATE; cache::SIZE / 4];
    let mut block = cache::Cache::host(&mut memory).expect("memory holds the block");
    assert!(block.ctrl_invalidate_busy());
    assert!(!block.ctrl_go_busy());
    block.start_ctrl_go();
    assert!(block.ctrl_go_busy());
    assert!(!block.ctrl_invalidate_busy(), "the running command is not replayed");
    drop(block);
    assert_eq!(memory[CTRL], RGID | GO);
}

#[test]
fn an_exclusive_command_refuses_while_its_group_runs() {
    let mut memory = vec![RGID | CLEAN; cache::SIZE / 4];
    let mut block = cache::Cache::host(&mut memory).expect("memory holds the block");
    assert_eq!(
        block.start_ctrl_invalidate(),
        Err(oer_mmio::CommandBusy { running: CLEAN })
    );
    drop(block);
    assert_eq!(memory[CTRL], RGID | CLEAN, "a refused start writes nothing");
    memory[CTRL] = RGID | GO;
    let mut block = cache::Cache::host(&mut memory).expect("memory holds the block");
    assert_eq!(block.start_ctrl_invalidate(), Ok(()), "GO is outside the group");
    assert_eq!(block.start_ctrl_clean(), Err(oer_mmio::CommandBusy { running: INVALIDATE }));
    drop(block);
    assert_eq!(memory[CTRL], RGID | INVALIDATE);
}

#[test]
fn a_data_port_stores_whole_words_and_a_wide_trigger_writes_its_value() {
    let mut memory = vec![u32::MAX; cache::SIZE / 4];
    let mut block = cache::Cache::host(&mut memory).expect("memory holds the block");
    let index = oer_mmio::Index::constant::<2>();
    block.write_w(index, 0x1234_5678);
    assert_eq!(block.read_w(index).buf(), 0x1234_5678);
    block.trigger_tie_high(oer_mmio::Bits::constant::<0b0101>());
    drop(block);
    assert_eq!(memory[1..5], [u32::MAX, u32::MAX, 0x1234_5678, u32::MAX]);
    assert_eq!(memory[5], 0b0101 << 4, "a write-only trigger word is written from zero");
}
"#;

#[test]
fn write_triggers_fifo_ports_shared_blocks_commands_and_data_ports_behave_over_host_memory() {
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
