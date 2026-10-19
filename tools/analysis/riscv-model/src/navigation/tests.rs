use super::*;
use std::sync::Arc;

fn expr(id: u32, expression: Expression) -> FunctionRecord {
    FunctionRecord::Expression {
        id,
        offset: 0,
        expression,
    }
}
fn entry(id: u32, register: u8) -> FunctionRecord {
    expr(id, Expression::EntryRegister { register })
}
fn load(id: u32, operand: u32) -> FunctionRecord {
    expr(
        id,
        Expression::Load {
            address: AbstractValue::Expression { id: operand },
            width: 4,
            signed: false,
        },
    )
}
fn inputs(offset: u64, value: u32) -> FunctionRecord {
    FunctionRecord::CallInputs {
        offset,
        registers: vec![AbstractValue::Constant { value }],
    }
}
fn jump(offset: u64, value: u32) -> FunctionRecord {
    FunctionRecord::Transfer {
        offset,
        target: AbstractValue::Constant { value },
        call: false,
    }
}
fn symbol(index: u64) -> SymbolId {
    SymbolId {
        object: ObjectId {
            artifact: ArtifactId::of_bytes(b"navigation"),
            location: ObjectLocation::Standalone,
        },
        table: SymbolTableKind::Static,
        table_section: 1,
        index,
    }
}
fn named(name: &[u8]) -> FunctionRecord {
    let target = Arc::new(ReferenceTarget {
        symbol: symbol(1),
        name: name.to_vec(),
        definition: SymbolDefinition::Undefined,
        binding: 1,
        section: None,
        offset: 0,
        symbol_type: 0,
    });
    FunctionRecord::Reference {
        raw: Box::new(FunctionRelocation {
            section: 1,
            index: 0,
            offset: 0,
            relocation_type: 0,
            addend: None,
            target: target.clone(),
        }),
        target,
        reference_kind: ReferenceKind::Address,
        addend: None,
        paired: None,
        known: false,
    }
}
fn instruction(offset: u64) -> FunctionRecord {
    FunctionRecord::Instruction {
        offset,
        bytes: vec![],
        decoded: DecodedOp {
            length: 4,
            text: String::new(),
            flow: InstructionFlow::Next,
        },
    }
}
fn access(offset: u64) -> FunctionRecord {
    FunctionRecord::MemoryAccess {
        offset,
        access: MemoryKind::Store,
        width: 4,
        address: AbstractValue::Expression { id: 99 },
        value: None,
        relocation: None,
    }
}

#[test]
fn canonical_graph_and_duplicate_contracts_are_preserved() {
    let memory = WorkingMemory::new(8192).expect("navigation fixture should succeed");
    for records in [
        vec![entry(1, 10)],
        vec![entry(u32::MAX, 10)],
        vec![entry(0, 10), entry(0, 11)],
        vec![load(0, 0)],
        vec![load(0, 1), entry(1, 10)],
        vec![expr(
            0,
            Expression::Integer {
                op: IntegerOp::Add,
                left: AbstractValue::Unknown,
                right: AbstractValue::Expression { id: 1 },
            },
        )],
        vec![instruction(4), instruction(4)],
        vec![inputs(4, 1), inputs(4, 1)],
        vec![jump(4, 1), jump(4, 2)],
        vec![named(b"first"), named(b"last")],
    ] {
        assert!(
            matches!(FunctionIndex::new(&records, RecordPolicy::Canonical, &memory, &mut || Ok(())), Err(e) if e.code == ErrorCode::Integrity)
        );
        assert_eq!(memory.used(), 0);
        // The same inputs were valid dictionary records before the move.
        drop(
            FunctionIndex::new(&records, RecordPolicy::Dictionary, &memory, &mut || Ok(()))
                .expect("navigation fixture should succeed"),
        );
        assert_eq!(memory.used(), 0);
    }
}

#[test]
fn dictionary_uses_exact_sparse_ids_and_last_source_definitions() {
    let records = [
        load(u32::MAX, 7),
        entry(7, 12),
        entry(7, 10),
        named(b"first"),
        inputs(8, 1),
        jump(8, 2),
        named(b"last\xff"),
        inputs(8, 3),
        jump(8, 4),
    ];
    let memory = WorkingMemory::new(8192).expect("navigation fixture should succeed");
    let index = FunctionIndex::new(&records, RecordPolicy::Dictionary, &memory, &mut || Ok(()))
        .expect("navigation fixture should succeed");
    assert_eq!(
        index
            .expression(7, &mut || Ok(()))
            .expect("navigation fixture should succeed"),
        Some(&Expression::EntryRegister { register: 10 })
    );
    assert!(matches!(
        index
            .expression(u32::MAX, &mut || Ok(()))
            .expect("navigation fixture should succeed"),
        Some(Expression::Load { .. })
    ));
    assert!(
        index
            .expression(0, &mut || Ok(()))
            .expect("navigation fixture should succeed")
            .is_none()
    );
    assert!(
        index
            .expression(6, &mut || Ok(()))
            .expect("navigation fixture should succeed")
            .is_none()
    );
    assert!(
        index
            .expression(8, &mut || Ok(()))
            .expect("navigation fixture should succeed")
            .is_none()
    );
    assert!(index.value_expression(&AbstractValue::Unknown).is_none());
    assert_eq!(
        index
            .reference(&symbol(1), &mut || Ok(()))
            .expect("navigation fixture should succeed")
            .expect("navigation fixture should succeed")
            .name,
        b"last\xff"
    );
    assert!(
        index
            .reference(&symbol(0), &mut || Ok(()))
            .expect("navigation fixture should succeed")
            .is_none()
    );
    assert!(
        index
            .reference(&symbol(2), &mut || Ok(()))
            .expect("navigation fixture should succeed")
            .is_none()
    );
    let input = index
        .call_inputs(8, &mut || Ok(()))
        .expect("navigation fixture should succeed")
        .expect("navigation fixture should succeed");
    assert_eq!(input.record, 7);
    assert_eq!(input.registers, &[AbstractValue::Constant { value: 3 }]);
    let target = index
        .transfer(8, &mut || Ok(()))
        .expect("navigation fixture should succeed")
        .expect("navigation fixture should succeed");
    assert_eq!(target.record, 8);
    assert_eq!(target.target, &AbstractValue::Constant { value: 4 });
    assert!(!target.call);
    for offset in [0, 7, 9, u64::MAX] {
        assert!(
            index
                .call_inputs(offset, &mut || Ok(()))
                .expect("navigation fixture should succeed")
                .is_none()
        );
        assert!(
            index
                .transfer(offset, &mut || Ok(()))
                .expect("navigation fixture should succeed")
                .is_none()
        );
    }
    assert!(memory.used() > 0);
    drop(index);
    assert_eq!(memory.used(), 0);
}

#[test]
fn incomplete_records_equal_references_and_source_access_order_are_valid() {
    let records = [
        access(20),
        inputs(12, 1),
        named(b"same"),
        entry(0, 10),
        access(4),
        named(b"same"),
        load(1, 0),
        jump(4, 9),
    ];
    let memory = WorkingMemory::new(8192).expect("navigation fixture should succeed");
    for policy in [RecordPolicy::Canonical, RecordPolicy::Dictionary] {
        let index = FunctionIndex::new(&records, policy, &memory, &mut || Ok(()))
            .expect("navigation fixture should succeed");
        assert_eq!(
            index
                .expression(0, &mut || Ok(()))
                .expect("navigation fixture should succeed"),
            Some(&Expression::EntryRegister { register: 10 })
        );
        assert!(
            index
                .expression(99, &mut || Ok(()))
                .expect("navigation fixture should succeed")
                .is_none()
        );
        assert_eq!(index.records(), &records);
        let mut seen = Vec::new();
        index
            .accesses(&mut || Ok(()), &mut |access, _| {
                seen.push((access.record, access.offset));
                Ok(())
            })
            .expect("navigation fixture should succeed");
        assert_eq!(seen, [(0, 20), (4, 4)]);
        // No instruction or matching transfer is required to retain an input.
        assert_eq!(
            index
                .call_inputs(12, &mut || Ok(()))
                .expect("navigation fixture should succeed")
                .expect("navigation fixture should succeed")
                .registers
                .len(),
            1
        );
        assert!(
            index
                .call_inputs(4, &mut || Ok(()))
                .expect("navigation fixture should succeed")
                .is_none()
        );
        drop(index);
        assert_eq!(memory.used(), 0);
    }
    for policy in [RecordPolicy::Canonical, RecordPolicy::Dictionary] {
        let index = FunctionIndex::new(&[], policy, &memory, &mut || Ok(()))
            .expect("navigation fixture should succeed");
        assert!(
            index
                .expression(0, &mut || Ok(()))
                .expect("navigation fixture should succeed")
                .is_none()
        );
        assert!(
            index
                .reference(&symbol(1), &mut || Ok(()))
                .expect("navigation fixture should succeed")
                .is_none()
        );
        assert!(
            index
                .call_inputs(0, &mut || Ok(()))
                .expect("navigation fixture should succeed")
                .is_none()
        );
        assert!(
            index
                .transfer(0, &mut || Ok(()))
                .expect("navigation fixture should succeed")
                .is_none()
        );
    }
    assert_eq!(memory.used(), 0);
}

struct Budget(u64);
impl RunControl for Budget {
    fn checkpoint(&mut self, units: u64) -> Result<()> {
        self.0 = self
            .0
            .checked_sub(units)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "work exhausted"))?;
        Ok(())
    }
}

#[test]
fn cancellation_work_and_capacity_errors_release_all_preparation() {
    let records = [
        instruction(0),
        entry(0, 10),
        named(b"name"),
        inputs(0, 1),
        jump(0, 2),
    ];
    for policy in [RecordPolicy::Canonical, RecordPolicy::Dictionary] {
        let memory = WorkingMemory::new(8192).expect("navigation fixture should succeed");
        let mut count = 0;
        drop(
            FunctionIndex::new(&records, policy, &memory, &mut || {
                count += 1;
                Ok(())
            })
            .expect("navigation fixture should succeed"),
        );
        for fail_at in 0..count {
            for code in [ErrorCode::Cancelled, ErrorCode::TimedOut] {
                let mut remaining = fail_at;
                let error = FunctionIndex::new(&records, policy, &memory, &mut || {
                    if remaining == 0 {
                        return Err(Error::new(code, "stop"));
                    }
                    remaining -= 1;
                    Ok(())
                })
                .err()
                .expect("navigation fixture should succeed");
                assert_eq!(error, Error::new(code, "stop"));
                assert_eq!(memory.used(), 0);
            }
        }
        let mut measured = Budget(u64::MAX);
        drop(
            FunctionIndex::new(&records, policy, &memory, &mut measured)
                .expect("navigation fixture should succeed"),
        );
        let work = u64::MAX - measured.0;
        assert!(
            matches!(FunctionIndex::new(&records, policy, &memory, &mut Budget(work - 1)), Err(e) if e.code == ErrorCode::ResourceLimited)
        );
        assert_eq!(memory.used(), 0);
        let peak = memory.peak();
        for limit in [1, 64, peak - 1] {
            let limited = WorkingMemory::new(limit).expect("navigation fixture should succeed");
            let error = FunctionIndex::new(&records, policy, &limited, &mut || Ok(()))
                .err()
                .expect("navigation fixture should succeed");
            assert_eq!(error.code, ErrorCode::ResourceLimited);
            assert!(error.memory.is_some());
            assert_eq!(limited.used(), 0);
        }
        let exact = WorkingMemory::new(peak).expect("navigation fixture should succeed");
        drop(
            FunctionIndex::new(&records, policy, &exact, &mut Budget(work))
                .expect("navigation fixture should succeed"),
        );
        assert_eq!(exact.used(), 0);
    }
}

#[test]
fn lookup_and_delivery_errors_are_never_an_empty_result() {
    let records = [
        entry(0, 10),
        named(b"name"),
        inputs(0, 1),
        jump(0, 2),
        access(0),
    ];
    let memory = WorkingMemory::new(8192).expect("navigation fixture should succeed");
    let index = FunctionIndex::new(&records, RecordPolicy::Canonical, &memory, &mut || Ok(()))
        .expect("navigation fixture should succeed");
    for code in [
        ErrorCode::Cancelled,
        ErrorCode::TimedOut,
        ErrorCode::ResourceLimited,
        ErrorCode::Integrity,
    ] {
        let stop = || Err(Error::new(code, "stop"));
        assert_eq!(
            index
                .expression(0, &mut { stop })
                .err()
                .expect("navigation should fail")
                .code,
            code
        );
        assert_eq!(
            index
                .reference(&symbol(1), &mut { stop })
                .err()
                .expect("navigation fixture should succeed")
                .code,
            code
        );
        assert_eq!(
            index
                .call_inputs(0, &mut { stop })
                .err()
                .expect("navigation should fail")
                .code,
            code
        );
        assert_eq!(
            index
                .transfer(0, &mut { stop })
                .err()
                .expect("navigation should fail")
                .code,
            code
        );
        assert_eq!(
            index
                .accesses(&mut { stop }, &mut |_, _| panic!("cancel before delivery"))
                .unwrap_err()
                .code,
            code
        );
        assert_eq!(
            index
                .accesses(&mut || Ok(()), &mut |_, _| stop())
                .unwrap_err()
                .code,
            code
        );
    }
    drop(index);
    assert_eq!(memory.used(), 0);
}
