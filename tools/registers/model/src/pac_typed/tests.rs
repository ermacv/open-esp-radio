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
        },
        OwnershipPartition {
            name: "OtherPeripherals".to_owned(),
            member: "other".to_owned(),
            description: "test".to_owned(),
            peripherals: vec!["OTHER".to_owned()],
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
        },
        OwnershipPartition {
            name: "OtherPeripherals".to_owned(),
            member: "other".to_owned(),
            description: "test".to_owned(),
            peripherals: vec!["OTHER".to_owned()],
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
    ] {
        assert!(rust.contains(item), "{item}\n{rust}");
    }
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
