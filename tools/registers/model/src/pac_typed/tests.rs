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
    let rust = typed_pack().render_typed_rust(&svd(&block())).unwrap();
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
    let error = typed_pack().render_typed_rust(&svd(&mixed)).unwrap_err();
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
    let error = pack.render_typed_rust(&svd(&block())).unwrap_err();
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
    let rust = pack.render_typed_rust(&svd(&block())).unwrap();
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
    let error = pack.render_typed_rust(&svd(&clearing)).unwrap_err();
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
    let rust = typed_pack().render_typed_rust(&svd(&block())).unwrap();
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
    let rust = typed_pack().render_typed_rust(&svd(&clear)).unwrap();
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
    let rust = pack.render_typed_rust(&svd(&block())).unwrap();
    assert!(
        rust.contains("\"set_declared writes declared fields only\""),
        "{rust}"
    );
    assert!(!rust.contains("set_vendor_word writes"), "{rust}");
    assert!(rust.contains("undeclared bits included"), "{rust}");
}

#[test]
fn a_fixed_sequence_writes_its_images_in_order() {
    let mut pack = typed_pack();
    pack.fixed_register_sequences
        .push(crate::FixedRegisterSequence {
            name: "start_then_enable".to_owned(),
            peripheral: "RADIO".to_owned(),
            steps: vec![
                crate::FixedRegisterSequenceStep {
                    register: "COMMAND".to_owned(),
                    index: None,
                    value: 0x41,
                },
                crate::FixedRegisterSequenceStep {
                    register: "CONTROL".to_owned(),
                    index: None,
                    value: 0x1,
                },
            ],
            sources: vec!["SOURCE".to_owned()],
        });
    let rust = pack.render_typed_rust(&svd(&block())).unwrap();
    let start = rust
        .find("pub fn start_then_enable(&mut self)")
        .expect("rendered");
    let body = &rust[start..];
    let command = body
        .find("write::<0x0>(0x00000041)")
        .expect("command write");
    let control = body
        .find("write::<0x4>(0x00000001)")
        .expect("control write");
    assert!(command < control, "{body}");
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
        .render_typed_rust(&svd(&block()))
        .unwrap();
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
    let error = pack.render_typed_rust(&svd(&block())).unwrap_err();
    assert!(
        error.to_string().contains("mixes typed peripherals"),
        "{error}"
    );
}

#[test]
fn an_unexposed_peripheral_must_exist() {
    let mut pack = typed_pack();
    pack.options.unexposed_peripherals = vec!["MISSING".to_owned()];
    let error = pack.render_typed_rust(&svd(&block())).unwrap_err();
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
    let rust = pack.render_typed_rust(&svd(&block())).unwrap();
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
