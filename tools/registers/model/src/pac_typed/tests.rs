use crate::{
    FixedRegisterImage, OwnershipPartition, PacApiPack, RegisterSplit, pac_api::tests::empty_pack,
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
    pack.register_splits.push(RegisterSplit {
        peripheral: "RADIO".to_owned(),
        part: "Interrupt".to_owned(),
        rest: "Task".to_owned(),
        registers: vec![],
        observes: vec!["COUNTER".to_owned()],
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
