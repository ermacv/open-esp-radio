use super::*;

// Both paths use r6 as the actual initial value, which can differ from a0.
pub(super) fn joined_code(pointer: u32, delta: i32) -> Vec<u32> {
    vec![
        addi(6, 10, delta),
        andi(14, 6, 3),
        branch(14, 0, 20, 1),
        addi(pointer, 6, 0),
        jal(0, 4),
        addi(7, pointer, 0), // checkpoint; also test a subsequent copy
        RET,
        addi(pointer, 6, 0),
        jal(0, 20),
        (lw(14, 11, 0) & !(7 << 12)) | (4 << 12),
        addi(pointer, pointer, 1),
        addi(11, 11, 1),
        store(14, pointer, -1, 0),
        andi(14, pointer, 3),
        branch(14, 0, -20, 1),
        jal(0, -40),
    ]
}
fn aligned_at(
    records: &[FunctionRecord],
    point: ValuePoint,
    register: u8,
) -> Option<&AlignedValue> {
    let mut found = records.iter().filter_map(|r| match r {
        FunctionRecord::AlignedValue { fact }
            if fact.point == point && fact.register == register =>
        {
            Some(fact.as_ref())
        }
        _ => None,
    });
    let first = found.next();
    assert!(found.next().is_none(), "duplicate equality at {point:?}");
    first
}
fn assert_normal_form(records: &[FunctionRecord], fact: &AlignedValue) {
    let memory = WorkingMemory::new(1024 * 1024).unwrap();
    let index = index(records, &memory);
    let Some(Expression::Integer {
        op: IntegerOp::And,
        left: sum,
        right,
    }) = index.value_expression(&fact.value)
    else {
        panic!("not a normalized mask expression: {fact:?}");
    };
    assert_eq!(
        *right,
        AbstractValue::Constant {
            value: !u32::from(fact.proof.mask)
        }
    );
    let Some(Expression::Integer {
        op: IntegerOp::Add,
        left,
        right,
    }) = index.value_expression(sum)
    else {
        panic!("not a wrapping addition");
    };
    assert_eq!(*left, fact.initial);
    assert_eq!(
        *right,
        AbstractValue::Constant {
            value: fact.proof.mask.into()
        }
    );
}

#[test]
fn loop_exit_and_both_merge_inputs_share_one_machine_witness_and_expression() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    for (entry, pointer, delta) in [(0x3000, 15, 0), (0x8290, 28, 12), (0x700100, 9, -3)] {
        let records = facts(&joined_code(pointer, delta), entry, &memory);
        let at = |index: u64| ValuePoint::Entry {
            offset: entry + 4 * index,
        };
        let edge = |from: u64, to: u64| ValuePoint::Edge {
            from: entry + 4 * from,
            target: entry + 4 * to,
        };
        let merged =
            aligned_at(&records, at(5), pointer as u8).expect("all incoming paths prove align-up");
        assert_eq!(merged.owner, entry);
        assert_eq!(merged.origin, AlignmentValueOrigin::AllIncoming);
        assert_normal_form(&records, merged);
        for (point, register, origin) in [
            (edge(14, 15), pointer as u8, AlignmentValueOrigin::LoopExit),
            (
                edge(15, 5),
                pointer as u8,
                AlignmentValueOrigin::Propagation,
            ),
            (edge(4, 5), pointer as u8, AlignmentValueOrigin::Propagation),
            (edge(2, 3), 6, AlignmentValueOrigin::ZeroResidue),
            (edge(5, 6), 7, AlignmentValueOrigin::Propagation),
        ] {
            let proof = aligned_at(&records, point, register).unwrap();
            assert_eq!(proof.initial, merged.initial);
            assert_eq!(proof.value, merged.value);
            assert_eq!(proof.proof, merged.proof);
            assert_eq!(proof.origin, origin);
        }
        let index = index(&records, &memory);
        if delta == 0 {
            assert!(matches!(
                index.value_expression(&merged.initial),
                Some(Expression::EntryRegister { register: 10 })
            ));
        } else {
            assert!(
                matches!(index.value_expression(&merged.initial), Some(Expression::Integer {
                op: IntegerOp::Add, right: AbstractValue::Constant { value }, ..
            }) if *value == delta as u32)
            );
        }
        let writes = summary(&records, entry, &memory);
        assert_eq!(writes.writes.len(), 3);
        assert!(
            writes
                .writes
                .iter()
                .all(|w| w.proof == WriteProof::AlignmentLoop(merged.proof))
        );
        // The assertion belongs to control flow, not an instruction write.
        assert!(!records.iter().any(|r| matches!(r, FunctionRecord::Value {
            offset, register, ..
        } if *offset == entry + 4 * 14 && *register == pointer as u8)));
    }
}

#[test]
fn exit_alignment_covers_every_residue_zero_iterations_and_rv32_wrap() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    for initial in [
        0u32, 1, 2, 3, 0xfffffffc, 0xfffffffd, 0xfffffffe, 0xffffffff,
    ] {
        let mut words = vec![
            (initial.wrapping_add(0x800) & 0xfffff000) | (10 << 7) | 0x37,
            addi(10, 10, ((initial << 20) as i32) >> 20),
        ];
        words.extend(joined_code(15, 0));
        let records = facts(&words, 0x1000, &memory);
        let merged = aligned_at(&records, ValuePoint::Entry { offset: 0x101c }, 15).unwrap();
        let result = initial.wrapping_add(3) & !3;
        assert_eq!(merged.initial, AbstractValue::Constant { value: initial });
        assert_eq!(merged.value, AbstractValue::Constant { value: result });
        assert_eq!(result & 3, 0);
        assert!(result.wrapping_sub(initial) <= 3);
        assert_eq!(result.wrapping_sub(initial), (4 - (initial & 3)) & 3);
        assert_eq!(summary(&records, 0x1000, &memory).writes.len(), 3);
    }
}

#[test]
fn failed_cycle_checks_supply_neither_exit_value_nor_store_bound() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    for (at, word) in [
        (8, jal(0, 4)),            // bypass test before first iteration
        (8, branch(12, 0, 20, 1)), // an extra entry through the body
        (10, addi(15, 15, 2)),
        (11, addi(15, 15, 1)),
        (13, andi(14, 15, 2)),
        (14, branch(14, 0, -20, 0)),
        (12, store(14, 15, -1, 2)),
    ] {
        let mut words = joined_code(15, 0);
        words[at] = word;
        let records = facts(&words, 0x1000, &memory);
        assert!(
            !records.iter().any(|r| matches!(
                r,
                FunctionRecord::AlignedValue { .. } | FunctionRecord::StoreAddressRange { .. }
            )),
            "{at}"
        );
        assert!(
            summary(&records, 0x1000, &memory)
                .barriers
                .iter()
                .any(|b| b.kind == WriteBarrierKind::UnknownStore)
        );
    }
}

#[test]
fn an_unproved_or_incompatible_merge_input_loses_the_equality_but_keeps_the_loop_proof() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let mut variants = Vec::new();
    let mut changed = joined_code(15, 0);
    changed[3] = addi(15, 6, 4); // aligned path computes a different value
    variants.push((changed, 0x1014));
    let mut changed = joined_code(15, 0);
    changed[2] = branch(12, 0, 20, 1); // no guard fact for the bypass path
    variants.push((changed, 0x1014));
    let mut changed = joined_code(15, 0);
    changed[15] = addi(15, 15, 4); // pointer mutation after the proved exit
    changed.push(jal(0, -44));
    variants.push((changed, 0x1014));
    let mut changed = vec![branch(12, 0, 24, 1)]; // third, unproved incoming path
    changed.extend(joined_code(15, 0));
    variants.push((changed, 0x1018));
    for (words, checkpoint) in variants {
        let records = facts(&words, 0x1000, &memory);
        assert!(aligned_at(&records, ValuePoint::Entry { offset: checkpoint }, 15).is_none());
        assert!(
            records
                .iter()
                .any(|r| matches!(r, FunctionRecord::AlignedValue { fact }
            if fact.origin == AlignmentValueOrigin::LoopExit))
        );
        assert_eq!(summary(&records, 0x1000, &memory).writes.len(), 3);
    }
}

#[test]
fn copies_after_exit_use_the_shared_effect_semantics_and_retain_ownership() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let mut words = joined_code(15, 12);
    words[15] = addi(8, 15, 0);
    words.extend([addi(15, 0, 0), addi(15, 8, 0), jal(0, -52)]);
    let records = facts(&words, 0x1000, &memory);
    let merged = aligned_at(&records, ValuePoint::Entry { offset: 0x1014 }, 15).unwrap();
    assert_eq!(merged.owner, 0x1000);
    assert_normal_form(&records, merged);
    assert_eq!(
        aligned_at(&records, ValuePoint::Entry { offset: 0x1044 }, 8)
            .unwrap()
            .value,
        merged.value
    );
}

#[test]
fn an_open_local_graph_cannot_supply_control_point_equalities() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let mut words = vec![branch(12, 0, 68, 1)];
    words.extend(joined_code(15, 0));
    words.push(0x00068067); // unknown jalr x0, a3
    let records = facts(&words, 0x1000, &memory);
    assert!(
        !records
            .iter()
            .any(|r| matches!(r, FunctionRecord::AlignedValue { .. }))
    );
    assert!(!summary(&records, 0x1000, &memory).closed_graph);
}

#[test]
fn two_store_footprints_share_one_exit_equality_propagation() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let mut words = joined_code(15, 0);
    words.insert(12, store(14, 15, -1, 0));
    words[8] = jal(0, 24);
    words[15] = branch(14, 0, -24, 1);
    words[16] = jal(0, -44);
    let records = facts(&words, 0x1000, &memory);
    let merged = aligned_at(&records, ValuePoint::Entry { offset: 0x1014 }, 15).unwrap();
    let writes = summary(&records, 0x1000, &memory);
    assert_eq!(writes.writes.len(), 6);
    assert!(
        writes
            .writes
            .iter()
            .all(|w| w.proof == WriteProof::AlignmentLoop(merged.proof))
    );
    assert_eq!(
        records
            .iter()
            .filter(|r| matches!(r, FunctionRecord::AlignedValue { fact }
        if fact.origin == AlignmentValueOrigin::LoopExit && fact.register == 15))
            .count(),
        1
    );
}

#[test]
fn undecoded_reachable_path_cannot_supply_control_point_equalities() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let mut words = vec![branch(12, 0, 68, 1)];
    words.extend(joined_code(15, 0));
    words.push(u32::MAX); // reachable instruction with unknown decoding/control flow
    let records = facts(&words, 0x1000, &memory);
    assert!(records.iter().any(|r| matches!(
        r,
        FunctionRecord::Gap {
            reason: GapReason::UnsupportedInstruction,
            ..
        }
    )));
    assert!(!summary(&records, 0x1000, &memory).closed_graph);
    assert!(
        !records
            .iter()
            .any(|r| matches!(r, FunctionRecord::AlignedValue { .. }))
    );
    let merged = aligned_at(&records, ValuePoint::Entry { offset: 0x1018 }, 15);
    assert!(
        merged.is_none(),
        "unclosed graph supplied an equality: {merged:?}"
    );
}

#[test]
fn unvisited_undecodable_bytes_preserve_the_closed_graph_equality() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let mut words = joined_code(15, 0);
    words.push(u32::MAX); // no incoming edge: these bytes are never decoded
    let records = facts(&words, 0x1000, &memory);
    assert!(records.iter().any(|r| matches!(
        r,
        FunctionRecord::Gap {
            reason: GapReason::Unvisited,
            ..
        }
    )));
    assert!(!records.iter().any(|r| matches!(
        r,
        FunctionRecord::Gap {
            reason: GapReason::UnsupportedInstruction,
            ..
        }
    )));
    assert!(summary(&records, 0x1000, &memory).closed_graph);
    assert!(aligned_at(&records, ValuePoint::Entry { offset: 0x1014 }, 15).is_some());
}
