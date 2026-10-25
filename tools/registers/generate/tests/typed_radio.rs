//! Run the typed renderer's radio kinds over host memory: typed DMA pointer
//! images, a status-to-clear snapshot, a read capability, a constant-zero
//! field, a sampled-bit write, a whole-word sequence step, a divided block
//! with shared trigger words and array elements, and a partition image. The
//! rendered blocks compile against `oer-mmio` in their own directory and
//! their tests observe the words; each refused use is a feature whose build
//! must fail with its error.
use std::fs;

/// `RADIO` holds the pointer, snapshot, capability and field kinds; `ETM`
/// is divided between two owners.
const SVD: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<device schemaVersion="1.3"><name>TEST</name><addressUnitBits>8</addressUnitBits>
<width>32</width><size>32</size><access>read-write</access><peripherals>
<peripheral><name>RADIO</name><baseAddress>0x60000000</baseAddress><registers>
<register><name>TX_HEAD</name><addressOffset>0x0</addressOffset><size>32</size><access>read-write</access><fields>
<field><name>ADDRESS_LOW</name><bitOffset>0</bitOffset><bitWidth>20</bitWidth></field>
<field><name>FORMAT</name><bitOffset>20</bitOffset><bitWidth>4</bitWidth></field>
</fields></register>
<register><name>LIST</name><addressOffset>0x4</addressOffset><size>32</size><access>write-only</access><fields>
<field><name>INDEX</name><bitOffset>0</bitOffset><bitWidth>20</bitWidth></field>
<field><name>VALID</name><bitOffset>31</bitOffset><bitWidth>1</bitWidth></field>
</fields></register>
<register><name>BASE</name><addressOffset>0x8</addressOffset><size>32</size><access>read-write</access><fields>
<field><name>ADDRESS</name><bitOffset>0</bitOffset><bitWidth>32</bitWidth></field>
</fields></register>
<register><name>STATUS</name><addressOffset>0xc</addressOffset><size>32</size><access>read-only</access><fields>
<field><name>DONE</name><bitOffset>0</bitOffset><bitWidth>1</bitWidth></field>
<field><name>ERROR</name><bitOffset>1</bitOffset><bitWidth>1</bitWidth></field>
<field><name>LEVEL</name><bitOffset>4</bitOffset><bitWidth>4</bitWidth></field>
</fields></register>
<register><name>CLEAR</name><addressOffset>0x10</addressOffset><size>32</size><access>write-only</access><modifiedWriteValues>oneToClear</modifiedWriteValues><fields>
<field><name>DONE</name><bitOffset>0</bitOffset><bitWidth>1</bitWidth></field>
<field><name>ERROR</name><bitOffset>1</bitOffset><bitWidth>1</bitWidth></field>
</fields></register>
<register><name>TIME</name><addressOffset>0x14</addressOffset><size>32</size><access>read-only</access><fields>
<field><name>VALUE</name><bitOffset>0</bitOffset><bitWidth>32</bitWidth></field>
</fields></register>
<register><name>KEYS</name><addressOffset>0x18</addressOffset><size>32</size><access>read-write</access><fields>
<field><name>VALID</name><bitOffset>0</bitOffset><bitWidth>25</bitWidth></field>
<field><name>RESERVED_HIGH</name><bitOffset>25</bitOffset><bitWidth>7</bitWidth><writeConstraint><range><minimum>0</minimum><maximum>0</maximum></range></writeConstraint></field>
</fields></register>
<register><name>INTERRUPT_ENABLE</name><addressOffset>0x1c</addressOffset><size>32</size><access>read-write</access><fields>
<field><name>SOURCE_A</name><bitOffset>0</bitOffset><bitWidth>1</bitWidth></field>
<field><name>SOURCE_B</name><bitOffset>1</bitOffset><bitWidth>1</bitWidth></field>
</fields></register>
<register><name>WORD</name><addressOffset>0x20</addressOffset><size>32</size><access>read-write</access><fields>
<field><name>VALUE</name><bitOffset>0</bitOffset><bitWidth>16</bitWidth></field>
</fields></register>
</registers></peripheral>
<peripheral><name>ETM</name><baseAddress>0x60001000</baseAddress><registers>
<register><name>ENABLE</name><addressOffset>0x0</addressOffset><size>32</size><access>read-only</access><fields>
<field><name>CH0</name><bitOffset>0</bitOffset><bitWidth>1</bitWidth></field>
<field><name>CH1</name><bitOffset>1</bitOffset><bitWidth>1</bitWidth></field>
<field><name>CH2</name><bitOffset>2</bitOffset><bitWidth>1</bitWidth></field>
<field><name>CH3</name><bitOffset>3</bitOffset><bitWidth>1</bitWidth></field>
</fields></register>
<register><name>ENABLE_SET</name><addressOffset>0x4</addressOffset><size>32</size><access>write-only</access><fields>
<field><name>CH0</name><bitOffset>0</bitOffset><bitWidth>1</bitWidth></field>
<field><name>CH1</name><bitOffset>1</bitOffset><bitWidth>1</bitWidth></field>
<field><name>CH2</name><bitOffset>2</bitOffset><bitWidth>1</bitWidth></field>
<field><name>CH3</name><bitOffset>3</bitOffset><bitWidth>1</bitWidth></field>
</fields></register>
<register><dim>4</dim><dimIncrement>0x8</dimIncrement><name>CH%s_EVENT</name><addressOffset>0x10</addressOffset><size>32</size><access>read-write</access><fields>
<field><name>ID</name><bitOffset>0</bitOffset><bitWidth>8</bitWidth></field>
</fields></register>
<register><dim>4</dim><dimIncrement>0x8</dimIncrement><name>CH%s_TASK</name><addressOffset>0x14</addressOffset><size>32</size><access>read-write</access><fields>
<field><name>ID</name><bitOffset>0</bitOffset><bitWidth>8</bitWidth></field>
</fields></register>
</registers></peripheral>
</peripherals></device>"#;

const POLICY: &str = r#"schema = 5

[options]
typed-peripherals = ["RADIO", "ETM"]

[[ownership-partitions]]
name = "RadioPeripherals"
member = "radio"
description = "The fixture's blocks."
peripherals = ["RADIO", "ETM"]

[[partition-image-reads]]
name = "radio_image"
partition = "RadioPeripherals"
exposure = "facade"
sources = ["FIXTURE"]

[[pointer-fields]]
name = "tx_head_pointer"
peripheral = "RADIO"
register = "TX_HEAD"
field = "ADDRESS_LOW"
points-to = "DESCRIPTOR"
encoding = "low-bits"
sources = ["FIXTURE"]

[[pointer-fields]]
name = "list_pointer"
peripheral = "RADIO"
register = "LIST"
field = "INDEX"
points-to = "DESCRIPTOR"
encoding = "ble-compressed"
sources = ["FIXTURE"]

[[pointer-fields]]
name = "base_member_pointer"
peripheral = "RADIO"
register = "BASE"
field = "ADDRESS"
points-to = "DESCRIPTOR"
member-offset = 0x2c
sources = ["FIXTURE"]

[[zero-based-field-writes]]
name = "publish_list"
peripheral = "RADIO"
register = "LIST"
fields = ["INDEX", "VALID"]
sources = ["FIXTURE"]

[[field-replace-modifies]]
name = "clear_tx_head"
peripheral = "RADIO"
register = "TX_HEAD"
fields = [{ field = "ADDRESS_LOW", source-bit-offset = 0 }]
value = 0
exposure = "facade"
sources = ["FIXTURE"]

[[field-or-modifies]]
name = "or_tx_head"
peripheral = "RADIO"
register = "TX_HEAD"
fields = [{ field = "ADDRESS_LOW", source-bit-offset = 0 }]
exposure = "facade"
sources = ["FIXTURE"]

[[fixed-register-images]]
name = "clear_stale_error"
peripheral = "RADIO"
register = "CLEAR"
value = 0x00000002
masks = "INTERRUPT_ENABLE"
sources = ["FIXTURE"]

[[interrupt-snapshots]]
name = "events"
peripheral = "RADIO"
status-register = "STATUS"
clear-register = "CLEAR"
sources = ["FIXTURE"]

[[read-capabilities]]
peripheral = "RADIO"
name = "Time"
registers = ["TIME"]
copy = true
description = "The free-running time, read from every context."
sources = ["FIXTURE"]

[[sampled-bit-zero-writes]]
name = "keep_source_b"
peripheral = "RADIO"
register = "INTERRUPT_ENABLE"
field = "SOURCE_B"
sources = ["FIXTURE"]

[[fixed-register-sequences]]
name = "vendor_word"
peripheral = "RADIO"
steps = [
    { register = "WORD", value = 0x0000ffff },
    { register = "WORD", value = 0xffffffff, whole-word = true },
]
sources = ["FIXTURE"]

[[write-trigger-fields]]
peripheral = "ETM"
register = "ENABLE_SET"
field = "CH0"
sources = ["FIXTURE"]

[[write-trigger-fields]]
peripheral = "ETM"
register = "ENABLE_SET"
field = "CH1"
sources = ["FIXTURE"]

[[write-trigger-fields]]
peripheral = "ETM"
register = "ENABLE_SET"
field = "CH2"
sources = ["FIXTURE"]

[[write-trigger-fields]]
peripheral = "ETM"
register = "ENABLE_SET"
field = "CH3"
sources = ["FIXTURE"]

[[fixed-register-images]]
name = "enable_high_channels"
peripheral = "ETM"
register = "ENABLE_SET"
value = 0x0000000c
sources = ["FIXTURE"]

[[fixed-register-sequences]]
name = "route_channel_2"
peripheral = "ETM"
steps = [
    { register = "CH%s_EVENT", index = 2, value = 8 },
    { register = "CH%s_TASK", index = 2, value = 20 },
    { register = "ENABLE_SET", value = 0x00000004 },
]
sources = ["FIXTURE"]

[[register-divisions]]
peripheral = "ETM"
description = "Channels zero and one for one protocol, two and three for another."
sources = ["FIXTURE"]

[[register-divisions.parts]]
name = "Low"
elements = [
    { register = "CH%s_EVENT", first = 0, count = 2 },
    { register = "CH%s_TASK", first = 0, count = 2 },
]
observes = ["ENABLE"]
triggers = [{ register = "ENABLE_SET", fields = ["CH0", "CH1"] }]

[[register-divisions.parts]]
name = "High"
elements = [
    { register = "CH%s_EVENT", first = 2, count = 2 },
    { register = "CH%s_TASK", first = 2, count = 2 },
]
observes = ["ENABLE"]
triggers = [{ register = "ENABLE_SET", fields = ["CH2", "CH3"] }]
"#;

/// The DMA window and the one layout the pointer fields name, as a chip's
/// generated `layouts` module has them.
const LAYOUTS: &str = r#"
pub mod layouts {
    pub struct DmaWindow;
    impl oer_mmio::DmaWindow for DmaWindow {
        const RANGE: core::ops::Range<u32> = 0x2f00_0000..0x2f08_0000;
    }
    pub type DmaWindowAddress = oer_mmio::DmaWindowAddress<DmaWindow>;
    pub use oer_mmio::DmaRef;
    pub struct Descriptor;
    impl oer_mmio::DmaLayout for Descriptor {
        type Window = DmaWindow;
    }
    /// A descriptor at `address`, as the fixture places it.
    pub fn descriptor(address: u32) -> DmaRef<Descriptor> {
        // SAFETY: a fixture address no hardware follows.
        unsafe { DmaRef::new_unchecked(DmaWindowAddress::new_unchecked(address)) }
    }
}
"#;

/// The fixture's tests; the bit positions are the fixture SVD's own.
const TESTS: &str = r#"
use layouts::descriptor;

const TX_HEAD: usize = 0;
const LIST: usize = 1;
const BASE: usize = 2;
const STATUS: usize = 3;
const CLEAR: usize = 4;
const TIME: usize = 5;
const KEYS: usize = 6;
const INTERRUPT_ENABLE: usize = 7;
const WORD: usize = 8;

fn memory() -> Vec<u32> {
    vec![0; radio::SIZE / 4]
}

#[test]
fn a_pointer_field_takes_only_the_image_of_a_reference() {
    let mut memory = memory();
    memory[TX_HEAD] = 0x0050_0000;
    let mut block = radio::Radio::host(&mut memory).expect("memory holds the block");
    let head = radio::tx_head::AddressLowImage::new(descriptor(0x2f01_2340));
    block.modify_tx_head(|writer| {
        writer.set_address_low(head);
    });
    let list = radio::list::IndexImage::new(descriptor(0x2f00_0100)).expect("an encodable address");
    block.publish_list(list, true);
    block.modify_base(|writer| {
        writer.set_address(radio::base::AddressImage::new(descriptor(0x2f00_1000)));
    });
    drop(block);
    assert_eq!(memory[TX_HEAD], 0x0051_2340, "the low bits, the other field kept");
    assert_eq!(memory[LIST], 0x8000_0040, "the word index and the flag, from zero");
    assert_eq!(memory[BASE], 0x2f00_102c, "the member's address");
    assert_eq!(radio::list::IndexImage::new(descriptor(0x2f00_0000)), None);
    assert_eq!(radio::list::IndexImage::new(descriptor(0x2f00_0102)), None);
    assert_eq!(radio::list::IndexImage::NULL.bits(), 0);
}

#[test]
fn a_pointer_field_is_cleared_and_then_ored_with_an_image() {
    let mut memory = memory();
    memory[TX_HEAD] = 0x0050_0001;
    let mut block = radio::Radio::host(&mut memory).expect("memory holds the block");
    block.clear_tx_head();
    drop(block);
    assert_eq!(memory[TX_HEAD], 0x0050_0000, "the field cleared, the other one kept");
    // A bit the hardware published between the two transactions stays.
    memory[TX_HEAD] |= 0x0000_0008;
    let mut block = radio::Radio::host(&mut memory).expect("memory holds the block");
    block.or_tx_head(radio::tx_head::AddressLowImage::new(descriptor(0x2f01_2340)));
    drop(block);
    assert_eq!(memory[TX_HEAD], 0x0051_2348, "the image ORed into the word read");
}

#[test]
fn a_fixed_image_acknowledges_its_named_source_beside_the_snapshot() {
    let mut memory = memory();
    // ERROR is enabled and pending: the clear masks it before acknowledging.
    memory[STATUS] = 0b11;
    memory[INTERRUPT_ENABLE] = 0xffff_ffff;
    let block = radio::Radio::host(&mut memory).expect("memory holds the block");
    let (mut rest, _time) = block.split_time();
    rest.clear_stale_error();
    drop(rest);
    assert_eq!(memory[CLEAR], 0b10, "exactly the reviewed image");
    assert_eq!(
        memory[INTERRUPT_ENABLE],
        0xffff_fffd,
        "its source masked, every other bit kept"
    );
}

#[test]
fn a_snapshot_acknowledges_exactly_the_sampled_events() {
    let mut memory = memory();
    memory[STATUS] = 0xffff_fff3;
    let block = radio::Radio::host(&mut memory).expect("memory holds the block");
    let (mut rest, _time) = block.split_time();
    let snapshot = rest.sample_events();
    assert!(snapshot.done() && snapshot.error());
    assert_eq!(radio::EventsSnapshot::EVENTS, 0b11, "LEVEL is not acknowledged");
    rest.acknowledge_events(snapshot.retain_error(false));
    drop(rest);
    assert_eq!(memory[CLEAR], 0b01, "one where sampled and kept, zero elsewhere");
}

#[test]
fn a_read_capability_is_copied_and_only_reads() {
    let mut memory = memory();
    memory[TIME] = 41;
    let block = radio::Radio::host(&mut memory).expect("memory holds the block");
    let (mut rest, time) = block.split_time();
    let copy = time;
    rest.modify_keys(|writer| {
        writer.set_valid(oer_mmio::Bits::constant::<3>());
    });
    assert_eq!(time.read_time().value(), 41);
    assert_eq!(copy.read_time().value(), 41);
    drop(rest);
    assert_eq!(memory[KEYS], 3);
}

#[test]
fn a_constant_zero_field_is_zero_in_every_write() {
    let mut memory = memory();
    memory[KEYS] = 0xfe00_0001;
    let block = radio::Radio::host(&mut memory).expect("memory holds the block");
    let (mut rest, _time) = block.split_time();
    rest.modify_keys(|writer| {
        writer.set_valid(oer_mmio::Bits::constant::<5>());
    });
    drop(rest);
    assert_eq!(memory[KEYS], 5);
    assert_eq!(radio::keys::ZERO, 0xfe00_0000);
}

#[test]
fn a_sampled_bit_write_keeps_the_bit_read_and_zeroes_the_rest() {
    let mut memory = memory();
    memory[INTERRUPT_ENABLE] = 0xffff_ffff;
    let block = radio::Radio::host(&mut memory).expect("memory holds the block");
    let (mut rest, _time) = block.split_time();
    rest.keep_source_b();
    drop(rest);
    assert_eq!(memory[INTERRUPT_ENABLE], 0b10);
}

#[test]
fn a_whole_word_step_writes_the_vendor_word() {
    let mut memory = memory();
    let block = radio::Radio::host(&mut memory).expect("memory holds the block");
    let (mut rest, _time) = block.split_time();
    rest.vendor_word();
    drop(rest);
    assert_eq!(memory[WORD], 0xffff_ffff);
}

const ENABLE: usize = 0;
const ENABLE_SET: usize = 1;
const EVENT: usize = 4;
const TASK: usize = 5;

#[test]
fn divided_owners_write_their_elements_and_trigger_bits_only() {
    let mut memory = vec![0; etm::SIZE / 4];
    memory[ENABLE] = 0b0101;
    let block = etm::Etm::host(&mut memory).expect("memory holds the block");
    let (mut low, mut high) = block.divide();
    assert!(low.read_enable().ch0() && high.read_enable().ch2());
    low.modify_ch_event(oer_mmio::Index::constant::<1>(), |writer| {
        writer.set_id(7);
    });
    // The high owner indexes its own elements from zero: its 1 is element 3.
    high.modify_ch_task(oer_mmio::Index::constant::<1>(), |writer| {
        writer.set_id(9);
    });
    high.route_channel_2();
    high.enable_high_channels();
    low.trigger_enable_set_ch1();
    drop((low, high));
    assert_eq!(memory[EVENT + 2], 7, "the low owner's element 1");
    assert_eq!(memory[TASK + 6], 9, "the high owner's element 1 is element 3");
    assert_eq!(memory[EVENT + 4], 8);
    assert_eq!(memory[TASK + 4], 20);
    assert_eq!(
        memory[ENABLE_SET], 0b0010,
        "the last store carries only its owner's bit, zero in the others'"
    );
}

#[test]
fn a_partitioned_block_reads_any_readable_word_for_its_image() {
    let mut memory = vec![0; etm::SIZE / 4];
    memory[TASK + 6] = 0x33;
    let block = etm::Etm::host(&mut memory).expect("memory holds the block");
    assert_eq!(block.read_image_word(0x2c), Some(0x33));
    assert_eq!(block.read_image_word(0x4), None, "a write-only word is no image word");
    assert_eq!(peripheral_ownership::RadioPeripherals::RADIO_IMAGE_LEN, 16);
}
"#;

/// Uses the API refuses, each the feature of the same name, with the error
/// its build must fail with.
const REFUSED: &[(&str, &str, &str)] = &[
    (
        "pointer_from_integer",
        r#"pub fn refused(block: &mut radio::Radio<oer_mmio::Host<'_>>) {
    block.modify_tx_head(|writer| {
        writer.set_address_low(oer_mmio::Bits::constant::<5>());
    });
}"#,
        "E0308",
    ),
    (
        "acknowledge_unsampled",
        r#"pub fn refused(block: &mut radio::Radio<oer_mmio::Host<'_>>) {
    block.write_clear(|writer| {
        writer.set_done(true);
    });
}"#,
        "E0599",
    ),
    (
        "capability_writes",
        r#"pub fn refused(time: radio::RadioTime<oer_mmio::Host<'_>>) {
    let _ = time.sample_events();
}"#,
        "E0599",
    ),
    (
        "other_owners_trigger",
        r#"pub fn refused(low: &mut etm::EtmLow<oer_mmio::Host<'_>>) {
    low.trigger_enable_set_ch2();
}"#,
        "E0599",
    ),
    (
        "join_of_a_division",
        r#"struct Three;
impl oer_mmio::Registers for Three {
    const WORDS: &'static [usize] = &[0, 4, 8];
}
struct First;
impl oer_mmio::Words for First {
    const OWNED: &'static [usize] = &[0];
    const OBSERVED: &'static [usize] = &[];
}
struct Second;
impl oer_mmio::Words for Second {
    const OWNED: &'static [usize] = &[4];
    const OBSERVED: &'static [usize] = &[];
}
struct Third;
impl oer_mmio::Words for Third {
    const OWNED: &'static [usize] = &[8];
    const OBSERVED: &'static [usize] = &[];
}
pub fn refused(memory: &mut [u32]) {
    let block = oer_mmio::Block::<oer_mmio::Host<'_>, Three>::host(memory).expect("three words");
    let (first, second, third) = block.divide::<(First, Second, Third)>();
    drop((oer_mmio::Block::try_join(first, second).ok(), third));
}"#,
        "the joined parts own every word of the block",
    ),
    (
        "shared_trigger_bit",
        r#"struct Pair;
impl oer_mmio::Registers for Pair {
    const WORDS: &'static [usize] = &[0];
}
struct Low;
impl oer_mmio::Words for Low {
    const OWNED: &'static [usize] = &[];
    const OBSERVED: &'static [usize] = &[];
    const TRIGGERS: &'static [(usize, u32)] = &[(0, 0b011)];
}
struct High;
impl oer_mmio::Words for High {
    const OWNED: &'static [usize] = &[];
    const OBSERVED: &'static [usize] = &[];
    const TRIGGERS: &'static [(usize, u32)] = &[(0, 0b110)];
}
pub fn refused(memory: &mut [u32]) {
    let block = oer_mmio::Block::<oer_mmio::Host<'_>, Pair>::host(memory).expect("one word");
    drop(block.divide::<(Low, High)>());
}"#,
        "a shared trigger word is owned by no part and its parts' bits are disjoint",
    ),
];

#[test]
fn radio_kinds_behave_over_host_memory_and_refuse_what_they_forbid() {
    let directory = tempfile::tempdir().expect("typed-radio fixture directory");
    let path = |name| directory.path().join(name);
    fs::write(path("api.toml"), POLICY).unwrap();
    let policy = oer_registers_model::PacApiPack::load(&path("api.toml")).unwrap();
    let blocks = policy.render_typed_rust(SVD, &Default::default()).unwrap();
    for unsafe_item in ["unsafe {", "unsafe fn", "unsafe impl", "allow(unsafe"] {
        assert!(!blocks.contains(unsafe_item), "{unsafe_item}\n{blocks}");
    }
    let mmio = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../portable/mmio");
    let features = REFUSED
        .iter()
        .map(|(feature, ..)| format!("{feature} = []\n"))
        .collect::<String>();
    fs::write(
        path("Cargo.toml"),
        format!(
            r#"[package]
name = "typed-radio-fixture"
version = "0.0.0"
edition = "2024"
[dependencies]
oer-mmio = {{ path = {:?} }}
[features]
{features}
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
    let refused = REFUSED
        .iter()
        .map(|(feature, source, _)| {
            format!("#[cfg(feature = \"{feature}\")]\npub mod {feature} {{\nuse super::*;\n{source}\n}}\n")
        })
        .collect::<String>();
    fs::write(
        path("src/lib.rs"),
        format!("#![allow(dead_code, unused)]\n{LAYOUTS}\n{blocks}\n{TESTS}\n{refused}"),
    )
    .unwrap();
    let cargo = |arguments: &[&str]| {
        oer_os_toolchain::cargo_in(directory.path())
            .args(arguments)
            .env("CARGO_TARGET_DIR", path("target"))
            .output()
            .expect("pinned Cargo typed-radio fixture")
    };
    let output = cargo(&["test", "--offline", "--quiet"]);
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    for (feature, _, error) in REFUSED {
        let output = cargo(&[
            "build",
            "--offline",
            "--quiet",
            "--lib",
            "--features",
            feature,
        ]);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success() && stderr.contains(error),
            "{feature} must fail with {error}:\n{stderr}"
        );
    }
}
