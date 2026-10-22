use std::sync::Arc;

use oer_analysis_riscv_model::{
    AbstractValue, ArtifactId, Expression, FunctionRecord, FunctionRelocation, IntegerOp,
    MemoryKind, ObjectId, ObjectLocation, ReferenceKind, ReferenceTarget, SymbolDefinition,
    SymbolId, SymbolTableKind,
};

use super::{FieldRoot, unknown_addresses};

fn symbol() -> SymbolId {
    SymbolId {
        object: ObjectId {
            artifact: ArtifactId::of_bytes(b"library"),
            location: ObjectLocation::ArchiveMember { ordinal: 40 },
        },
        table: SymbolTableKind::Static,
        table_section: 309,
        index: 7,
    }
}

fn expression(id: u32, expression: Expression) -> FunctionRecord {
    FunctionRecord::Expression {
        id,
        offset: 0,
        expression,
    }
}

fn add(base: AbstractValue, constant: u32) -> Expression {
    Expression::Integer {
        op: IntegerOp::Add,
        left: base,
        right: AbstractValue::Constant { value: constant },
    }
}

fn store(offset: u64, width: u8, id: u32) -> FunctionRecord {
    FunctionRecord::MemoryAccess {
        offset,
        access: MemoryKind::Store,
        width,
        address: AbstractValue::Expression { id },
        value: None,
        relocation: None,
    }
}

/// `a5 = *(g_ic + 16)`, then `sb 148(a5)`, and the same byte again through
/// `a0 = a5 + 24; sb 124(a0)`; a store to an absolute address beside them.
fn records() -> Vec<FunctionRecord> {
    let g_ic = AbstractValue::Symbol {
        symbol: symbol(),
        addend: 0,
    };
    let target = Arc::new(ReferenceTarget {
        binding: 1,
        definition: SymbolDefinition::Undefined,
        symbol: symbol(),
        name: b"g_ic".to_vec(),
        section: None,
        offset: 0,
        symbol_type: 1,
    });
    vec![
        FunctionRecord::Reference {
            raw: Box::new(FunctionRelocation {
                section: 1,
                index: 0,
                offset: 4,
                relocation_type: 26,
                addend: Some(0),
                target: target.clone(),
            }),
            reference_kind: ReferenceKind::Address,
            target,
            addend: Some(0),
            paired: None,
            known: true,
        },
        expression(1, add(g_ic, 16)),
        expression(
            2,
            Expression::Load {
                address: AbstractValue::Expression { id: 1 },
                width: 4,
                signed: false,
            },
        ),
        expression(3, add(AbstractValue::Expression { id: 2 }, 148)),
        store(54, 1, 3),
        expression(4, add(AbstractValue::Expression { id: 2 }, 24)),
        expression(5, add(AbstractValue::Expression { id: 4 }, 124)),
        store(72, 1, 5),
        expression(6, add(AbstractValue::Expression { id: 2 }, 148)),
        store(80, 4, 6),
        FunctionRecord::MemoryAccess {
            offset: 90,
            access: MemoryKind::Store,
            width: 1,
            address: AbstractValue::Constant { value: 148 },
            value: None,
            relocation: None,
        },
    ]
}

#[test]
fn accesses_fold_into_a_named_root_and_a_path() {
    let accesses = field_accesses(&records(), 148, Some(1));
    // Both byte stores land on the same field; the word store has another
    // width and the absolute store is no field.
    assert_eq!(accesses.len(), 2);
    for (access, at) in accesses.iter().zip([54, 72]) {
        assert_eq!(access.offset, at);
        assert_eq!(access.path, [16, 148]);
        assert_eq!(
            access.root,
            FieldRoot::Symbol {
                symbol: symbol(),
                name: Some("g_ic".into()),
            }
        );
    }
    assert_eq!(field_accesses(&records(), 148, None).len(), 3);
    assert!(field_accesses(&records(), 124, None).is_empty());
}

#[test]
fn a_negative_constant_folds_as_a_signed_displacement() {
    let records = vec![
        expression(
            1,
            add(AbstractValue::EntryStack { offset: 0 }, (-8_i32) as u32),
        ),
        store(4, 4, 1),
    ];
    let accesses = field_accesses(&records, -8, None);
    assert_eq!(accesses.len(), 1);
    assert_eq!(accesses[0].root, FieldRoot::EntryStack);
    assert_eq!(accesses[0].path, [-8]);
}

#[test]
fn an_unresolved_base_keeps_its_exact_displacements() {
    // `sb 148(a5)` where the analysis lost what `a5` holds.
    let records = vec![
        expression(1, add(AbstractValue::Unknown, 148)),
        store(54, 1, 1),
        FunctionRecord::MemoryAccess {
            offset: 60,
            access: MemoryKind::Load,
            width: 1,
            address: AbstractValue::Unknown,
            value: None,
            relocation: None,
        },
    ];
    let accesses = field_accesses(&records, 148, Some(1));
    assert_eq!(accesses.len(), 1);
    assert_eq!(accesses[0].offset, 54);
    assert_eq!(accesses[0].root, FieldRoot::Unknown);
    assert_eq!(accesses[0].path, [148]);
    assert_eq!(unknown_addresses(&records), 1);
}

#[test]
fn dictionary_ids_forward_definitions_and_last_definition_are_preserved() {
    let records = vec![
        store(20, 4, u32::MAX),
        expression(
            u32::MAX,
            add(AbstractValue::Expression { id: 7 }, (-8_i32) as u32),
        ),
        expression(7, Expression::EntryRegister { register: 12 }),
        expression(7, Expression::EntryRegister { register: 10 }),
        store(4, 4, u32::MAX),
    ];
    let accesses = field_accesses(&records, -8, None);
    assert_eq!(
        accesses.iter().map(|a| a.offset).collect::<Vec<_>>(),
        [20, 4]
    );
    for access in accesses {
        assert_eq!(access.root, FieldRoot::EntryRegister { register: 10 });
        assert_eq!(access.path, [-8]);
    }
}

#[test]
fn missing_expressions_stay_unknown_and_cycles_do_not_match() {
    let missing = [
        expression(3, add(AbstractValue::Expression { id: 99 }, 148)),
        store(0, 1, 3),
    ];
    let accesses = field_accesses(&missing, 148, None);
    assert_eq!(accesses.len(), 1);
    assert_eq!(accesses[0].root, FieldRoot::Unknown);
    assert_eq!(accesses[0].path, [148]);
    let cycle = [
        expression(3, add(AbstractValue::Expression { id: 3 }, 148)),
        store(0, 1, 3),
    ];
    assert!(field_accesses(&cycle, 148, None).is_empty());
}

#[test]
fn late_names_use_the_last_reference_and_missing_names_stay_absent() {
    let mut records = records();
    let FunctionRecord::Reference { target, .. } = &records[0] else {
        unreachable!("fixture contains a reference record")
    };
    let mut late = target.as_ref().clone();
    late.name = b"last\xff".to_vec();
    let mut reference = records[0].clone();
    let FunctionRecord::Reference { target, .. } = &mut reference else {
        unreachable!("fixture contains a reference record")
    };
    *target = Arc::new(late);
    records.push(reference);
    assert_eq!(
        field_accesses(&records, 148, None)[0].root,
        FieldRoot::Symbol {
            symbol: symbol(),
            name: Some("last\u{fffd}".into())
        }
    );
    records.retain(|r| !matches!(r, FunctionRecord::Reference { .. }));
    assert_eq!(
        field_accesses(&records, 148, None)[0].root,
        FieldRoot::Symbol {
            symbol: symbol(),
            name: None
        }
    );
}

fn field_accesses(
    records: &[FunctionRecord],
    offset: i64,
    width: Option<u8>,
) -> Vec<super::FieldAccess> {
    let memory = oer_analysis_riscv_model::WorkingMemory::new(8192)
        .expect("navigation fixture should succeed");
    let facts = oer_analysis_riscv_model::FunctionIndex::new(
        records,
        oer_analysis_riscv_model::RecordPolicy::Dictionary,
        &memory,
        &mut || Ok(()),
    )
    .expect("navigation fixture should succeed");
    super::field_accesses(&facts, offset, width, &mut || Ok(()))
        .expect("navigation fixture should succeed")
}

#[test]
fn field_errors_during_folding_and_name_lookup_reach_the_caller() {
    use oer_analysis_riscv_model::{Error, ErrorCode, FunctionIndex, RecordPolicy, WorkingMemory};
    let records = records();
    let memory = WorkingMemory::new(8192).expect("navigation fixture should succeed");
    let facts = FunctionIndex::new(&records, RecordPolicy::Dictionary, &memory, &mut || Ok(()))
        .expect("navigation fixture should succeed");
    let mut checkpoints = 0;
    assert!(
        !super::field_accesses(&facts, 148, None, &mut || {
            checkpoints += 1;
            Ok(())
        })
        .expect("navigation fixture should succeed")
        .is_empty()
    );
    for fail_at in 0..checkpoints {
        for code in [
            ErrorCode::Cancelled,
            ErrorCode::TimedOut,
            ErrorCode::ResourceLimited,
            ErrorCode::Integrity,
        ] {
            let mut remaining = fail_at;
            let error = super::field_accesses(&facts, 148, None, &mut || {
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
