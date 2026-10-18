use std::collections::BTreeMap;
use std::sync::Arc;

use oer_riscv_model::{
    AbstractValue, ArtifactId, DecodedOp, EdgeKind, FunctionRecord, FunctionRelocation,
    InstructionFlow, MemoryKind, ObjectId, ObjectLocation, ReferenceKind, ReferenceTarget,
    SymbolDefinition, SymbolId, SymbolTableKind, ValueAlternative, ValueAlternatives,
};

use super::{ImageSymbols, SymbolReference, is_executable_image, listing, references_to};

fn reference(offset: u64, name: &str, addend: i64, kind: ReferenceKind) -> FunctionRecord {
    let target = Arc::new(ReferenceTarget {
        binding: 1,
        definition: SymbolDefinition::Undefined,
        symbol: SymbolId {
            object: ObjectId {
                artifact: ArtifactId::of_bytes(b"library"),
                location: ObjectLocation::ArchiveMember { ordinal: 3 },
            },
            table: SymbolTableKind::Static,
            table_section: 9,
            index: 1,
        },
        name: name.as_bytes().to_vec(),
        section: None,
        offset: 0,
        symbol_type: 2,
    });
    FunctionRecord::Reference {
        raw: Box::new(FunctionRelocation {
            section: 1,
            index: 0,
            offset,
            relocation_type: 19,
            addend: Some(addend),
            target: target.clone(),
        }),
        reference_kind: kind,
        target,
        addend: Some(addend),
        paired: None,
        known: true,
    }
}

fn instruction(offset: u64, text: &str) -> FunctionRecord {
    FunctionRecord::Instruction {
        offset,
        bytes: vec![0; 4],
        decoded: DecodedOp {
            length: 4,
            text: text.into(),
            flow: InstructionFlow::Next,
        },
    }
}

fn records() -> Vec<FunctionRecord> {
    vec![
        instruction(0, "lui a5, 0"),
        reference(0, "g_pm", 52, ReferenceKind::Address),
        instruction(4, "auipc ra, 0"),
        reference(4, "pm_scale_listen_interval", 0, ReferenceKind::Call),
        instruction(8, "ret"),
    ]
}

#[test]
fn the_listing_names_each_instructions_relocated_symbols() {
    assert_eq!(
        listing(&records(), None, &BTreeMap::new()),
        [
            format!("{:6x}  {:<40} g_pm+52", 0, "lui a5, 0"),
            format!("{:6x}  {:<40} pm_scale_listen_interval", 4, "auipc ra, 0"),
            format!("{:6x}  ret", 8),
        ]
    );
}

#[test]
fn references_to_select_the_named_targets_with_their_kind() {
    let found = references_to(&records(), &["pm_scale_listen_interval".into()]);
    assert_eq!(
        found,
        [SymbolReference {
            offset: 4,
            kind: ReferenceKind::Call,
            target: b"pm_scale_listen_interval".to_vec(),
        }]
    );
    assert!(references_to(&records(), &["pm_start".into()]).is_empty());
}

fn value(offset: u64, register: u8, value: AbstractValue) -> FunctionRecord {
    FunctionRecord::Value {
        offset,
        register,
        value,
        relocation: None,
    }
}

fn access(
    offset: u64,
    access: MemoryKind,
    address: AbstractValue,
    stored: Option<AbstractValue>,
) -> FunctionRecord {
    FunctionRecord::MemoryAccess {
        offset,
        access,
        width: 4,
        address,
        value: stored,
        relocation: None,
    }
}

#[test]
fn the_listing_annotates_exact_values_and_addresses_in_hexadecimal() {
    let constant = |value| AbstractValue::Constant { value };
    let records = vec![
        instruction(0, "lui a4, 131337"),
        value(0, 14, constant(0x2010_9000)),
        instruction(4, "lw a5, 4(a4)"),
        access(4, MemoryKind::Load, constant(0x2010_9004), None),
        value(4, 15, AbstractValue::Unknown),
        instruction(8, "sw a3, 24(a4)"),
        access(
            8,
            MemoryKind::Store,
            constant(0x2010_9018),
            Some(constant(0x1b)),
        ),
        instruction(12, "sw ra, 60(sp)"),
        access(
            12,
            MemoryKind::Store,
            AbstractValue::EntryStack { offset: -4 },
            None,
        ),
        instruction(14, "ret"),
        value(14, 0, constant(0)),
        instruction(16, "addi sp, sp, -64"),
        value(16, 2, AbstractValue::EntryStack { offset: -64 }),
        instruction(20, "addi a0, sp, 12"),
        value(20, 10, AbstractValue::EntryStack { offset: -52 }),
        instruction(24, "mv a1, a2"),
        value(
            24,
            11,
            AbstractValue::Alternatives {
                values: ValueAlternatives::new(vec![
                    ValueAlternative::Constant { value: 2 },
                    ValueAlternative::Constant { value: 0x40 },
                ])
                .unwrap(),
            },
        ),
    ];
    assert_eq!(
        listing(&records, None, &BTreeMap::new()),
        [
            format!("{:6x}  {:<40}  # a4=0x20109000", 0, "lui a4, 131337"),
            format!("{:6x}  {:<40}  # [0x20109004]", 4, "lw a5, 4(a4)"),
            format!("{:6x}  {:<40}  # [0x20109018] <- 0x1b", 8, "sw a3, 24(a4)"),
            format!("{:6x}  sw ra, 60(sp)", 12),
            format!("{:6x}  ret", 14),
            format!("{:6x}  addi sp, sp, -64", 16),
            format!("{:6x}  {:<40}  # a0=sp-0x34", 20, "addi a0, sp, 12"),
            format!("{:6x}  {:<40}  # a1=one-of{{0x2 | 0x40}}", 24, "mv a1, a2"),
        ]
    );
}

#[test]
fn a_symbol_value_is_named_with_its_addend() {
    let symbol = match reference(0, "phy_param", 0, ReferenceKind::Address) {
        FunctionRecord::Reference { target, .. } => target.symbol.clone(),
        _ => unreachable!(),
    };
    let records = vec![
        instruction(0, "lui s0, 0"),
        reference(0, "phy_param", 0, ReferenceKind::Address),
        value(
            0,
            8,
            AbstractValue::Symbol {
                symbol,
                addend: 239,
            },
        ),
    ];
    assert_eq!(
        listing(&records, None, &BTreeMap::new()),
        [format!(
            "{:6x}  {:<40} phy_param  # s0=phy_param+0xef",
            0, "lui s0, 0"
        )]
    );
}

#[test]
fn a_pcrel_low_value_is_named_by_its_high_target_not_the_label() {
    let FunctionRecord::Reference { target: high, .. } =
        reference(0, "phy_param", 0, ReferenceKind::Address)
    else {
        unreachable!()
    };
    let label = Arc::new(ReferenceTarget {
        name: b".Lpcrel_hi0".to_vec(),
        symbol: SymbolId {
            index: 7,
            ..high.symbol.clone()
        },
        ..(*high).clone()
    });
    let low = FunctionRecord::Reference {
        raw: Box::new(FunctionRelocation {
            section: 1,
            index: 1,
            offset: 4,
            relocation_type: 24,
            addend: Some(0),
            target: label,
        }),
        reference_kind: ReferenceKind::Address,
        target: high.clone(),
        addend: Some(0),
        paired: Some((0, 0)),
        known: true,
    };
    let records = vec![
        instruction(0, "auipc a5, 0"),
        reference(0, "phy_param", 0, ReferenceKind::Address),
        instruction(4, "addi a5, a5, 0"),
        low,
        value(
            4,
            15,
            AbstractValue::Symbol {
                symbol: high.symbol.clone(),
                addend: 0x10,
            },
        ),
    ];
    assert_eq!(
        listing(&records, None, &BTreeMap::new())[1],
        format!(
            "{:6x}  {:<40} .Lpcrel_hi0  # a5=phy_param+0x10",
            4, "addi a5, a5, 0"
        ),
        "the relocation column keeps its own label; the value names the HI20 target"
    );
}

#[test]
fn image_transfers_and_branches_are_named_from_the_function_symbols() {
    let image = ImageSymbols::new([
        (0x2f82_6024, 0xe, "phy_get_data_sat".to_string()),
        (0x2f82_6242, 0x108, "phy_rc_cal".to_string()),
        (
            0x2f82_6242,
            0x108,
            "phy_rc_cal_alias_with_longer_name".to_string(),
        ),
        (0x2f82_7000, 0, "unsized".to_string()),
    ]);
    let records = vec![
        instruction(0x2f82_62ac, "jal ra, -648"),
        FunctionRecord::Transfer {
            offset: 0x2f82_62ac,
            target: AbstractValue::ImageAddress {
                address: 0x2f82_6024,
            },
            call: true,
        },
        instruction(0x2f82_6264, "blt a4, zero, 210"),
        FunctionRecord::Edge {
            from: 0x2f82_6264,
            target: Some(0x2f82_6336),
            relation: EdgeKind::Taken,
            external: false,
        },
        FunctionRecord::Edge {
            from: 0x2f82_6264,
            target: Some(0x2f82_6268),
            relation: EdgeKind::Fallthrough,
            external: false,
        },
        instruction(0x2f82_6300, "jal ra, 3326"),
        FunctionRecord::Transfer {
            offset: 0x2f82_6300,
            target: AbstractValue::ImageAddress {
                address: 0x2f82_7000,
            },
            call: true,
        },
    ];
    assert_eq!(
        listing(&records, Some(&image), &BTreeMap::new()),
        [
            format!(
                "{:6x}  {:<40}  # -> phy_get_data_sat",
                0x2f82_62ac_u64, "jal ra, -648"
            ),
            format!(
                "{:6x}  {:<40}  # -> phy_rc_cal+0xf4",
                0x2f82_6264_u64, "blt a4, zero, 210"
            ),
            format!(
                "{:6x}  {:<40}  # -> 0x2f827000",
                0x2f82_6300_u64, "jal ra, 3326"
            ),
        ]
    );
    assert_eq!(
        listing(&records, None, &BTreeMap::new()),
        [
            format!("{:6x}  jal ra, -648", 0x2f82_62ac_u64),
            format!("{:6x}  blt a4, zero, 210", 0x2f82_6264_u64),
            format!("{:6x}  jal ra, 3326", 0x2f82_6300_u64),
        ],
        "without an image the relocations remain the only names"
    );
}

#[test]
fn only_a_little_endian_executable_header_is_an_image() {
    let mut header = vec![0x7f, b'E', b'L', b'F', 1, 1, 1, 0];
    header.resize(16, 0);
    let image = [header.clone(), vec![2, 0]].concat();
    assert!(is_executable_image(&image));
    assert!(
        !is_executable_image(&[header.clone(), vec![1, 0]].concat()),
        "ET_REL"
    );
    let mut big = image.clone();
    big[5] = 2;
    assert!(!is_executable_image(&big), "big-endian");
    assert!(!is_executable_image(b"!<arch>\n"), "an archive");
    assert!(!is_executable_image(&header), "a truncated header");
}

#[test]
fn referenced_read_only_text_is_quoted_with_escapes() {
    let records = vec![
        instruction(0x2c2, "lui a0, 0"),
        reference(0x2c2, ".LC0", 0, ReferenceKind::Address),
    ];
    let strings = BTreeMap::from([(0x2c2, vec!["0x%x,0x%x\\n".to_string()])]);
    assert_eq!(
        listing(&records, None, &strings),
        [format!(
            "{:6x}  {:<40} .LC0  # \"0x%x,0x%x\\n\"",
            0x2c2, "lui a0, 0"
        )]
    );
}
