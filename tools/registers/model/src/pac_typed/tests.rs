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
        masks: None,
        consumed_event: None,
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
        masks: None,
        consumed_event: None,
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
fn a_write_only_one_to_clear_field_outside_a_snapshot_is_a_write_trigger() {
    let clear = register(
        "CLEAR",
        0,
        "write-only",
        "<modifiedWriteValues>oneToClear</modifiedWriteValues>",
        &(field("DONE", 0, 1, "") + &field("ERRORS", 1, 4, "")),
    );
    let rust = typed_pack()
        .render_typed_rust(&svd(&clear), &Default::default())
        .expect("the typed block renders");
    for item in [
        "pub fn trigger_clear_done(",
        "pub fn trigger_clear_errors(&mut self, value: oer_mmio::Bits<4>)",
    ] {
        assert!(rust.contains(item), "{item}\n{rust}");
    }
    for item in [
        "pub fn write_clear(",
        "pub fn sample_clear(",
        "pub fn set_done(",
    ] {
        assert!(!rust.contains(item), "{item}\n{rust}");
    }
}

/// `registers` in a device that states no default access.
fn bare_svd(registers: &str) -> String {
    svd(registers).replacen(
        "<access>read-write</access><peripherals>",
        "<peripherals>",
        1,
    )
}

/// One register of each field class the typed backend derives from the
/// SVD alone, and the accessors each class gives.
#[test]
fn every_field_class_renders_exactly_its_accessors() {
    let unstated = format!(
        "<register><name>UNSTATED</name><addressOffset>0x18</addressOffset><size>32</size>\
         <fields>{}</fields></register>",
        field("VALUE", 0, 8, "")
    );
    let block = [
        register("PLAIN", 0, "read-write", "", &field("VALUE", 0, 8, "")),
        register("OBSERVED", 4, "read-only", "", &field("VALUE", 0, 8, "")),
        register("STORED", 8, "write-only", "", &field("VALUE", 0, 8, "")),
        register(
            "STROBE",
            0xc,
            "write-only",
            "<modifiedWriteValues>oneToClear</modifiedWriteValues>",
            &field("VALUE", 0, 1, ""),
        ),
        register(
            "EVENTS",
            0x10,
            "read-write",
            "<modifiedWriteValues>oneToClear</modifiedWriteValues>",
            &field("VALUE", 0, 1, ""),
        ),
        register(
            "LATCH",
            0x14,
            "read-only",
            "<readAction>clear</readAction>",
            &field("VALUE", 0, 8, ""),
        ),
        unstated,
    ]
    .concat();
    let rust = typed_pack()
        .render_typed_rust(&bare_svd(&block), &Default::default())
        .expect("the typed block renders");
    let present = [
        // plain: read, read-modify-write.
        "pub fn read_plain(",
        "pub fn modify_plain(",
        // read-only: read.
        "pub fn read_observed(",
        // write-only: written from zero.
        "pub fn write_stored(",
        // write-trigger: its trigger.
        "pub fn trigger_strobe_value(",
        // write-one-to-clear: sampled and acknowledged.
        "pub fn sample_events(",
        "pub fn acknowledge_events(",
        // no stated access: read.
        "pub fn read_unstated(",
    ];
    let absent = [
        "pub fn modify_observed(",
        "pub fn write_observed(",
        "pub fn read_stored(",
        "pub fn modify_stored(",
        "pub fn write_strobe(",
        "pub fn read_strobe(",
        "pub fn modify_events(",
        "pub fn write_events(",
        // read-clears: nothing.
        "pub fn read_latch(",
        "pub fn modify_latch(",
        "pub fn write_latch(",
        "pub fn modify_unstated(",
        "pub fn write_unstated(",
    ];
    for item in present {
        assert!(rust.contains(item), "{item}\n{rust}");
    }
    for item in absent {
        assert!(!rust.contains(item), "{item}\n{rust}");
    }
    assert!(
        rust.contains("0x0, 0x4, 0x8, 0xc, 0x10, 0x14, 0x18"),
        "every register stays a word of the block: {rust}"
    );
}

#[test]
fn a_read_only_one_to_clear_field_is_only_read() {
    // A field a write would acknowledge, read only by its access (the `soc`
    // pass's debug-only field): no sample and no acknowledgement.
    let status = register(
        "STATUS",
        0,
        "read-write",
        "",
        &field(
            "DONE",
            0,
            1,
            "<access>read-only</access><modifiedWriteValues>oneToClear</modifiedWriteValues>",
        ),
    );
    let rust = typed_pack()
        .render_typed_rust(&svd(&status), &Default::default())
        .expect("the typed block renders");
    assert!(rust.contains("pub fn read_status("), "{rust}");
    for item in [
        "pub fn sample_status(",
        "pub fn acknowledge_status(",
        "pub fn modify_status(",
    ] {
        assert!(!rust.contains(item), "{item}\n{rust}");
    }
}

#[test]
fn narrowing_a_w1c_lane_to_read_only_still_refuses_a_mixed_rmw() {
    for inherited in [false, true] {
        let modified = "<modifiedWriteValues>oneToClear</modifiedWriteValues>";
        let mixed = register(
            "CONTROL",
            0,
            "read-write",
            if inherited { modified } else { "" },
            &(field(
                "EVENT",
                0,
                1,
                &format!(
                    "<access>read-only</access>{}",
                    if inherited { "" } else { modified }
                ),
            ) + &field(
                "ENABLE",
                1,
                1,
                "<modifiedWriteValues>modify</modifiedWriteValues>",
            )),
        );
        refused(
            &typed_pack(),
            &mixed,
            "mixes write-one-to-clear and other fields",
        );
    }
}

#[test]
fn a_field_with_another_modified_write_value_is_refused() {
    let cleared = register(
        "CONTROL",
        0,
        "read-write",
        "",
        &field(
            "RELOAD",
            0,
            1,
            "<modifiedWriteValues>clear</modifiedWriteValues>",
        ),
    );
    refused(&typed_pack(), &cleared, "unsupported modified write values");
}

/// `ADDRESS`, one 32-bit DMA address word, and its policy write.
fn addressed(points_to: Option<&str>) -> (PacApiPack, String) {
    let mut pack = typed_pack();
    pack.full_register_writes.push(crate::FullRegisterWrite {
        name: "publish_address".to_owned(),
        peripheral: "RADIO".to_owned(),
        register: "ADDRESS".to_owned(),
        field: "ADDRESS".to_owned(),
        domain: None,
        points_to: points_to.map(str::to_owned),
        placement: points_to.is_none().then(|| "dma-window".to_owned()),
        exposure: crate::PacApiExposure::Facade,
        sources: vec!["SOURCE".to_owned()],
    });
    let registers = register("ADDRESS", 0, "write-only", "", &field("ADDRESS", 0, 32, ""));
    (pack, registers)
}

#[test]
fn a_dma_address_write_requires_its_typed_address() {
    let (pack, registers) = addressed(None);
    let rust = pack
        .render_typed_rust(&svd(&registers), &Default::default())
        .expect("the typed block renders");
    assert!(rust.contains("pub fn write_address("), "{rust}");
    assert!(
        rust.contains("pub fn write_address(&mut self, value: super::layouts::DmaWindowAddress)"),
        "{rust}"
    );
    assert!(!rust.contains("value: u32) -> &mut Self"), "{rust}");

    let (pack, registers) = addressed(Some("DESCRIPTOR"));
    let rust = pack
        .render_typed_rust(&svd(&registers), &Default::default())
        .expect("the typed block renders");
    assert!(
        rust.contains("value: super::layouts::DmaRef<super::layouts::Descriptor>"),
        "{rust}"
    );

    // A transaction naming the field takes no integer either.
    let (mut pack, registers) = addressed(None);
    pack.zero_based_field_writes
        .push(crate::ZeroBasedFieldWrite {
            name: "publish_raw_address".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "ADDRESS".to_owned(),
            fields: vec!["ADDRESS".to_owned()],
            sources: vec!["SOURCE".to_owned()],
        });
    refused(&pack, &registers, "which has no writer");
}

/// A host stand-in for the PAC root's `layouts` module: a DMA-window
/// address the program makes from an integer.
const LAYOUTS: &str = r#"
mod layouts {
    #[derive(Clone, Copy)]
    pub struct DmaWindowAddress(pub u32);
    impl DmaWindowAddress {
        pub const fn get(self) -> u32 {
            self.0
        }
    }
}
"#;

#[test]
fn a_dma_address_field_writes_its_typed_address_and_refuses_an_integer() {
    let (pack, registers) = addressed(None);
    let rendered = pack
        .render_typed_rust(&svd(&registers), &Default::default())
        .expect("the typed block renders");
    let directory = tempfile::tempdir().expect("a temporary directory");
    let output = run_rendered(
        directory.path(),
        &format!("{rendered}{LAYOUTS}"),
        r#"
fn main() {
    let mut memory = [0_u32; radio::SIZE / 4];
    let mut block = radio::Radio::host(&mut memory).expect("memory holds the block");
    block.write_address(layouts::DmaWindowAddress(0x4080_1000));
    drop(block);
    println!("{:08x}", memory[0]);
}
"#,
    );
    assert_eq!(output.trim(), "40801000");

    let directory = tempfile::tempdir().expect("a temporary directory");
    let stderr = compile_rendered_failure(
        directory.path(),
        &format!("{rendered}{LAYOUTS}"),
        r#"
fn main() {
    let mut memory = [0_u32; radio::SIZE / 4];
    let mut block = radio::Radio::host(&mut memory).expect("memory holds the block");
    block.write_address(0x4080_1000_u32);
}
"#,
    );
    assert!(stderr.contains("mismatched types"), "{stderr}");
}

#[test]
fn dma_commits_on_unsplit_and_task_owners_require_the_typed_argument() {
    for split in [false, true] {
        let (mut pack, registers) = addressed(None);
        if split {
            pack.register_sides.push(RegisterSides {
                peripheral: "RADIO".to_owned(),
                description: "Interrupt status and task DMA address".to_owned(),
                irq: vec!["STATUS".to_owned()],
                irq_observes: Vec::new(),
                sources: vec!["SOURCE".to_owned()],
            });
        }
        let registers =
            registers + &register("STATUS", 4, "read-only", "", &field("DONE", 0, 1, ""));
        let rendered = pack
            .render_typed_rust(&svd(&registers), &Default::default())
            .expect("address owners render");
        assert!(!rendered.contains("pub struct W("), "{rendered}");
        let owner = if split {
            "let (mut block, _) = block.split();"
        } else {
            ""
        };
        for call in ["block.write_address(|_| {});", "block.write_address();"] {
            let directory = tempfile::tempdir().expect("a temporary directory");
            let stderr = compile_rendered_failure(
                directory.path(),
                &format!("{rendered}{LAYOUTS}"),
                &format!(
                    r#"
fn main() {{
    let mut memory = [0_u32; radio::SIZE / 4];
    let mut block = radio::Radio::host(&mut memory).expect("memory holds the block");
    {owner}
    {call}
}}
"#
                ),
            );
            assert!(stderr.contains("DmaWindowAddress"), "{stderr}");
        }
        let directory = tempfile::tempdir().expect("a temporary directory");
        let output = run_rendered(
            directory.path(),
            &format!("{rendered}{LAYOUTS}"),
            &format!(
                r#"
fn main() {{
    let mut memory = [0_u32; radio::SIZE / 4];
    let mut block = radio::Radio::host(&mut memory).expect("memory holds the block");
    {owner}
    block.write_address(layouts::DmaWindowAddress(0x4080_1000));
    drop(block);
    println!("{{:08x}}", memory[0]);
}}
"#
            ),
        );
        assert_eq!(output.trim(), "40801000");
    }
}

#[test]
fn only_a_whole_word_vendor_image_skips_the_declared_field_assertion() {
    let mut pack = typed_pack();
    for (name, whole_word) in [("set_declared", false), ("set_vendor_word", true)] {
        pack.fixed_register_images.push(FixedRegisterImage {
            whole_word,
            masks: None,
            consumed_event: None,
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
    let output = cargo_rendered(directory, rendered, main, "run");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("the program prints text")
}

/// The compiler's errors on a program over `rendered` that must not build.
fn compile_rendered_failure(directory: &std::path::Path, rendered: &str, main: &str) -> String {
    let output = cargo_rendered(directory, rendered, main, "build");
    assert!(!output.status.success(), "the program builds");
    String::from_utf8(output.stderr).expect("the compiler prints text")
}

/// `cargo <command>` on a program of `rendered` and `main` over `oer-mmio`,
/// in `directory`.
fn cargo_rendered(
    directory: &std::path::Path,
    rendered: &str,
    main: &str,
    command: &str,
) -> std::process::Output {
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
    std::process::Command::new(env!("CARGO"))
        .args([command, "--quiet", "--offline", "--manifest-path"])
        .arg(directory.join("Cargo.toml"))
        .env("CARGO_TARGET_DIR", directory.join("target"))
        .current_dir(directory)
        .output()
        .expect("the pinned cargo runs")
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
        reads_pending: false,
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
fn a_mixed_readable_write_only_word_has_no_generic_writer() {
    let rust = typed_pack()
        .render_typed_rust(&svd(&triggered()), &Default::default())
        .expect("a mixed word still has a reader");
    assert!(rust.contains("pub fn read_config("), "{rust}");
    for item in [
        "pub fn modify_config(",
        "pub fn write_config(",
        "pub struct W(",
        "pub const fn update(",
    ] {
        assert!(!rust.contains(item), "{item}\n{rust}");
    }
}

#[test]
fn mixed_word_commands_write_from_zero_and_policy_rmws_are_refused() {
    let mut pack = typed_pack();
    pack.zero_based_field_writes
        .push(crate::ZeroBasedFieldWrite {
            name: "publish_command".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "CONFIG".to_owned(),
            fields: vec!["ENABLE".to_owned(), "UPDATE".to_owned()],
            sources: vec!["SOURCE".to_owned()],
        });
    let rendered = pack
        .render_typed_rust(&svd(&triggered()), &Default::default())
        .expect("the sourced whole-word command renders");
    let directory = tempfile::tempdir().expect("a temporary directory");
    let output = run_rendered(
        directory.path(),
        &rendered,
        r#"
fn main() {
    let mut memory = [u32::MAX; radio::SIZE / 4];
    let mut block = radio::Radio::host(&mut memory).expect("memory holds the block");
    block.publish_command(true, false);
    drop(block);
    println!("{}", memory[0]);
}
"#,
    );
    assert_eq!(
        output.trim(),
        "1",
        "the unreadable reset image is not replayed"
    );
    pack.field_replace_modifies
        .push(crate::pac_api::FieldReplaceModify {
            name: "unsafe_rmw".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "CONFIG".to_owned(),
            fields: vec![crate::pac_api::FieldOrProjection {
                field: "ENABLE".to_owned(),
                source_bit_offset: 0,
            }],
            domain: None,
            value: Some(1),
            input_transform: None,
            exposure: crate::PacApiExposure::Facade,
            sources: vec!["SOURCE".to_owned()],
        });
    refused(&pack, &triggered(), "a read-modify-write would replay");
}

/// Render the reviewed legacy inputs themselves, so a class correction in a
/// synthetic fixture cannot hide a regression in the publisher's inputs.
fn s31_legacy_block(fragment: &str) -> (PacApiPack, String) {
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../chips/esp32s31/registers");
    let fragment: crate::RegisterModelFragment = toml::from_str(
        &std::fs::read_to_string(root.join("model/peripherals").join(fragment))
            .expect("the legacy fragment exists"),
    )
    .expect("the legacy fragment parses");
    let policy: PacApiPack = toml::from_str(
        &std::fs::read_to_string(root.join("policy/api.toml"))
            .expect("the legacy API policy exists"),
    )
    .expect("the legacy API policy parses");
    let name = &fragment.peripherals[0].name;
    let mut pack = typed_pack();
    pack.options.typed_peripherals = vec![name.clone()];
    pack.write_trigger_fields = policy
        .write_trigger_fields
        .into_iter()
        .filter(|f| &f.peripheral == name)
        .collect();
    pack.fixed_register_images = policy
        .fixed_register_images
        .into_iter()
        .filter(|f| &f.peripheral == name)
        .collect();
    pack.zero_based_field_writes = policy
        .zero_based_field_writes
        .into_iter()
        .filter(|f| &f.peripheral == name)
        .collect();
    pack.masked_register_modifies = policy
        .masked_register_modifies
        .into_iter()
        .filter(|f| &f.peripheral == name)
        .collect();
    pack.field_replace_modifies = policy
        .field_replace_modifies
        .into_iter()
        .filter(|f| &f.peripheral == name)
        .collect();
    let mut device = svd_parser::parse(&svd(&block())).expect("the fixture device parses");
    device.peripherals = fragment.peripherals;
    let svd = svd_encoder::encode(&device).expect("the legacy fragment encodes as SVD");
    (pack, svd)
}

#[test]
fn pmu_immediate_clock_power_renders_every_sourced_lane_as_a_trigger() {
    let (pack, svd) = s31_legacy_block("pmu-radio.toml");
    let rust = pack
        .render_typed_rust(&svd, &Default::default())
        .expect("the PMU renders");
    for lane in [
        "tie_low_global_xtalx2_icg",
        "tie_low_global_xtal_icg",
        "tie_low_i2c_retention",
        "tie_low_xpd_bb_i2c",
        "tie_low_xpd_xtalx2",
        "tie_low_xpd_xtal",
        "tie_high_global_xtalx2_icg",
        "tie_high_global_xtal_icg",
        "tie_high_i2c_retention",
        "tie_high_xpd_bb_i2c",
        "tie_high_xpd_xtalx2",
        "tie_high_xpd_xtal",
    ] {
        assert!(
            rust.contains(&format!("pub fn trigger_imm_hp_ck_power_0_{lane}(")),
            "{lane}"
        );
        assert!(!rust.contains(&format!("pub fn set_{lane}(")), "{lane}");
    }
    assert!(!rust.contains("pub fn modify_imm_hp_ck_power_0("));
    for method in [
        "enable_baseband_i2c_power_tie",
        "disable_baseband_i2c_power_tie",
        "clear_phy_rf_immediate_clock_power",
    ] {
        let body = rust
            .split(&format!("pub fn {method}("))
            .nth(1)
            .expect("the policy transaction remains")
            .split("}\n")
            .next()
            .expect("the method has a body");
        assert!(body.contains("& !imm_hp_ck_power_0::TRIGGERS"), "{body}");
    }
}

#[test]
fn a_trigger_and_named_pmu_style_transactions_never_replay_sibling_strobes() {
    // Synthetic lanes: a readable stored bit, three triggers and reserved
    // bits. Their bus images are fixture values, not chip geometry.
    let word = register(
        "POWER",
        0,
        "read-write",
        "",
        &(field("ENABLE", 0, 1, "")
            + &field("BB", 1, 1, "")
            + &field("XTAL", 2, 1, "")
            + &field("XTALX2", 3, 1, "")),
    );
    let mut pack = typed_pack();
    for lane in ["BB", "XTAL", "XTALX2"] {
        pack.write_trigger_fields.push(trigger("POWER", lane));
    }
    pack.field_replace_modifies
        .push(crate::pac_api::FieldReplaceModify {
            name: "enable_bb".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "POWER".to_owned(),
            fields: vec![crate::pac_api::FieldOrProjection {
                field: "BB".to_owned(),
                source_bit_offset: 0,
            }],
            domain: None,
            value: Some(1),
            input_transform: None,
            exposure: crate::PacApiExposure::Facade,
            sources: vec!["SOURCE".to_owned()],
        });
    let rendered = pack
        .render_typed_rust(&svd(&word), &Default::default())
        .expect("the trigger word renders");
    let directory = tempfile::tempdir().expect("a temporary directory");
    let output = run_rendered(
        directory.path(),
        &rendered,
        r#"
fn main() {
    for named in [false, true] {
        let mut memory = [u32::MAX; radio::SIZE / 4];
        let mut block = radio::Radio::host(&mut memory).expect("memory holds the block");
        if named { block.enable_bb(); } else { block.trigger_power_bb(); }
        assert!(block.read_power().enable(), "the stored lane is preserved");
        drop(block);
        println!("{:08x}", memory[0]);
    }
}
"#,
    );
    assert_eq!(
        output.trim(),
        "fffffff3\nfffffff3",
        "reserved lanes are preserved, sibling triggers are zero"
    );
}

#[test]
fn analog_completion_is_read_only_and_the_reset_word_has_no_rmw() {
    let (pack, svd) = s31_legacy_block("i2c-ana-mst.toml");
    let rust = pack
        .render_typed_rust(&svd, &Default::default())
        .expect("the analog host renders");
    for method in [
        "read_i2c0_ctrl",
        "read_i2c1_ctrl",
        "read_ana_conf0",
        "read_ana_conf1",
        "read_ana_conf2",
    ] {
        assert!(rust.contains(&format!("pub fn {method}(")), "{method}");
    }
    for method in [
        "modify_i2c0_ctrl",
        "modify_i2c1_ctrl",
        "write_i2c0_ctrl",
        "write_i2c1_ctrl",
        "set_busy",
        "set_start_or_reset",
        "set_ana_status0",
        "set_read_mask_complement_high",
        "set_ana_status2",
    ] {
        assert!(!rust.contains(&format!("pub fn {method}(")), "{method}");
    }
    assert!(!rust.contains("pub const fn start_or_reset("));
    for getter in [
        "busy",
        "ana_status0",
        "read_mask_complement_high",
        "ana_status2",
    ] {
        assert!(
            rust.contains(&format!("pub const fn {getter}(")),
            "{getter}"
        );
    }
}

#[test]
fn the_documented_pac_boundary_keeps_svd_names_and_named_policy_transactions() {
    let (pack, svd) = s31_legacy_block("i2c-ana-mst.toml");
    let rust = pack
        .render_typed_rust(&svd, &Default::default())
        .expect("the documented SVD/policy path renders");
    for method in [
        "pulse_phy_i2c_host0_reset",
        "pulse_phy_i2c_host1_reset",
        "publish_phy_i2c_host0_command",
        "publish_phy_i2c_host1_command",
        "publish_phy_i2c_read_mask",
    ] {
        assert!(
            rust.contains(&format!("pub fn {method}(")),
            "the named transaction {method} remains"
        );
    }
    assert!(rust.contains("value: super::PhyI2cReadMask"));
    assert!(
        rust.contains("pub const fn read_mask_complement_high("),
        "the reviewed SVD name remains"
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
fn a_write_trigger_field_wider_than_one_bit_is_written_with_its_value() {
    let wide = register(
        "CONFIG",
        0,
        "read-write",
        "",
        &(field("ENABLE", 0, 1, "") + &field("UPDATE", 28, 4, "<access>write-only</access>")),
    );
    let mut pack = typed_pack();
    pack.write_trigger_fields.push(trigger("CONFIG", "UPDATE"));
    let rust = pack
        .render_typed_rust(&svd(&wide), &Default::default())
        .unwrap();
    assert!(rust.contains("value: oer_mmio::Bits<4>"), "{rust}");
    assert!(rust.contains("| (value.get() << 28)"), "{rust}");
}

fn command(commands: &[&str], qualifiers: &[&str], held: &[&str]) -> crate::CommandRegister {
    let names = |fields: &[&str]| fields.iter().map(|field| (*field).to_owned()).collect();
    crate::CommandRegister {
        peripheral: "RADIO".to_owned(),
        register: "CONFIG".to_owned(),
        commands: names(commands),
        qualifiers: names(qualifiers),
        held: names(held),
        sources: vec!["FIXTURE".to_owned()],
    }
}

/// `CONFIG` with two one-bit commands, a qualifier, a held bit, a field
/// only read and a two-bit field.
fn commanded(level: &str) -> String {
    register(
        "CONFIG",
        0,
        "read-write",
        "",
        &(field("GO", 0, 1, "")
            + &field("STOP", 1, 1, "")
            + &field("ERASE", 2, 1, "")
            + &field("DEBUG", 3, 1, "")
            + &field("LEVEL", 4, 2, level)),
    )
}

#[test]
fn a_command_register_is_written_only_by_starts_that_refuse_while_a_bit_runs() {
    let mut pack = typed_pack();
    pack.command_registers
        .push(command(&["GO", "STOP"], &["ERASE"], &["DEBUG"]));
    let rust = pack
        .render_typed_rust(
            &svd(&commanded("<access>read-only</access>")),
            &Default::default(),
        )
        .unwrap();
    for item in [
        "pub const COMMANDS: u32 = 0x00000003;",
        "pub const BUSY: u32 = 0x0000000f;",
        "pub fn start_config_go(&mut self, erase: bool, ) -> Result<(), oer_mmio::CommandBusy>",
        "if value & config::BUSY != 0",
        "(value & !config::BUSY) | 0x00000001 | if erase { 0x00000004 } else { 0 }",
        "pub fn config_stop_busy(",
        "pub const fn debug(",
        "pub const fn level(",
    ] {
        assert!(rust.contains(item), "{item}\n{rust}");
    }
    for absent in [
        "pub fn modify_config(",
        "pub fn set_go(",
        "pub fn start_config_erase(",
        "pub fn start_config_debug(",
        "pub fn config_erase_busy(",
    ] {
        assert!(!rust.contains(absent), "{absent}\n{rust}");
    }
}

#[test]
fn a_command_register_with_a_setter_a_wide_command_or_a_trigger_is_refused() {
    let mut pack = typed_pack();
    pack.command_registers.push(command(&["GO"], &[], &[]));
    let error = pack
        .render_typed_rust(&svd(&commanded("")), &Default::default())
        .unwrap_err();
    assert!(
        error.to_string().contains("written only by its starts"),
        "{error}"
    );
    let mut pack = typed_pack();
    pack.command_registers.push(command(&["LEVEL"], &[], &[]));
    let error = pack
        .render_typed_rust(
            &svd(&commanded("<access>read-write</access>")),
            &Default::default(),
        )
        .unwrap_err();
    assert!(
        error.to_string().contains("one readable and writable bit"),
        "{error}"
    );
    let mut pack = typed_pack();
    pack.command_registers.push(command(&["GO"], &[], &[]));
    pack.write_trigger_fields.push(trigger("CONFIG", "STOP"));
    let error = pack.validate().unwrap_err();
    assert!(
        error.to_string().contains("holds no write trigger"),
        "{error}"
    );
}

#[test]
fn a_data_port_is_read_and_stored_whole_never_modified() {
    let words = register("CONFIG", 0, "read-write", "", &field("BUF", 0, 32, ""));
    let mut pack = typed_pack();
    pack.data_ports.push(crate::DataPort {
        peripheral: "RADIO".to_owned(),
        register: "CONFIG".to_owned(),
        sources: vec!["FIXTURE".to_owned()],
    });
    let rust = pack
        .render_typed_rust(&svd(&words), &Default::default())
        .unwrap();
    assert!(rust.contains("pub fn write_config("), "{rust}");
    assert!(rust.contains("pub fn read_config("), "{rust}");
    assert!(!rust.contains("pub fn modify_config("), "{rust}");
    let mut pack = typed_pack();
    pack.data_ports.push(crate::DataPort {
        peripheral: "RADIO".to_owned(),
        register: "CONFIG".to_owned(),
        sources: vec!["FIXTURE".to_owned()],
    });
    let error = pack
        .render_typed_rust(&svd(&commanded("")), &Default::default())
        .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("one readable and writable field"),
        "{error}"
    );
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
fn a_whole_word_fifo_port_pops_the_word_it_reads() {
    let mut pack = typed_pack();
    pack.fifo_ports.push(fifo("DATA"));
    let word = register("DATA", 0, "read-only", "", &field("VALUE", 0, 32, ""));
    let rust = pack
        .render_typed_rust(&svd(&word), &Default::default())
        .unwrap();
    assert!(
        rust.contains("pub fn pop_data(&mut self) -> u32 {\nself.0.read::<0x0>()\n}"),
        "{rust}"
    );
    assert!(!rust.contains("pub fn push_data("), "{rust}");
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
        masks: None,
        consumed_event: None,
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
        masks: None,
        consumed_event: None,
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
        masks: None,
        consumed_event: None,
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

fn pointer_projection(name: &str, value: Option<u32>) -> crate::FieldOrModify {
    crate::FieldOrModify {
        name: name.to_owned(),
        peripheral: "RADIO".to_owned(),
        register: "HEAD".to_owned(),
        fields: vec![crate::pac_api::FieldOrProjection {
            field: "ADDRESS_LOW".to_owned(),
            source_bit_offset: 0,
        }],
        domain: None,
        value,
        exposure: crate::PacApiExposure::Facade,
        sources: vec!["SOURCE".to_owned()],
    }
}

#[test]
fn a_pointer_field_transaction_takes_the_image_or_zero() {
    let mut pack = pointer_pack(Some(crate::RegisterPointerEncoding::BleCompressed));
    pack.field_or_modifies
        .push(pointer_projection("or_head", None));
    pack.field_clear_modifies
        .push(pointer_projection("clear_head", Some(0xf_ffff)));
    pack.field_replace_modifies
        .push(crate::pac_api::FieldReplaceModify {
            name: "replace_head".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "HEAD".to_owned(),
            fields: vec![crate::pac_api::FieldOrProjection {
                field: "ADDRESS_LOW".to_owned(),
                source_bit_offset: 0,
            }],
            domain: None,
            value: None,
            input_transform: None,
            exposure: crate::PacApiExposure::Facade,
            sources: vec!["SOURCE".to_owned()],
        });
    // A clear by AND-NOT of the field's bits is no zero the image grants.
    refused(&pack, &pointed(), "only a typed DMA reference or zero sets");

    pack.field_clear_modifies.clear();
    let mut zero = pack.field_replace_modifies[0].clone();
    zero.name = "zero_head".to_owned();
    zero.value = Some(0);
    pack.field_replace_modifies.push(zero);
    let rust = pack
        .render_typed_rust(&svd(&pointed()), &Default::default())
        .expect("the image and zero transactions render");
    assert!(
        rust.contains("pub fn or_head(&mut self, value: head::AddressLowImage)"),
        "{rust}"
    );
    assert!(
        rust.contains("pub fn replace_head(&mut self, value: head::AddressLowImage)"),
        "{rust}"
    );
    assert!(rust.contains("pub fn zero_head(&mut self"), "{rust}");
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
    pack.zero_based_field_writes
        .push(crate::ZeroBasedFieldWrite {
            name: "clear_done".to_owned(),
            peripheral: "RADIO".to_owned(),
            register: "CLEAR".to_owned(),
            fields: vec!["DONE".to_owned()],
            sources: vec!["SOURCE".to_owned()],
        });
    refused(
        &pack,
        &events(&field("DONE", 0, 1, "")),
        "only its snapshot's acknowledgement writes",
    );
}

fn stale_clear_pack(masks: Option<&str>, consumed_event: Option<&str>) -> PacApiPack {
    let mut pack = snapshot_pack();
    pack.fixed_register_images.push(FixedRegisterImage {
        whole_word: false,
        masks: masks.map(str::to_owned),
        consumed_event: consumed_event.map(str::to_owned),
        name: "clear_stale_done".to_owned(),
        peripheral: "RADIO".to_owned(),
        register: "CLEAR".to_owned(),
        value: 0x1,
        sources: vec!["SOURCE".to_owned()],
    });
    pack
}

/// The events block with the enable register of its sources.
fn enabled_events() -> String {
    events(&field("DONE", 0, 1, ""))
        + &register(
            "ENABLE",
            8,
            "read-write",
            "",
            &(field("DONE", 0, 1, "") + &field("ERROR", 1, 1, "")),
        )
}

#[test]
fn a_stale_source_clear_masks_its_sources_before_it_acknowledges_them() {
    let rust = stale_clear_pack(Some("ENABLE"), None)
        .render_typed_rust(&svd(&enabled_events()), &Default::default())
        .expect("the masking fixed image on the clear register renders");
    assert!(
        rust.contains(
            "pub fn clear_stale_done(&mut self, ) {\nlet bits = self.0.read::<0x8>();\nself.0.write::<0x8>(bits & !0x00000001);\nself.0.write::<0x4>(0x00000001);\n}"
        ),
        "{rust}"
    );
    assert!(
        rust.contains("within(0x00000001, enable::FIELDS)"),
        "{rust}"
    );
    assert!(rust.contains("pub fn acknowledge_events("), "{rust}");
    assert!(!rust.contains("pub fn write_clear("), "{rust}");
}

#[test]
fn a_consumed_event_acknowledgement_stores_its_image_alone() {
    let rust = stale_clear_pack(None, Some("the one consumer of DONE."))
        .render_typed_rust(&svd(&enabled_events()), &Default::default())
        .expect("the consumed acknowledgement renders");
    assert!(
        rust.contains(
            "pub fn clear_stale_done(&mut self, ) {\nself.0.write::<0x4>(0x00000001);\n}"
        ),
        "{rust}"
    );
    assert!(rust.contains("the one consumer of DONE."), "{rust}");
}

#[test]
fn a_clear_register_image_without_its_proof_is_refused() {
    refused(
        &stale_clear_pack(None, None),
        &enabled_events(),
        "that no sample captured",
    );
    refused(
        &stale_clear_pack(Some("ENABLE"), Some("both.")),
        &enabled_events(),
        "names one proof",
    );
    refused(
        &stale_clear_pack(Some("CLEAR"), None),
        &enabled_events(),
        "no plain enable register",
    );
    refused(
        &stale_clear_pack(Some("MISSING"), None),
        &enabled_events(),
        "no plain enable register",
    );
    let mut pack = stale_clear_pack(Some("ENABLE"), None);
    pack.data_ports.push(crate::DataPort {
        peripheral: "RADIO".to_owned(),
        register: "ENABLE".to_owned(),
        sources: vec!["SOURCE".to_owned()],
    });
    refused(
        &pack,
        &(events(&field("DONE", 0, 1, ""))
            + &register("ENABLE", 8, "read-write", "", &field("DONE", 0, 1, ""))),
        "no plain enable register",
    );
    let mut pack = stale_clear_pack(Some("ENABLE"), None);
    pack.fixed_register_images[0].register = "ENABLE".to_owned();
    refused(&pack, &enabled_events(), "is no snapshot's clear register");
}

#[test]
fn a_whole_word_event_field_is_dropped_from_a_snapshot_without_an_and() {
    let rust = snapshot_pack()
        .render_typed_rust(
            &svd(
                &(register("STATUS", 0, "read-only", "", &field("PENDING", 0, 32, ""))
                    + &register(
                        "CLEAR",
                        4,
                        "write-only",
                        "<modifiedWriteValues>oneToClear</modifiedWriteValues>",
                        &field("PENDING", 0, 32, ""),
                    )),
            ),
            &Default::default(),
        )
        .expect("the whole-word snapshot renders");
    assert!(rust.contains("if keep { self } else { Self(0) }"), "{rust}");
    assert!(!rust.contains("& !0xffffffff"), "{rust}");
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
fn a_partition_image_reads_no_word_of_a_block_without_a_readable_register() {
    let mut pack = partitioned_pack();
    pack.partition_image_reads.push(crate::PartitionImageRead {
        name: "image".to_owned(),
        partition: pack.ownership_partitions[0].name.clone(),
        exposure: crate::PacApiExposure::Facade,
        sources: vec!["SOURCE".to_owned()],
    });
    let commands = register("COMMAND", 0, "write-only", "", &field("OPCODE", 0, 8, ""));
    let rust = pack
        .render_typed_rust(&svd(&commands), &Default::default())
        .expect("the write-only block renders");
    assert!(!rust.contains("fn read_image_word"), "{rust}");
    assert!(rust.contains("pub const IMAGE_LEN: usize = 0;"), "{rust}");
}

#[test]
fn an_enumeration_with_a_value_above_i32_max_takes_the_register_width() {
    let values = |high: u32| {
        format!(
            "<enumeratedValues><enumeratedValue><name>CLOSED</name><value>0</value></enumeratedValue>\
             <enumeratedValue><name>OPEN</name><value>{high:#x}</value></enumeratedValue></enumeratedValues>"
        )
    };
    let gates = |high| {
        register(
            "GATES",
            0,
            "read-write",
            "",
            &field("STATE", 0, 32, &values(high)),
        )
    };
    let wide = typed_pack()
        .render_typed_rust(&svd(&gates(0xffff_ffff)), &Default::default())
        .expect("the wide enumeration renders");
    assert!(wide.contains("#[repr(u32)]\npub enum State"), "{wide}");
    let narrow = typed_pack()
        .render_typed_rust(&svd(&gates(0x7fff_ffff)), &Default::default())
        .expect("the narrow enumeration renders");
    assert!(!narrow.contains("#[repr(u32)]"), "{narrow}");
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
