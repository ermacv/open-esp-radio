use std::sync::Arc;

use oer_riscv_model::{
    AbstractValue, ArtifactId, Expression, FunctionRecord, FunctionRelocation, ObjectId,
    ObjectLocation, ReferenceKind, ReferenceTarget, SymbolDefinition, SymbolId, SymbolTableKind,
    ValueAlternative, ValueAlternatives,
};

use super::{ArgumentValue, CallSite, call_sites};
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
