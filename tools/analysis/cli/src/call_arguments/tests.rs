use std::sync::Arc;

use oer_analysis_riscv_model::{
    AbstractValue, ArtifactId, Expression, FunctionRecord, FunctionRelocation, ObjectId,
    ObjectLocation, ReferenceKind, ReferenceTarget, SymbolDefinition, SymbolId, SymbolTableKind,
    ValueAlternative, ValueAlternatives,
};

use super::{ArgumentValue, CallSite};
use crate::listing::ImageSymbols;

fn symbol(index: u64) -> SymbolId {
    SymbolId {
        object: ObjectId {
            artifact: ArtifactId::of_bytes(b"library"),
            location: ObjectLocation::ArchiveMember { ordinal: 0 },
        },
        table: SymbolTableKind::Static,
        table_section: 9,
        index,
    }
}

fn reference(offset: u64, name: &str, index: u64, kind: ReferenceKind) -> FunctionRecord {
    let target = Arc::new(ReferenceTarget {
        binding: 1,
        definition: SymbolDefinition::Undefined,
        symbol: symbol(index),
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
            addend: Some(0),
            target: target.clone(),
        }),
        reference_kind: kind,
        target,
        addend: Some(0),
        paired: None,
        known: true,
    }
}

fn inputs(offset: u64, arguments: &[(usize, AbstractValue)]) -> FunctionRecord {
    let mut registers = vec![AbstractValue::Unknown; 32];
    for (register, value) in arguments {
        registers[*register] = value.clone();
    }
    FunctionRecord::CallInputs { offset, registers }
}

fn constant(value: u32) -> AbstractValue {
    AbstractValue::Constant { value }
}

#[test]
fn a_relocated_call_reports_each_argument_kind() {
    let records = vec![
        reference(0x10, "phy_param", 2, ReferenceKind::Address),
        FunctionRecord::Expression {
            id: 5,
            offset: 0,
            expression: Expression::EntryRegister { register: 10 },
        },
        reference(0x28, "phy_i2c_writeReg", 1, ReferenceKind::Call),
        inputs(
            0x2c,
            &[
                (10, constant(0x67)),
                (11, constant(1)),
                (12, AbstractValue::Expression { id: 5 }),
                (
                    13,
                    AbstractValue::Alternatives {
                        values: ValueAlternatives::new(vec![
                            ValueAlternative::Constant { value: 4 },
                            ValueAlternative::Constant { value: 5 },
                        ])
                        .unwrap(),
                    },
                ),
                (
                    14,
                    AbstractValue::Symbol {
                        symbol: symbol(2),
                        addend: 0xef,
                    },
                ),
            ],
        ),
        reference(0x40, "phy_i2c_writeReg", 1, ReferenceKind::Call),
        reference(0x50, "phy_other", 3, ReferenceKind::Call),
        inputs(0x54, &[(10, constant(9))]),
    ];
    let sites = call_sites(&records, &["phy_i2c_writeReg".into()], None);
    assert_eq!(
        sites.len(),
        2,
        "two calls of the named function; phy_other is not named"
    );
    assert_eq!(
        sites[0].offset, 0x2c,
        "the register state is the jalr of the pair"
    );
    assert_eq!(
        sites[0].arguments,
        [
            ArgumentValue::Constant { value: 0x67 },
            ArgumentValue::Constant { value: 1 },
            ArgumentValue::EntryArgument { index: 0 },
            ArgumentValue::Alternatives {
                values: vec![
                    ArgumentValue::Constant { value: 4 },
                    ArgumentValue::Constant { value: 5 },
                ]
            },
            ArgumentValue::Symbol {
                name: b"phy_param".to_vec(),
                addend: 0xef,
            },
            ArgumentValue::Unknown,
            ArgumentValue::Unknown,
            ArgumentValue::Unknown,
        ]
    );
    let human: Vec<_> = sites[0]
        .arguments
        .iter()
        .map(ArgumentValue::human)
        .collect();
    assert_eq!(
        human,
        [
            "0x67",
            "0x1",
            "arg0",
            "one-of{0x4 | 0x5}",
            "phy_param+0xef",
            "?",
            "?",
            "?"
        ]
    );
    assert_eq!(
        sites[1],
        CallSite {
            offset: 0x44,
            target: b"phy_i2c_writeReg".to_vec(),
            kind: crate::references::Kind::Relocation,
            destination: None,
            alternatives: None,
            arguments: Vec::new(),
        },
        "a call without retained register state says so instead of guessing; \
         its site is the pair's jalr, as for a call with state"
    );
}

#[test]
fn an_image_transfer_to_a_named_function_is_a_call_site() {
    let image = ImageSymbols::new([(0x2f82_a3a8, 0x2c, "phy_i2c_writeReg_Mask".to_string())]);
    let records = vec![
        FunctionRecord::Transfer {
            offset: 0x2f82_a64e,
            target: AbstractValue::ImageAddress {
                address: 0x2f82_a3a8,
            },
            call: true,
        },
        inputs(
            0x2f82_a64e,
            &[(10, constant(107)), (11, constant(1)), (12, constant(17))],
        ),
        FunctionRecord::Transfer {
            offset: 0x2f82_a700,
            target: AbstractValue::ImageAddress {
                address: 0x2f82_0000,
            },
            call: true,
        },
    ];
    let sites = call_sites(&records, &["phy_i2c_writeReg_Mask".into()], Some(&image));
    assert_eq!(sites.len(), 1);
    assert_eq!(sites[0].offset, 0x2f82_a64e);
    assert_eq!(sites[0].target, b"phy_i2c_writeReg_Mask");
    assert_eq!(
        &sites[0].arguments[..3],
        [
            ArgumentValue::Constant { value: 107 },
            ArgumentValue::Constant { value: 1 },
            ArgumentValue::Constant { value: 17 },
        ]
    );
    assert!(call_sites(&records, &["phy_i2c_writeReg_Mask".into()], None).is_empty());
}

#[test]
fn a_jump_without_its_own_register_state_never_borrows_the_next_call() {
    let records = vec![
        reference(0x40, "phy_i2c_writeReg", 1, ReferenceKind::Branch),
        reference(0x44, "other", 3, ReferenceKind::Call),
        inputs(0x44, &[(10, constant(9))]),
        inputs(0x48, &[(10, constant(7))]),
    ];
    assert_eq!(
        call_sites(&records, &["phy_i2c_writeReg".into()], None),
        [CallSite {
            offset: 0x40,
            target: b"phy_i2c_writeReg".to_vec(),
            kind: crate::references::Kind::Relocation,
            destination: None,
            alternatives: None,
            arguments: Vec::new(),
        }],
        "`j phy_i2c_writeReg` at 0x40 has no state; 0x44 and 0x48 belong to other transfers"
    );
    let jal = vec![
        reference(0x40, "phy_i2c_writeReg", 1, ReferenceKind::Branch),
        inputs(0x40, &[(10, constant(0x67))]),
    ];
    assert_eq!(
        call_sites(&jal, &["phy_i2c_writeReg".into()], None)[0].arguments[0],
        ArgumentValue::Constant { value: 0x67 },
        "a linked `jal` relocation carries its own state"
    );
}

#[test]
fn late_dictionary_facts_and_duplicate_inputs_use_last_definitions() {
    let records = vec![
        reference(0x28, "callee", 1, ReferenceKind::Call),
        inputs(0x2c, &[(10, constant(9))]),
        inputs(
            0x2c,
            &[
                (10, AbstractValue::Expression { id: u32::MAX }),
                (
                    11,
                    AbstractValue::Symbol {
                        symbol: symbol(2),
                        addend: -8,
                    },
                ),
                (
                    12,
                    AbstractValue::Symbol {
                        symbol: symbol(3),
                        addend: 0,
                    },
                ),
            ],
        ),
        FunctionRecord::Expression {
            id: u32::MAX,
            offset: 0,
            expression: Expression::EntryRegister { register: 12 },
        },
        FunctionRecord::Expression {
            id: u32::MAX,
            offset: 0,
            expression: Expression::EntryRegister { register: 17 },
        },
        reference(0, "first", 2, ReferenceKind::Address),
        reference(0, "last", 2, ReferenceKind::Address),
    ];
    let sites = call_sites(&records, &["callee".into()], None);
    assert_eq!(
        &sites[0].arguments[..3],
        [
            ArgumentValue::EntryArgument { index: 7 },
            ArgumentValue::Symbol {
                name: b"last".to_vec(),
                addend: -8
            },
            ArgumentValue::Symbol {
                name: b"symbol3".to_vec(),
                addend: 0
            }
        ]
    );
}

#[test]
fn incomplete_state_missing_and_cyclic_expressions_remain_unknown() {
    let mut records = vec![
        reference(0, "callee", 1, ReferenceKind::Branch),
        FunctionRecord::CallInputs {
            offset: 0,
            registers: vec![constant(7); 17],
        },
    ];
    assert!(
        call_sites(&records, &["callee".into()], None)[0]
            .arguments
            .is_empty()
    );
    records.push(inputs(
        0,
        &[
            (10, AbstractValue::Expression { id: 999 }),
            (11, AbstractValue::Expression { id: 4 }),
        ],
    ));
    records.push(FunctionRecord::Expression {
        id: 4,
        offset: 0,
        expression: Expression::Load {
            address: AbstractValue::Expression { id: 4 },
            width: 4,
            signed: false,
        },
    });
    assert!(
        call_sites(&records, &["callee".into()], None)[0]
            .arguments
            .iter()
            .all(|a| *a == ArgumentValue::Unknown)
    );
}

fn call_sites(
    records: &[FunctionRecord],
    targets: &[String],
    image: Option<&ImageSymbols>,
) -> Vec<CallSite> {
    let memory = oer_analysis_riscv_model::WorkingMemory::new(8192)
        .expect("navigation fixture should succeed");
    let facts = oer_analysis_riscv_model::FunctionIndex::new(
        records,
        oer_analysis_riscv_model::RecordPolicy::Dictionary,
        &memory,
        &mut || Ok(()),
    )
    .expect("navigation fixture should succeed");
    let entries = image
        .map(|image| image.starts_named(targets))
        .unwrap_or_default()
        .into_iter()
        .map(|(address, name)| crate::references::Target {
            address,
            input: 0,
            name: name.as_bytes().to_vec(),
        });
    let targets_index = crate::references::Targets::new(entries, &memory, &mut || Ok(())).unwrap();
    super::call_sites(&facts, targets, &targets_index, &mut || Ok(()))
        .expect("navigation fixture should succeed")
}

#[test]
fn call_argument_lookup_errors_reach_the_caller_after_any_checkpoint() {
    use oer_analysis_riscv_model::{Error, ErrorCode, FunctionIndex, RecordPolicy, WorkingMemory};
    let records = [
        reference(0, "callee", 1, ReferenceKind::Call),
        inputs(
            4,
            &[(
                10,
                AbstractValue::Symbol {
                    symbol: symbol(2),
                    addend: 0,
                },
            )],
        ),
        reference(0, "data", 2, ReferenceKind::Address),
    ];
    let memory = WorkingMemory::new(8192).expect("navigation fixture should succeed");
    let facts = FunctionIndex::new(&records, RecordPolicy::Dictionary, &memory, &mut || Ok(()))
        .expect("navigation fixture should succeed");
    let targets = ["callee".into()];
    let targets_index = crate::references::Targets::new([], &memory, &mut || Ok(())).unwrap();
    let mut checkpoints = 0;
    assert_eq!(
        super::call_sites(&facts, &targets, &targets_index, &mut || {
            checkpoints += 1;
            Ok(())
        })
        .expect("navigation fixture should succeed")
        .len(),
        1
    );
    for fail_at in 0..checkpoints {
        for code in [
            ErrorCode::Cancelled,
            ErrorCode::TimedOut,
            ErrorCode::ResourceLimited,
            ErrorCode::Integrity,
        ] {
            let mut remaining = fail_at;
            let error = super::call_sites(&facts, &targets, &targets_index, &mut || {
                if remaining == 0 {
                    return Err(Error::new(code, "stop"));
                }
                remaining -= 1;
                Ok(())
            })
            .unwrap_err();
            assert_eq!(error, Error::new(code, "stop"));
        }
    }
    drop(facts);
    assert_eq!(memory.used(), 0);
}

#[test]
fn selected_targets_keep_their_own_names_and_unknown_transfers_stay_absent() {
    let image = ImageSymbols::new([
        (0x100, 4, "image_target".to_string()),
        (0x200, 4, "other_target".to_string()),
    ]);
    let records = [
        reference(0x20, "callee", 1, ReferenceKind::Branch),
        reference(0x20, "callee", 1, ReferenceKind::Branch),
        reference(0x10, "callee", 1, ReferenceKind::Call),
        reference(0, "last_name", 1, ReferenceKind::Address),
        FunctionRecord::Transfer {
            offset: 0x30,
            target: AbstractValue::Unknown,
            call: true,
        },
        FunctionRecord::Transfer {
            offset: 0x40,
            target: AbstractValue::ImageAddress { address: 0x100 },
            call: false,
        },
    ];
    let mut records = records.to_vec();
    records.push(FunctionRecord::Transfer {
        offset: 0x40,
        target: AbstractValue::ImageAddress { address: 0x200 },
        call: true,
    });
    let sites = call_sites(
        &records,
        &[
            "callee".into(),
            "image_target".into(),
            "other_target".into(),
        ],
        Some(&image),
    );
    assert_eq!(
        sites.iter().map(|site| site.offset).collect::<Vec<_>>(),
        [0x14, 0x20, 0x40, 0x40]
    );
    assert_eq!(sites[0].target, b"callee");
    assert_eq!(sites[1].target, b"callee");
    assert_eq!(sites[2].target, b"image_target");
    assert_eq!(sites[3].target, b"other_target");
    assert!(sites.iter().all(|site| site.arguments.is_empty()));
}
