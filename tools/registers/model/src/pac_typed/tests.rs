use crate::{
    FixedRegisterImage, OwnershipPartition, PacApiPack, RegisterSides, pac_api::tests::empty_pack,
};

/// One 32-bit register of `RADIO`, as SVD XML.
fn register(name: &str, offset: u32, access: &str, extra: &str, fields: &str) -> String {
    format!(
        "<register><name>{name}</name><addressOffset>0x{offset:x}</addressOffset>\
         <size>32</size><access>{access}</access>{extra}<fields>{fields}</fields></register>"
    )
}

fn field(name: &str, offset: u32, width: u32, extra: &str) -> String {
    format!(
        "<field><name>{name}</name><bitOffset>{offset}</bitOffset><bitWidth>{width}</bitWidth>{extra}</field>"
    )
}

fn svd(registers: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\
         <device schemaVersion=\"1.3\"><name>TEST</name><addressUnitBits>8</addressUnitBits>\
         <width>32</width><size>32</size><access>read-write</access><peripherals>\
         <peripheral><name>RADIO</name><baseAddress>0x60000000</baseAddress>\
         <registers>{registers}</registers></peripheral>\
         <peripheral><name>OTHER</name><baseAddress>0x60001000</baseAddress><registers>{}</registers></peripheral>\
         </peripherals></device>",
        register("WORD", 0, "read-write", "", &field("VALUE", 0, 32, ""))
    )
}

fn typed_pack() -> PacApiPack {
    let mut pack = empty_pack();
    pack.options.typed_peripherals = vec!["RADIO".to_owned()];
    pack
}

fn block() -> String {
    [
        register("COMMAND", 0, "write-only", "", &field("OPCODE", 0, 8, "")),
        register(
            "CONTROL",
            4,
            "read-write",
            "",
            &(field("ENABLE", 0, 1, "") + &field("LEVEL", 4, 3, "")),
        ),
        register(
            "STATUS",
            8,
            "read-write",
            "<modifiedWriteValues>oneToClear</modifiedWriteValues>",
            &(field("DONE", 0, 1, "") + &field("ERROR", 1, 1, "")),
        ),
    ]
    .concat()
}

#[test]
fn a_typed_block_renders_without_unsafe_or_raw_writes() {
    let rust = typed_pack()
        .render_typed_rust(&svd(&block()), &Default::default())
        .expect("the typed block renders");
    for unsafe_item in [
        "unsafe {",
        "unsafe fn",
        "unsafe impl",
        "unsafe trait",
        "allow(unsafe",
    ] {
        assert!(!rust.contains(unsafe_item), "{unsafe_item}\n{rust}");
    }
    for item in [
        "pub mod radio {",
        "pub fn write_command(",
        "pub fn modify_control(",
        "pub fn sample_status(",
        "pub fn acknowledge_status(",
        "pub fn set_level(&mut self, value: oer_mmio::Bits<3>)",
    ] {
        assert!(rust.contains(item), "{item}\n{rust}");
    }
    assert!(!rust.contains("pub fn modify_status("), "{rust}");
    assert!(!rust.contains("mod other"), "{rust}");
}

#[test]
fn a_register_mixing_write_one_to_clear_and_ordinary_fields_is_refused() {
    let mixed = register(
        "STATUS",
        0,
        "read-write",
        "",
        &(field(
            "DONE",
            0,
            1,
            "<modifiedWriteValues>oneToClear</modifiedWriteValues>",
        ) + &field("ENABLE", 1, 1, "")),
    );
    let error = typed_pack()
        .render_typed_rust(&svd(&mixed), &Default::default())
        .expect_err("the typed block is refused");
    assert!(
        error.to_string().contains("mixes write-one-to-clear"),
        "{error}"
    );
}

#[test]
fn a_transaction_the_typed_backend_does_not_render_is_refused() {
    let mut pack = typed_pack();
    pack.w1c_register_snapshots
        .push(crate::W1cRegisterSnapshot {
            name: "status".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "STATUS".to_owned(),
            sources: vec!["SOURCE".to_owned()],
        });
    let error = pack
        .render_typed_rust(&svd(&block()), &Default::default())
        .expect_err("the typed block is refused");
    assert!(
        error
            .to_string()
            .contains("samples and acknowledges itself"),
        "{error}"
    );
}

#[test]
fn a_fixed_image_becomes_a_method_with_its_mask_asserted() {
    let mut pack = typed_pack();
    pack.fixed_register_images.push(FixedRegisterImage {
        whole_word: false,
        name: "issue_start".to_owned(),
        peripheral: "RADIO".to_owned(),
        register: "COMMAND".to_owned(),
        value: 0x41,
        sources: vec!["SOURCE".to_owned()],
    });
    let rust = pack
        .render_typed_rust(&svd(&block()), &Default::default())
        .expect("the typed block renders");
    assert!(rust.contains("pub fn issue_start(&mut self"), "{rust}");
    assert!(
        rust.contains("oer_mmio::check::within(0x00000041, command::FIELDS)"),
        "{rust}"
    );
}

#[test]
fn a_split_observing_a_register_its_read_changes_is_refused() {
    let mut pack = typed_pack();
    let clearing = register(
        "COUNTER",
        0,
        "read-only",
        "<readAction>clear</readAction>",
        &field("COUNT", 0, 16, ""),
    ) + &register("CONTROL", 4, "read-write", "", &field("ENABLE", 0, 1, ""));
    pack.register_sides.push(RegisterSides {
        peripheral: "RADIO".to_owned(),
        irq: vec![],
        irq_observes: vec!["COUNTER".to_owned()],
        description: "test".to_owned(),
        sources: vec!["SOURCE".to_owned()],
    });
    let error = pack
        .render_typed_rust(&svd(&clearing), &Default::default())
        .expect_err("the typed block is refused");
    assert!(
        error.to_string().contains("no side-effect-free read"),
        "{error}"
    );
}

#[test]
fn the_svd2rust_policy_loses_everything_bound_to_a_typed_peripheral() {
    let mut pack = typed_pack();
    pack.ownership_partitions = vec![
        OwnershipPartition {
            name: "RadioPeripherals".to_owned(),
            member: "radio".to_owned(),
            description: "test".to_owned(),
            peripherals: vec!["RADIO".to_owned()],
            handoff: false,
        },
        OwnershipPartition {
            name: "OtherPeripherals".to_owned(),
            member: "other".to_owned(),
            description: "test".to_owned(),
            peripherals: vec!["OTHER".to_owned()],
            handoff: false,
        },
    ];
    pack.fixed_register_images.push(FixedRegisterImage {
        whole_word: false,
        name: "issue_start".to_owned(),
        peripheral: "RADIO".to_owned(),
        register: "COMMAND".to_owned(),
        value: 0x41,
        sources: vec!["SOURCE".to_owned()],
    });
    let legacy = pack.without_typed();
    assert!(legacy.fixed_register_images.is_empty());
    assert!(legacy.options.typed_peripherals.is_empty());
    assert_eq!(legacy.ownership_partitions.len(), 1);
    assert_eq!(legacy.ownership_partitions[0].peripherals, ["OTHER"]);
}

#[test]
fn a_field_reads_back_as_the_type_its_writer_takes() {
    let rust = typed_pack()
        .render_typed_rust(&svd(&block()), &Default::default())
        .expect("the typed block renders");
    assert!(
        rust.contains("pub const fn level(&self) -> oer_mmio::Bits<3>"),
        "{rust}"
    );
    assert!(rust.contains("oer_mmio::Bits::truncate("), "{rust}");
    assert!(
        rust.contains("pub const fn enable(&self) -> bool"),
        "{rust}"
    );
}

#[test]
fn a_write_only_clear_register_is_written_from_zero() {
    let clear = register(
        "CLEAR",
        0,
        "write-only",
        "<modifiedWriteValues>oneToClear</modifiedWriteValues>",
        &(field("DONE", 0, 1, "") + &field("ERROR", 1, 1, "")),
    );
    let rust = typed_pack()
        .render_typed_rust(&svd(&clear), &Default::default())
        .expect("the typed block renders");
    assert!(rust.contains("pub fn write_clear("), "{rust}");
    assert!(!rust.contains("pub fn sample_clear("), "{rust}");
}

#[test]
fn only_a_whole_word_vendor_image_skips_the_declared_field_assertion() {
    let mut pack = typed_pack();
    for (name, whole_word) in [("set_declared", false), ("set_vendor_word", true)] {
        pack.fixed_register_images.push(FixedRegisterImage {
            whole_word,
            name: name.to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "CONTROL".to_owned(),
            value: 0x0f0f_0f01,
            sources: vec!["SOURCE".to_owned()],
        });
    }
    let rust = pack
        .render_typed_rust(&svd(&block()), &Default::default())
        .expect("the typed block renders");
    assert!(
        rust.contains("\"set_declared writes declared fields only\""),
        "{rust}"
    );
    assert!(!rust.contains("set_vendor_word writes"), "{rust}");
    assert!(rust.contains("undeclared bits included"), "{rust}");
}

/// Builds `rendered` with the workspace's `oer-mmio` and `main` into a
/// program in `directory` (the pinned cargo, offline, its own target
/// directory), runs it and returns its standard output.
fn run_rendered(directory: &std::path::Path, rendered: &str, main: &str) -> String {
    let mmio = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../portable/mmio");
    std::fs::write(
        directory.join("Cargo.toml"),
        format!(
            "[package]\nname = \"rendered\"\nversion = \"0.0.0\"\nedition = \"2024\"\n\
             [dependencies]\noer-mmio = {{ path = {:?} }}\n[workspace]\n",
            mmio.display().to_string()
        ),
    )
    .expect("the fixture manifest is written");
    std::fs::create_dir(directory.join("src")).expect("the fixture source directory");
    std::fs::write(
        directory.join("src/main.rs"),
        format!("#![allow(dead_code, unused)]\n{rendered}\n{main}"),
    )
    .expect("the fixture program is written");
    let output = std::process::Command::new(env!("CARGO"))
        .args(["run", "--quiet", "--offline", "--manifest-path"])
        .arg(directory.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", directory.join("target"))
        .current_dir(directory)
        .output()
        .expect("the pinned cargo runs");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("the program prints text")
}

#[test]
fn a_fixed_sequence_writes_its_images_in_order() {
    let mut pack = typed_pack();
    let step = |register: &str, value| crate::FixedRegisterSequenceStep {
        register: register.to_owned(),
        index: None,
        value,
        whole_word: false,
    };
    // Two images of one word: the one written last is the one that stays.
    pack.fixed_register_sequences
        .push(crate::FixedRegisterSequence {
            name: "start_then_enable".to_owned(),
            peripheral: "RADIO".to_owned(),
            steps: vec![
                step("COMMAND", 0x41),
                step("CONTROL", 0x10),
                step("CONTROL", 0x1),
            ],
            sources: vec!["SOURCE".to_owned()],
        });
    let rendered = pack
        .render_typed_rust(&svd(&block()), &Default::default())
        .expect("the sequence renders");
    let directory = tempfile::tempdir().expect("a temporary directory");
    let output = run_rendered(
        directory.path(),
        &rendered,
        r#"
fn main() {
    let before = [0_u32; radio::SIZE / 4];
    let mut memory = before;
    let mut block = radio::Radio::host(&mut memory).expect("memory holds the block");
    block.start_then_enable();
    let control = block.read_control();
    let status = block.read_status();
    println!("enable={} level={}", control.enable(), control.level().get());
    println!("done={} error={}", status.done(), status.error());
    drop(block);
    let changed = before.iter().zip(&memory).filter(|(a, b)| a != b).count();
    println!("changed={changed}");
}
"#,
    );
    assert_eq!(
        output.lines().collect::<Vec<_>>(),
        ["enable=true level=0", "done=false error=false", "changed=2"],
        "the command and the control word, the control image written last"
    );
}

fn partitioned_pack() -> PacApiPack {
    let mut pack = typed_pack();
    pack.options.typed_peripherals.push("OTHER".to_owned());
    pack.ownership_partitions = vec![
        OwnershipPartition {
            name: "RadioPeripherals".to_owned(),
            member: "radio".to_owned(),
            description: "test".to_owned(),
            peripherals: vec!["RADIO".to_owned()],
            handoff: false,
        },
        OwnershipPartition {
            name: "OtherPeripherals".to_owned(),
            member: "other".to_owned(),
            description: "test".to_owned(),
            peripherals: vec!["OTHER".to_owned()],
            handoff: false,
        },
    ];
    pack
}

#[test]
fn the_root_claims_every_typed_block_at_once_and_checks_their_words() {
    let rust = partitioned_pack()
        .render_typed_rust(&svd(&block()), &Default::default())
        .expect("the typed block renders");
    for item in [
        "pub type Claims = (super::radio::Claim, super::other::Claim,);",
        "pub fn partition(claims: Claims) -> PeripheralPartitions",
        "oer_mmio::check::disjoint_blocks(",
        "const WORDS: &'static [usize] = &[0x0, 0x4, 0x8];",
    ] {
        assert!(rust.contains(item), "{item}\n{rust}");
    }
}

#[test]
fn a_partition_mixing_typed_and_untyped_peripherals_is_refused() {
    let mut pack = partitioned_pack();
    pack.options
        .typed_peripherals
        .retain(|name| name != "OTHER");
    pack.ownership_partitions[0]
        .peripherals
        .push("OTHER".to_owned());
    pack.ownership_partitions.truncate(1);
    let error = pack
        .render_typed_rust(&svd(&block()), &Default::default())
        .expect_err("the typed block is refused");
    assert!(
        error.to_string().contains("mixes typed peripherals"),
        "{error}"
    );
}

#[test]
fn an_unexposed_peripheral_must_exist() {
    let mut pack = typed_pack();
    pack.options.unexposed_peripherals = vec!["MISSING".to_owned()];
    let error = pack
        .render_typed_rust(&svd(&block()), &Default::default())
        .expect_err("the typed block is refused");
    assert!(error.to_string().contains("is not in the SVD"), "{error}");
}

#[test]
fn the_two_sides_are_owners_whose_words_cover_the_block() {
    let mut pack = typed_pack();
    pack.register_sides.push(RegisterSides {
        peripheral: "RADIO".to_owned(),
        irq: vec!["STATUS".to_owned()],
        irq_observes: vec!["CONTROL".to_owned()],
        description: "test".to_owned(),
        sources: vec!["SOURCE".to_owned()],
    });
    pack.interrupt_sources.push(crate::InterruptSource {
        peripheral: "RADIO".to_owned(),
        source: 12,
        sources: vec!["SOURCE".to_owned()],
    });
    let rust = pack
        .render_typed_rust(&svd(&block()), &Default::default())
        .expect("the typed block renders");
    for item in [
        "pub struct RadioIrq<",
        "pub struct RadioTask<",
        "pub fn split(self) -> (RadioTask<P>, RadioIrq<P>)",
        "oer_mmio::check::sides_cover(",
        "pub const SOURCE: u16 = 12;",
        // The handler's side names the source an interrupt table binds.
        "impl<P: oer_mmio::Placement> oer_mmio::InterruptSource for RadioIrq<P> {\nconst SOURCE: u16 = 12;",
    ] {
        assert!(rust.contains(item), "{item}\n{rust}");
    }
}

#[test]
fn an_undivided_block_with_an_interrupt_names_its_source_on_its_owner() {
    let mut pack = typed_pack();
    pack.interrupt_sources.push(crate::InterruptSource {
        peripheral: "RADIO".to_owned(),
        source: 7,
        sources: vec!["SOURCE".to_owned()],
    });
    let rust = pack
        .render_typed_rust(&svd(&block()), &Default::default())
        .expect("the typed block renders");
    assert!(
        rust.contains(
            "impl<P: oer_mmio::Placement> oer_mmio::InterruptSource for Radio<P> {\nconst SOURCE: u16 = 7;"
        ),
        "{rust}"
    );
}

fn trigger(register: &str, field: &str) -> crate::WriteTriggerField {
    crate::WriteTriggerField {
        peripheral: "RADIO".to_owned(),
        register: register.to_owned(),
        field: field.to_owned(),
        sources: vec!["SOURCE".to_owned()],
    }
}

fn fifo(register: &str) -> crate::FifoPort {
    crate::FifoPort {
        peripheral: "RADIO".to_owned(),
        register: register.to_owned(),
        sources: vec!["SOURCE".to_owned()],
    }
}

/// `CONFIG` with an ordinary field and a write-only one-bit `UPDATE`.
fn triggered() -> String {
    register(
        "CONFIG",
        0,
        "read-write",
        "",
        &(field("ENABLE", 0, 1, "") + &field("UPDATE", 31, 1, "<access>write-only</access>")),
    )
}

#[test]
fn an_undeclared_write_only_field_in_a_readable_register_is_still_refused() {
    let error = typed_pack()
        .render_typed_rust(&svd(&triggered()), &Default::default())
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("a read-modify-write would replay"),
        "{error}"
    );
}

#[test]
fn a_write_trigger_field_has_only_its_explicit_trigger() {
    let mut pack = typed_pack();
    pack.write_trigger_fields.push(trigger("CONFIG", "UPDATE"));
    let rust = pack
        .render_typed_rust(&svd(&triggered()), &Default::default())
        .unwrap();
    for item in [
        "pub fn trigger_config_update(",
        "pub const TRIGGERS: u32",
        "pub fn modify_config(",
        "& !config::TRIGGERS",
        "pub fn set_enable(",
    ] {
        assert!(rust.contains(item), "{item}\n{rust}");
    }
    assert!(!rust.contains("pub fn set_update("), "{rust}");
    assert!(!rust.contains("pub const fn update("), "{rust}");
}

#[test]
fn a_write_trigger_field_wider_than_one_bit_is_refused() {
    let wide = register(
        "CONFIG",
        0,
        "read-write",
        "",
        &(field("ENABLE", 0, 1, "") + &field("UPDATE", 30, 2, "<access>write-only</access>")),
    );
    let mut pack = typed_pack();
    pack.write_trigger_fields.push(trigger("CONFIG", "UPDATE"));
    let error = pack
        .render_typed_rust(&svd(&wide), &Default::default())
        .unwrap_err();
    assert!(error.to_string().contains("one writable bit"), "{error}");
}

#[test]
fn a_write_trigger_field_the_block_lacks_is_refused() {
    let mut pack = typed_pack();
    pack.write_trigger_fields.push(trigger("CONFIG", "UPDATE"));
    pack.write_trigger_fields.push(trigger("CONFIG", "MISSING"));
    let error = pack
        .render_typed_rust(&svd(&triggered()), &Default::default())
        .unwrap_err();
    assert!(error.to_string().contains("is not a field"), "{error}");
}

/// A FIFO port `DATA` and an ordinary `CONTROL`.
fn port() -> String {
    register("DATA", 0, "read-write", "", &field("BYTE", 0, 8, ""))
        + &register("CONTROL", 4, "read-write", "", &field("ENABLE", 0, 1, ""))
}

#[test]
fn a_fifo_port_is_only_popped_and_pushed() {
    let mut pack = typed_pack();
    pack.fifo_ports.push(fifo("DATA"));
    let rust = pack
        .render_typed_rust(&svd(&port()), &Default::default())
        .unwrap();
    assert!(rust.contains("pub fn pop_data(&mut self) -> u8"), "{rust}");
    assert!(
        rust.contains("pub fn push_data(&mut self, value: u8)"),
        "{rust}"
    );
    for absent in [
        "pub fn read_data(",
        "pub fn modify_data(",
        "pub fn write_data(",
    ] {
        assert!(!rust.contains(absent), "{absent}\n{rust}");
    }
}

#[test]
fn a_fifo_port_its_read_declares_changing_is_still_pushed() {
    let clearing = register(
        "DATA",
        0,
        "read-write",
        "<readAction>clear</readAction>",
        &field("BYTE", 0, 8, ""),
    );
    let error = typed_pack()
        .render_typed_rust(&svd(&clearing), &Default::default())
        .unwrap_err();
    assert!(error.to_string().contains("changes when read"), "{error}");
    let mut pack = typed_pack();
    pack.fifo_ports.push(fifo("DATA"));
    let rust = pack
        .render_typed_rust(&svd(&clearing), &Default::default())
        .unwrap();
    assert!(rust.contains("pub fn push_data("), "{rust}");
}

#[test]
fn a_fifo_port_is_never_observed_or_named_by_a_transaction() {
    let mut pack = typed_pack();
    pack.fifo_ports.push(fifo("DATA"));
    pack.register_sides.push(RegisterSides {
        peripheral: "RADIO".to_owned(),
        irq: vec![],
        irq_observes: vec!["DATA".to_owned()],
        description: "test".to_owned(),
        sources: vec!["SOURCE".to_owned()],
    });
    let error = pack
        .render_typed_rust(&svd(&port()), &Default::default())
        .unwrap_err();
    assert!(
        error.to_string().contains("no side-effect-free read"),
        "{error}"
    );
    let mut pack = typed_pack();
    pack.fifo_ports.push(fifo("DATA"));
    pack.fixed_register_images.push(FixedRegisterImage {
        whole_word: false,
        name: "push_fixed".to_owned(),
        peripheral: "RADIO".to_owned(),
        register: "DATA".to_owned(),
        value: 0x41,
        sources: vec!["SOURCE".to_owned()],
    });
    let error = pack
        .render_typed_rust(&svd(&port()), &Default::default())
        .unwrap_err();
    assert!(
        error.to_string().contains("only popped and pushed"),
        "{error}"
    );
}

#[test]
fn a_register_narrower_than_a_word_is_refused() {
    let byte = "<register><name>CTL</name><addressOffset>0x3</addressOffset>\
                <size>8</size><access>read-write</access><fields>"
        .to_owned()
        + &field("LEVEL", 0, 8, "")
        + "</fields></register>";
    let error = typed_pack()
        .render_typed_rust(&svd(&byte), &Default::default())
        .unwrap_err();
    assert!(error.to_string().contains("not 32 bits wide"), "{error}");
}

#[test]
fn trigger_and_fifo_declarations_need_a_typed_peripheral() {
    let mut pack = typed_pack();
    pack.fifo_ports.push(crate::FifoPort {
        peripheral: "OTHER".to_owned(),
        ..fifo("WORD")
    });
    let error = pack
        .render_typed_rust(&svd(&block()), &Default::default())
        .unwrap_err();
    assert!(error.to_string().contains("is not typed"), "{error}");
    let mut pack = typed_pack();
    pack.write_trigger_fields.push(trigger("CONFIG", "UPDATE"));
    pack.fifo_ports.push(fifo("DATA"));
    let legacy = pack.without_typed();
    assert!(legacy.write_trigger_fields.is_empty() && legacy.fifo_ports.is_empty());
}

/// A register library's API: one fixed image on `RADIO.CONTROL`.
fn library_pack() -> PacApiPack {
    let mut pack = empty_pack();
    pack.fixed_register_images.push(FixedRegisterImage {
        whole_word: false,
        name: "enable_level_two".to_owned(),
        peripheral: "RADIO".to_owned(),
        register: "CONTROL".to_owned(),
        value: 0x21,
        sources: vec!["SOURCE".to_owned()],
    });
    pack
}

/// `RADIO` placed from the library whose module is `crate::library`.
fn placed_radio() -> std::collections::BTreeMap<String, crate::SharedPeripheral> {
    std::collections::BTreeMap::from([(
        "RADIO".to_owned(),
        crate::SharedPeripheral {
            library: "fixture".to_owned(),
            crate_name: "crate::library".to_owned(),
        },
    )])
}

#[test]
fn a_library_block_has_no_address_and_a_chip_places_it() {
    let library = library_pack()
        .render_library_typed_rust(&svd(&block()))
        .expect("the library renders");
    for item in [
        "pub mod radio {",
        "pub mod other {",
        "pub struct Radio<P: oer_mmio::Placement>(",
        "impl<const BASE: usize> Radio<oer_mmio::Fixed<BASE>> {",
        "pub fn enable_level_two(",
    ] {
        assert!(library.contains(item), "{item}\n{library}");
    }
    for chip_only in ["pub const BASE", "pub type Claim", "0x60000000", "Claims"] {
        assert!(!library.contains(chip_only), "{chip_only}\n{library}");
    }
    let chip = partitioned_pack()
        .render_typed_rust(&svd(&block()), &placed_radio())
        .expect("the chip places the library block");
    for item in [
        "pub use crate::library::radio::*;",
        "pub const BASE: usize = 0x60000000;",
        "pub type Radio<P = oer_mmio::Fixed<0x60000000>> = crate::library::radio::Radio<P>;",
        "pub type Claims = (super::radio::Claim, super::other::Claim,);",
    ] {
        assert!(chip.contains(item), "{item}\n{chip}");
    }
    assert!(!chip.contains("pub fn modify_control("), "{chip}");
}

#[test]
fn a_placed_library_block_runs_through_the_chip_names() {
    let library = library_pack()
        .render_library_typed_rust(&svd(&block()))
        .expect("the library renders");
    let chip = partitioned_pack()
        .render_typed_rust(&svd(&block()), &placed_radio())
        .expect("the chip places the library block");
    let directory = tempfile::tempdir().expect("a temporary directory");
    let output = run_rendered(
        directory.path(),
        &format!("mod library {{\n{library}\n}}\n{chip}"),
        r#"
/// The chip's owner of its claimed block is the library's at the chip's base.
fn owner(claim: radio::Claim) -> radio::Radio {
    radio::Radio::new(claim)
}

fn main() {
    let mut memory = [0_u32; radio::SIZE / 4];
    let mut block = radio::Radio::host(&mut memory).expect("memory holds the block");
    block.enable_level_two();
    let control = block.read_control();
    println!("enable={} level={}", control.enable(), control.level().get());
    drop(block);
    println!("control={:#x}", memory[1]);
}
"#,
    );
    assert_eq!(
        output.lines().collect::<Vec<_>>(),
        ["enable=true level=2", "control=0x21"]
    );
}

#[test]
fn a_chip_types_every_library_block_and_changes_none() {
    let error = typed_pack()
        .render_typed_rust(
            &svd(&block()),
            &std::collections::BTreeMap::from([(
                "OTHER".to_owned(),
                placed_radio()["RADIO"].clone(),
            )]),
        )
        .expect_err("an untyped library block is refused");
    assert!(
        error.to_string().contains("list it in typed-peripherals"),
        "{error}"
    );
    let mut pack = typed_pack();
    pack.fifo_ports.push(fifo("STATUS"));
    let error = pack
        .render_typed_rust(&svd(&block()), &placed_radio())
        .expect_err("a chip FIFO port on a library block is refused");
    assert!(
        error
            .to_string()
            .contains("declare it in the library's API"),
        "{error}"
    );
}

#[test]
fn a_library_declares_nothing_a_chip_owns() {
    let mut pack = library_pack();
    pack.interrupt_sources.push(crate::InterruptSource {
        peripheral: "RADIO".to_owned(),
        source: 12,
        sources: vec!["SOURCE".to_owned()],
    });
    let error = pack
        .render_library_typed_rust(&svd(&block()))
        .expect_err("a library interrupt source is refused");
    assert!(error.to_string().contains("belong to a chip"), "{error}");
    let mut pack = library_pack();
    pack.options.typed_peripherals = vec!["RADIO".to_owned()];
    let error = pack
        .render_library_typed_rust(&svd(&block()))
        .expect_err("a library typed list is refused");
    assert!(error.to_string().contains("types every layout"), "{error}");
}

#[test]
fn a_handoff_partition_has_its_own_claims_and_the_root_does_not_claim_it() {
    let mut pack = partitioned_pack();
    pack.ownership_partitions[1].handoff = true;
    let rust = pack
        .render_typed_rust(&svd(&block()), &Default::default())
        .expect("the handoff pack renders");
    for item in [
        "pub type Claims = super::radio::Claim;",
        "pub type OtherPeripheralsClaims = super::other::Claim;",
        "pub fn from_claims(claims: OtherPeripheralsClaims) -> Self",
        "oer_mmio::check::disjoint_blocks(",
    ] {
        assert!(rust.contains(item), "{item}\n{rust}");
    }
    assert!(!rust.contains("pub other: OtherPeripherals"), "{rust}");
    pack.ownership_partitions[0].handoff = true;
    let rust = pack
        .render_typed_rust(&svd(&block()), &Default::default())
        .expect("the handoff pack renders");
    assert!(!rust.contains("pub type Claims"), "{rust}");
    assert!(rust.contains("pub type RadioPeripheralsClaims"), "{rust}");
}

fn refused(pack: &PacApiPack, registers: &str, expected: &str) {
    let error = pack
        .render_typed_rust(&svd(registers), &Default::default())
        .expect_err("the typed block is refused");
    assert!(error.to_string().contains(expected), "{error}");
}

/// `HEAD` with a 20-bit pointer field beside an ordinary one.
fn pointed() -> String {
    register(
        "HEAD",
        0,
        "read-write",
        "",
        &(field("ADDRESS_LOW", 0, 20, "") + &field("FORMAT", 20, 4, "")),
    )
}

fn pointer_pack(encoding: Option<crate::RegisterPointerEncoding>) -> PacApiPack {
    let mut pack = typed_pack();
    pack.pointer_fields.push(crate::PointerField {
        name: "head_pointer".to_owned(),
        peripheral: "RADIO".to_owned(),
        register: "HEAD".to_owned(),
        field: "ADDRESS_LOW".to_owned(),
        points_to: "DESCRIPTOR".to_owned(),
        member_offset: 0,
        encoding,
        sources: vec!["SOURCE".to_owned()],
    });
    pack
}

#[test]
fn a_pointer_field_has_an_image_setter_and_no_integer_one() {
    let rust = pointer_pack(Some(crate::RegisterPointerEncoding::LowBits))
        .render_typed_rust(&svd(&pointed()), &Default::default())
        .expect("the pointer field renders");
    assert!(rust.contains("pub fn set_address_low(&mut self, value: AddressLowImage)"));
    assert!(
        !rust.contains("value: oer_mmio::Bits<20>) -> &mut Self"),
        "{rust}"
    );
    assert!(rust.contains("every address of the DMA window shares the high bits"));
}

#[test]
fn a_pointer_field_of_the_wrong_width_is_refused() {
    refused(
        &pointer_pack(None),
        &pointed(),
        "must be a writable field of its encoding's width",
    );
}

#[test]
fn only_a_typed_reference_changes_a_pointer_field() {
    let mut pack = pointer_pack(Some(crate::RegisterPointerEncoding::LowBits));
    pack.fixed_register_images.push(FixedRegisterImage {
        whole_word: false,
        name: "head_image".to_owned(),
        peripheral: "RADIO".to_owned(),
        register: "HEAD".to_owned(),
        value: 0x10,
        sources: vec!["SOURCE".to_owned()],
    });
    refused(&pack, &pointed(), "sets a DMA address field");

    let mut pack = pointer_pack(Some(crate::RegisterPointerEncoding::LowBits));
    pack.field_clear_modifies.push(crate::FieldOrModify {
        name: "clear_head".to_owned(),
        peripheral: "RADIO".to_owned(),
        register: "HEAD".to_owned(),
        fields: vec![crate::pac_api::FieldOrProjection {
            field: "ADDRESS_LOW".to_owned(),
            source_bit_offset: 0,
        }],
        domain: None,
        value: Some(0xf_ffff),
        exposure: crate::PacApiExposure::Facade,
        sources: vec!["SOURCE".to_owned()],
    });
    refused(&pack, &pointed(), "only a typed DMA reference or zero sets");
}

/// `STATUS` (read-only) and its write-only, write-one-to-clear `CLEAR`.
fn events(status_fields: &str) -> String {
    register("STATUS", 0, "read-only", "", status_fields)
        + &register(
            "CLEAR",
            4,
            "write-only",
            "<modifiedWriteValues>oneToClear</modifiedWriteValues>",
            &(field("DONE", 0, 1, "") + &field("ERROR", 1, 1, "")),
        )
}

fn snapshot_pack() -> PacApiPack {
    let mut pack = typed_pack();
    pack.interrupt_snapshots.push(crate::InterruptSnapshot {
        name: "events".to_owned(),
        peripheral: "RADIO".to_owned(),
        status_register: "STATUS".to_owned(),
        clear_register: "CLEAR".to_owned(),
        sources: vec!["SOURCE".to_owned()],
    });
    pack
}

#[test]
fn a_snapshot_acknowledges_through_its_clear_register_alone() {
    let rust = snapshot_pack()
        .render_typed_rust(
            &svd(&events(
                &(field("DONE", 0, 1, "") + &field("LEVEL", 4, 4, "")),
            )),
            &Default::default(),
        )
        .expect("the snapshot renders");
    assert!(rust.contains("pub fn acknowledge_events(&mut self, snapshot: EventsSnapshot)"));
    assert!(
        rust.contains("pub const EVENTS: u32 = 0x00000001;"),
        "{rust}"
    );
    assert!(!rust.contains("pub fn write_clear("), "{rust}");

    let mut pack = snapshot_pack();
    pack.fixed_register_images.push(FixedRegisterImage {
        whole_word: false,
        name: "clear_everything".to_owned(),
        peripheral: "RADIO".to_owned(),
        register: "CLEAR".to_owned(),
        value: 0x3,
        sources: vec!["SOURCE".to_owned()],
    });
    refused(
        &pack,
        &events(&field("DONE", 0, 1, "")),
        "only its snapshot's acknowledgement writes",
    );
}

#[test]
fn a_status_field_acknowledged_in_part_is_refused() {
    refused(
        &snapshot_pack(),
        &events(&field("STATE", 1, 2, "")),
        "acknowledged only in part",
    );
}

fn capability_pack(registers: &[&str]) -> PacApiPack {
    let mut pack = typed_pack();
    pack.read_capabilities.push(crate::ReadCapability {
        peripheral: "RADIO".to_owned(),
        name: "Time".to_owned(),
        registers: registers.iter().map(|name| (*name).to_owned()).collect(),
        copy: true,
        description: "The time.".to_owned(),
        sources: vec!["SOURCE".to_owned()],
    });
    pack
}

#[test]
fn a_read_capability_of_the_whole_block_consumes_its_owner() {
    let time = register("TIME", 0, "read-only", "", &field("VALUE", 0, 32, ""));
    let rust = capability_pack(&["TIME"])
        .render_typed_rust(&svd(&time), &Default::default())
        .expect("the capability renders");
    assert!(
        rust.contains("pub fn into_time(self) -> RadioTime<P>"),
        "{rust}"
    );
    assert!(rust.contains("impl<P: oer_mmio::Placement> Copy for RadioTime<P> {}"));
}

#[test]
fn a_read_capability_reads_without_side_effects_and_excludes_register_sides() {
    let changing = register(
        "TIME",
        0,
        "read-only",
        "<readAction>clear</readAction>",
        &field("VALUE", 0, 32, ""),
    );
    refused(
        &capability_pack(&["TIME"]),
        &changing,
        "reads it with a side effect",
    );
    let mut pack = capability_pack(&["CONTROL"]);
    pack.register_sides.push(RegisterSides {
        peripheral: "RADIO".to_owned(),
        irq: vec!["STATUS".to_owned()],
        irq_observes: Vec::new(),
        description: "sides".to_owned(),
        sources: vec!["SOURCE".to_owned()],
    });
    refused(&pack, &block(), "at most one of register sides");
}

/// `ENABLE_SET`, a write-only trigger word, and `CH%s`, four channel words.
fn channels() -> String {
    register(
        "ENABLE_SET",
        0,
        "write-only",
        "",
        &(field("CH0", 0, 1, "") + &field("CH1", 1, 1, "") + &field("CH2", 2, 1, "")),
    ) + "<register><dim>4</dim><dimIncrement>0x4</dimIncrement><name>CH%s</name>\
         <addressOffset>0x10</addressOffset><size>32</size><access>read-write</access>\
         <fields><field><name>ID</name><bitOffset>0</bitOffset><bitWidth>8</bitWidth></field>\
         </fields></register>"
}

fn division_pack(low: (&[&str], u32), high: (&[&str], u32)) -> PacApiPack {
    let mut pack = typed_pack();
    pack.write_trigger_fields = ["CH0", "CH1"]
        .iter()
        .map(|name| trigger("ENABLE_SET", name))
        .collect();
    let part = |name: &str, (fields, first): (&[&str], u32)| crate::DivisionPart {
        name: name.to_owned(),
        registers: Vec::new(),
        elements: vec![crate::DivisionElements {
            register: "CH%s".to_owned(),
            first,
            count: 2,
        }],
        observes: Vec::new(),
        triggers: vec![crate::DivisionTrigger {
            register: "ENABLE_SET".to_owned(),
            fields: fields.iter().map(|name| (*name).to_owned()).collect(),
        }],
    };
    pack.register_divisions.push(crate::RegisterDivision {
        peripheral: "RADIO".to_owned(),
        description: "Two protocols.".to_owned(),
        sources: vec!["SOURCE".to_owned()],
        parts: vec![part("Low", low), part("High", high)],
    });
    pack
}

#[test]
fn a_division_gives_each_part_its_elements_and_trigger_bits() {
    let rust = division_pack((&["CH0"], 0), (&["CH1"], 2))
        .render_typed_rust(&svd(&channels()), &Default::default())
        .expect("the division renders");
    for item in [
        "pub fn divide(self) -> (RadioLow<P>, RadioHigh<P>)",
        "const TRIGGERS: &'static [(usize, u32)] = &[(0x0, 0x00000002)];",
        "self.0.write_trigger::<0x0>(0x00000001);",
        "read_indexed::<0x18, 0x4, 2>(index)",
    ] {
        assert!(rust.contains(item), "{item}\n{rust}");
    }
}

#[test]
fn a_division_refuses_shared_elements_bits_and_undeclared_triggers() {
    refused(
        &division_pack((&["CH0"], 0), (&["CH1"], 1)),
        &channels(),
        "element 1 of CH%s has another owner",
    );
    refused(
        &division_pack((&["CH0"], 0), (&["CH0"], 2)),
        &channels(),
        "are none or another part's",
    );
    refused(
        &division_pack((&["CH0"], 0), (&["CH2"], 2)),
        &channels(),
        "is no declared write-trigger field",
    );
}

#[test]
fn a_range_of_zero_alone_is_a_constant_zero_field() {
    let keys = register(
        "KEYS",
        0,
        "read-write",
        "",
        &(field("VALID", 0, 25, "")
            + &field(
                "RESERVED_HIGH",
                25,
                7,
                "<writeConstraint><range><minimum>0</minimum><maximum>0</maximum></range></writeConstraint>",
            )),
    );
    let rust = typed_pack()
        .render_typed_rust(&svd(&keys), &Default::default())
        .expect("the zero field renders");
    assert!(rust.contains("pub const ZERO: u32 = 0xfe000000;"), "{rust}");
    assert!(rust.contains("& !keys::ZERO"), "{rust}");
    assert!(!rust.contains("set_reserved_high"), "{rust}");
    let narrow = register(
        "KEYS",
        0,
        "read-write",
        "",
        &field(
            "VALID",
            0,
            8,
            "<writeConstraint><range><minimum>0</minimum><maximum>9</maximum></range></writeConstraint>",
        ),
    );
    refused(&typed_pack(), &narrow, "renders no bounded field yet");
}

#[test]
fn a_partition_image_refuses_a_register_its_read_changes() {
    let mut pack = partitioned_pack();
    pack.partition_image_reads.push(crate::PartitionImageRead {
        name: "image".to_owned(),
        partition: pack.ownership_partitions[0].name.clone(),
        exposure: crate::PacApiExposure::Facade,
        sources: vec!["SOURCE".to_owned()],
    });
    let changing = block()
        + &register(
            "LATCH",
            0xc,
            "read-only",
            "<readAction>clear</readAction>",
            &field("VALUE", 0, 32, ""),
        );
    refused(&pack, &changing, "changes when read");
    assert!(
        pack.without_typed_operations()
            .partition_image_reads
            .is_empty(),
        "the svd2rust backend reads no typed partition"
    );
}

#[test]
fn a_library_declares_no_read_capability() {
    let mut pack = library_pack();
    pack.read_capabilities = capability_pack(&["CONTROL"]).read_capabilities;
    let error = pack
        .render_library_typed_rust(&svd(&block()))
        .expect_err("the library is refused");
    assert!(error.to_string().contains("read capability"), "{error}");
}

#[test]
fn a_fifo_port_takes_no_pointer_field() {
    let mut pack = typed_pack();
    pack.pointer_fields.push(crate::PointerField {
        name: "base_pointer".to_owned(),
        peripheral: "RADIO".to_owned(),
        register: "BASE".to_owned(),
        field: "ADDRESS".to_owned(),
        points_to: "DESCRIPTOR".to_owned(),
        member_offset: 0,
        encoding: None,
        sources: vec!["SOURCE".to_owned()],
    });
    pack.fifo_ports.push(fifo("BASE"));
    refused(
        &pack,
        &register("BASE", 0, "read-write", "", &field("ADDRESS", 0, 32, "")),
        "must be one register with one field that is no pointer field",
    );
}

#[test]
fn an_acknowledged_clear_register_takes_no_write_trigger() {
    let mut pack = snapshot_pack();
    pack.write_trigger_fields.push(trigger("CLEAR", "DONE"));
    refused(
        &pack,
        &events(&field("DONE", 0, 1, "")),
        "writes a register only its snapshot's acknowledgement writes",
    );
}

fn shared() -> crate::SharedBlock {
    crate::SharedBlock {
        peripheral: "RADIO".to_owned(),
        description: "A counter every context reads.".to_owned(),
        sources: vec!["SOURCE".to_owned()],
    }
}

/// A counter: `OP` (a read-only `VALID` and a write-only trigger `UPDATE`),
/// a read-only `VALUE` and a one-field `RAISE`.
fn counter() -> String {
    register(
        "OP",
        0,
        "read-write",
        "",
        &(field("VALID", 29, 1, "<access>read-only</access>")
            + &field("UPDATE", 30, 1, "<access>write-only</access>")),
    ) + &register("VALUE", 4, "read-only", "", &field("LOW", 0, 32, ""))
        + &register("RAISE", 8, "read-write", "", &field("LEVEL", 0, 1, ""))
}

#[test]
fn a_shared_block_strobes_stores_and_reads_through_a_shared_borrow() {
    let mut pack = typed_pack();
    pack.write_trigger_fields.push(trigger("OP", "UPDATE"));
    pack.shared_blocks.push(shared());
    let rust = pack
        .render_typed_rust(&svd(&counter()), &Default::default())
        .unwrap();
    for item in [
        "impl oer_mmio::SharedRegisters for BlockWords {}",
        "pub fn trigger_op_update(&self) {\nself.0.write_shared::<0x0>(0x40000000);",
        "pub fn store_raise(&self, value: bool) {\nself.0.write_shared::<0x8>(u32::from(value));",
        "pub fn read_op(&self",
        "pub fn read_value(&self",
    ] {
        assert!(rust.contains(item), "{item}\n{rust}");
    }
    for absent in [
        "&mut self",
        "pub fn modify_",
        "pub fn write_",
        "pub struct W(",
    ] {
        assert!(!rust.contains(absent), "{absent}\n{rust}");
    }
}

#[test]
fn a_shared_block_refuses_every_other_kind_of_register() {
    let refused = [
        // An ordinary field a store would have to read back.
        register(
            "CONTROL",
            0,
            "read-write",
            "",
            &(field("ENABLE", 0, 1, "") + &field("LEVEL", 4, 3, "")),
        ),
        // A write-one-to-clear status.
        register(
            "STATUS",
            0,
            "read-write",
            "<modifiedWriteValues>oneToClear</modifiedWriteValues>",
            &field("DONE", 0, 1, ""),
        ),
        // A read that changes the register.
        register(
            "DATA",
            0,
            "read-only",
            "<readAction>clear</readAction>",
            &field("BYTE", 0, 8, ""),
        ),
    ];
    for register in refused {
        let mut pack = typed_pack();
        pack.shared_blocks.push(shared());
        let error = pack
            .render_typed_rust(&svd(&register), &Default::default())
            .unwrap_err();
        assert!(
            error.to_string().contains("shared block register"),
            "{error}"
        );
    }
    let mut pack = typed_pack();
    pack.shared_blocks.push(shared());
    pack.register_sides.push(RegisterSides {
        peripheral: "RADIO".to_owned(),
        irq: vec![],
        irq_observes: vec!["VALUE".to_owned()],
        description: "test".to_owned(),
        sources: vec!["SOURCE".to_owned()],
    });
    pack.write_trigger_fields.push(trigger("OP", "UPDATE"));
    let error = pack
        .render_typed_rust(&svd(&counter()), &Default::default())
        .unwrap_err();
    assert!(error.to_string().contains("no register sides"), "{error}");
}
