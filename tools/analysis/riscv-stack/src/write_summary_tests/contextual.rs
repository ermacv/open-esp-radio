use super::alignment::{andi, branch};
use super::*;

struct Fixture {
    words: Vec<u32>,
    guard: usize,
    bulk: Vec<usize>,
    back: usize,
}
fn add(dest: u32, left: u32, right: u32) -> u32 {
    (right << 20) | (left << 15) | (dest << 7) | 0x33
}
fn sub(dest: u32, left: u32, right: u32) -> u32 {
    add(dest, left, right) | (0x20 << 25)
}
fn fixture(test: u32, extra_entry: bool) -> Fixture {
    let mut w = vec![add(13, 10, 12), andi(14, 10, 3), 0, addi(15, 10, 0), 0];
    let merge = w.len();
    w.extend([andi(14, 13, !3), sub(16, 14, 15), addi(12, 0, 32)]);
    let guard = w.len();
    w.push(0);
    let extra = w.len();
    if extra_entry {
        w.push(0);
    }
    w.extend([store(11, 15, 0, 0), store(11, 15, 4, 2), RET]);
    let init = w.len();
    w.extend([addi(15, 10, 0), 0]);
    let body = w.len();
    w.extend([
        (lw(14, 11, 0) & !(7 << 12)) | (4 << 12),
        addi(15, 15, 1),
        addi(11, 11, 1),
        store(14, 15, -1, 0),
    ]);
    let align_guard = w.len();
    w.extend([
        andi(14, 15, 3),
        branch(14, 0, ((body as i32) - (align_guard as i32 + 1)) * 4, 1),
        jal(0, ((merge as i32) - (align_guard as i32 + 2)) * 4),
    ]);
    let bulk_entry = w.len();
    let mut bulk = Vec::new();
    for delta in 0..9 {
        bulk.push(w.len());
        w.push(store(11, 15, delta * 4, 2));
    }
    w.push(addi(15, 15, 36));
    let back = w.len();
    w.extend([
        branch(15, 14, ((bulk_entry as i32) - (back as i32)) * 4, 1),
        RET,
    ]);
    w[2] = branch(14, 0, ((init as i32) - 2) * 4, 1);
    w[4] = jal(0, ((merge as i32) - 4) * 4);
    w[init + 1] = jal(0, ((align_guard as i32) - (init as i32 + 1)) * 4);
    w[guard] = branch(12, 16, ((bulk_entry as i32) - (guard as i32)) * 4, test);
    if extra_entry {
        w[extra] = branch(17, 0, ((bulk_entry as i32) - (extra as i32)) * 4, 1);
    }
    Fixture {
        words: w,
        guard,
        bulk,
        back,
    }
}
fn caller(
    length: Option<u32>,
    destination: Option<u32>,
    target: u32,
    memory: &WorkingMemory,
) -> RecordBuffer<'_> {
    let mut words = Vec::new();
    for (register, value) in [(10, destination), (12, length)] {
        if let Some(value) = value {
            words.push((value.wrapping_add(0x800) & 0xfffff000) | (register << 7) | 0x37);
            words.push(addi(register, register, ((value << 20) as i32) >> 20));
        }
    }
    words.push(jal(1, target as i32 - (0x1000 + words.len() as i32 * 4)));
    words.push(RET);
    facts(&words, 0x1000, memory)
}
fn call_site(records: &[FunctionRecord]) -> u64 {
    records
        .iter()
        .find_map(|r| match r {
            FunctionRecord::Transfer {
                offset, call: true, ..
            } => Some(*offset),
            _ => None,
        })
        .unwrap()
}
fn specialized<'m>(
    f: &Fixture,
    entry: u64,
    context: &CallContext,
    memory: &'m WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<FunctionWrites<'m>> {
    let code: Vec<_> = f.words.iter().flat_map(|w| w.to_le_bytes()).collect();
    contextual_function_writes(
        FunctionInput {
            image: Some(&NoMemory),
            section: 1,
            extent: CodeRange {
                start: entry,
                length: code.len() as u64,
            },
            bytes: &code,
            relocations: &PreparedReferences::empty(),
            data_ranges: &[],
            jumps: &[],
            returns: &NoReturns,
            call_behavior: &UnknownCallBehavior,
        },
        &RiscvDecoder,
        context,
        &[],
        memory,
        control,
    )
}
#[test]
fn short_call_excludes_all_bulk_stores_only_under_proved_entry_length() {
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    let f = fixture(4, false);
    let generic_records = facts(&f.words, 0x3000, &memory);
    let generic = summary(&generic_records, 0x3000, &memory);
    assert_eq!(generic.writes.len(), 14);
    let trampoline_records = facts(&[jal(0, 0x1000)], 0x2000, &memory);
    let trampoline = summary_with_callees(&trampoline_records, 0x2000, &[&generic], &memory);
    let caller_records = caller(Some(20), None, 0x2000, &memory);
    let site = call_site(&caller_records);
    let caller_index = index(&caller_records, &memory);
    let context = call_context(
        &caller_index,
        0x1000,
        site,
        ABI,
        &trampoline,
        &memory,
        &mut || Ok(()),
    )
    .unwrap()
    .unwrap();
    assert_eq!(context.entry(), 0x3000);
    assert_eq!(context.arguments()[2], Some(20));
    let specialized = specialized(&f, 0x3000, &context, &memory, &mut || Ok(())).unwrap();
    assert_eq!(specialized.writes.as_ref(), generic.writes.as_ref());
    let proof = specialized.specialization().unwrap();
    let branch = proof
        .branches()
        .iter()
        .find(|b| b.site == 0x3000 + f.guard as u64 * 4)
        .unwrap();
    assert_eq!(&branch.left[..branch.left_count as usize], &[32]);
    assert_eq!(&branch.right[..branch.right_count as usize], &[16, 20]);
    assert!(!branch.taken);
    for bulk in &f.bulk {
        assert!(proof.unreachable().contains(&(0x3000 + *bulk as u64 * 4)));
    }
    assert!(proof.unreachable().contains(&(0x3000 + f.back as u64 * 4)));
    let trampoline = summary_with_callees(&trampoline_records, 0x2000, &[&specialized], &memory);
    let applied = call_writes(
        &caller_index,
        0x1000,
        site,
        ABI,
        &[&trampoline],
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    assert_eq!(applied.writes.len(), 5);
    assert_eq!(
        applied
            .writes
            .iter()
            .filter(|w| matches!(w.write.proof, WriteProof::AlignmentLoop(_)))
            .count(),
        3
    );
    assert!(
        !applied
            .preserves_stack_word(0x1000, -600, &mut || Ok(()))
            .unwrap()
    );
    for length in [None, Some(21), Some(128)] {
        let changed = caller(length, None, 0x2000, &memory);
        let applied = call_writes(
            &index(&changed, &memory),
            0x1000,
            call_site(&changed),
            ABI,
            &[&trampoline],
            &memory,
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(applied.writes.len(), 14);
    }
}

fn direct_context<'m>(
    f: &Fixture,
    length: u32,
    destination: Option<u32>,
    memory: &'m WorkingMemory,
) -> (CallContext, FunctionWrites<'m>) {
    let records = facts(&f.words, 0x3000, memory);
    let generic = summary(&records, 0x3000, memory);
    let caller = caller(Some(length), destination, 0x3000, memory);
    let context = call_context(
        &index(&caller, memory),
        0x1000,
        call_site(&caller),
        ABI,
        &generic,
        memory,
        &mut || Ok(()),
    )
    .unwrap()
    .unwrap();
    (context, generic)
}
#[test]
fn lengths_and_all_rv32_residues_agree_with_machine_arithmetic() {
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    for test in [4, 6] {
        // signed/unsigned LT use the instruction's actual predicate
        let f = fixture(test, false);
        for length in [
            0u32,
            1,
            3,
            4,
            16,
            20,
            31,
            32,
            33,
            36,
            37,
            64,
            128,
            0x80000000,
            u32::MAX,
        ] {
            let expected: Vec<_> = (0u32..4)
                .map(|r| (r.wrapping_add(length) & !3).wrapping_sub(r.wrapping_add(3) & !3))
                .collect();
            let reachable = expected.iter().any(|r| {
                if test == 4 {
                    32i32 < *r as i32
                } else {
                    32 < *r
                }
            });
            let (context, _) = direct_context(&f, length, None, &memory);
            let s = specialized(&f, 0x3000, &context, &memory, &mut || Ok(())).unwrap();
            let proof = s.specialization().unwrap();
            for site in &f.bulk {
                assert_eq!(
                    !proof.unreachable().contains(&(0x3000 + *site as u64 * 4)),
                    reachable,
                    "length={length:#x} test={test}"
                );
            }
            for base in [0u32, 0x7ffffffc, 0x80000000, 0xfffffff0, 0xfffffffc] {
                for residue in 0..4 {
                    let d = base + residue;
                    let r = (d.wrapping_add(length) & !3).wrapping_sub(d.wrapping_add(3) & !3);
                    let taken = if test == 4 { 32i32 < r as i32 } else { 32 < r };
                    let (context, _) = direct_context(&f, length, Some(d), &memory);
                    let s = specialized(&f, 0x3000, &context, &memory, &mut || Ok(())).unwrap();
                    let proof = s.specialization().unwrap();
                    for site in &f.bulk {
                        assert_eq!(
                            !proof.unreachable().contains(&(0x3000 + *site as u64 * 4)),
                            taken,
                            "D={d:#x}, N={length:#x}, test={test}"
                        );
                    }
                }
            }
        }
    }
}
#[test]
fn another_reachable_entry_keeps_the_bulk_region_despite_a_false_guard() {
    let memory = WorkingMemory::new(16 * 1024 * 1024).unwrap();
    let f = fixture(4, true);
    let (context, generic) = direct_context(&f, 20, None, &memory);
    let s = specialized(&f, 0x3000, &context, &memory, &mut || Ok(())).unwrap();
    let proof = s.specialization().unwrap();
    assert!(
        !proof
            .branches()
            .iter()
            .find(|b| b.site == 0x3000 + f.guard as u64 * 4)
            .unwrap()
            .taken
    );
    for site in f.bulk {
        assert!(!proof.unreachable().contains(&(0x3000 + site as u64 * 4)));
    }
    let caller = caller(Some(20), None, 0x3000, &memory);
    let applied = call_writes(
        &index(&caller, &memory),
        0x1000,
        call_site(&caller),
        ABI,
        &[&s],
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    assert_eq!(applied.writes.len(), generic.writes.len());
}
#[test]
fn changed_or_unknown_target_abi_and_arguments_cannot_reuse_exclusions() {
    let memory = WorkingMemory::new(16 * 1024 * 1024).unwrap();
    let f = fixture(4, false);
    let (context, generic) = direct_context(&f, 20, None, &memory);
    let s = specialized(&f, 0x3000, &context, &memory, &mut || Ok(())).unwrap();
    let caller_records = caller(Some(20), None, 0x3000, &memory);
    for (target, length, abi, expected_writes, barrier) in [
        (0x3000, Some(20), ABI, 5, None),
        (0x3000, None, ABI, 14, None),
        (0x3000, Some(128), ABI, 14, None),
        (
            0x4000,
            Some(20),
            ABI,
            0,
            Some(WriteBarrierKind::UnknownCallee),
        ),
        (
            0x3000,
            Some(20),
            None,
            0,
            Some(WriteBarrierKind::UnknownAbi),
        ),
    ] {
        let changed = caller(length, None, target, &memory);
        let applied = call_writes(
            &index(&changed, &memory),
            0x1000,
            call_site(&changed),
            abi,
            &[&s],
            &memory,
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(applied.writes.len(), expected_writes);
        if let Some(barrier) = barrier {
            assert!(applied.barriers.iter().any(|b| b.kind == barrier));
        }
    }
    let mut unknown = caller_records.to_vec();
    for record in &mut unknown {
        if let FunctionRecord::Transfer { target, .. } = record {
            *target = AbstractValue::Unknown;
        }
    }
    assert!(
        call_context(
            &index(&unknown, &memory),
            0x1000,
            call_site(&unknown),
            ABI,
            &generic,
            &memory,
            &mut || Ok(())
        )
        .unwrap()
        .is_none()
    );
    let applied = call_writes(
        &index(&unknown, &memory),
        0x1000,
        call_site(&unknown),
        ABI,
        &[&s],
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    assert!(applied.writes.is_empty());
    assert!(
        applied
            .barriers
            .iter()
            .any(|b| b.kind == WriteBarrierKind::UnknownCallee)
    );
    assert_eq!(
        specialized(&f, 0x4000, &context, &memory, &mut || Ok(()))
            .err()
            .unwrap()
            .code,
        ErrorCode::InvalidRequest
    );
}
#[test]
fn unsupported_reachable_instructions_suppress_exclusions_but_unvisited_bytes_do_not() {
    let memory = WorkingMemory::new(16 * 1024 * 1024).unwrap();
    let f = fixture(4, false);
    let (context, _) = direct_context(&f, 20, None, &memory);
    let mut bad = fixture(4, false);
    // Valid CFG path to a reachable undecodable word opens the shared graph.
    bad.words[0] = 0xffffffff;
    let s = specialized(&bad, 0x3000, &context, &memory, &mut || Ok(())).unwrap();
    assert!(!s.closed_graph);
    assert!(s.specialization().is_none());
    let mut semantic = fixture(4, false);
    semantic.words[0] = 0x00100073; // ebreak decodes but shared semantics are unsupported
    let records = facts(&semantic.words, 0x3000, &memory);
    assert!(records.iter().any(|r| matches!(
        r,
        FunctionRecord::SemanticGap {
            reason: SemanticGapReason::UnsupportedInstruction,
            ..
        }
    )));
    let s = specialized(&semantic, 0x3000, &context, &memory, &mut || Ok(())).unwrap();
    assert!(s.specialization().is_none());
    let mut unvisited = fixture(4, false);
    unvisited.words.push(0xffffffff); // behind the final return
    let records = facts(&unvisited.words, 0x3000, &memory);
    assert!(records.iter().any(|r| matches!(
        r,
        FunctionRecord::Gap {
            reason: GapReason::Unvisited,
            ..
        }
    )));
    let s = specialized(&unvisited, 0x3000, &context, &memory, &mut || Ok(())).unwrap();
    assert!(s.closed_graph);
    for site in f.bulk {
        assert!(
            s.specialization()
                .unwrap()
                .unreachable()
                .contains(&(0x3000 + site as u64 * 4))
        );
    }
}

#[test]
fn an_unproved_alignment_input_or_unknown_noncall_edge_prevents_bulk_exclusion() {
    let memory = WorkingMemory::new(16 * 1024 * 1024).unwrap();
    let f = fixture(4, false);
    let (context, _) = direct_context(&f, 20, None, &memory);
    let mut changed = fixture(4, false);
    changed.words[3] = addi(15, 11, 0); // the already-aligned path supplies a different base
    let s = specialized(&changed, 0x3000, &context, &memory, &mut || Ok(())).unwrap();
    for site in &changed.bulk {
        assert!(
            !s.specialization()
                .unwrap()
                .unreachable()
                .contains(&(0x3000 + *site as u64 * 4))
        );
    }
    changed.words[0] = (6 << 15) | 0x67; // unknown non-call target may enter local blocks
    let s = specialized(&changed, 0x3000, &context, &memory, &mut || Ok(())).unwrap();
    assert!(!s.closed_graph);
    assert!(s.specialization().is_none());
}
#[test]
fn every_cancellation_and_admission_failure_releases_context_scratch() {
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    let f = fixture(4, false);
    let (context, _) = direct_context(&f, 20, None, &memory);
    let before = memory.used();
    let mut calls = 0;
    let s = specialized(&f, 0x3000, &context, &memory, &mut || {
        calls += 1;
        Ok(())
    })
    .unwrap();
    assert!(s.specialization().is_some());
    drop(s);
    assert_eq!(memory.used(), before);
    for stop in 0..calls {
        let mut at = 0;
        let result = specialized(&f, 0x3000, &context, &memory, &mut || {
            if at == stop {
                return Err(Error::new(ErrorCode::Cancelled, "test cancellation"));
            }
            at += 1;
            Ok(())
        });
        assert_eq!(
            result.err().unwrap().code,
            ErrorCode::Cancelled,
            "checkpoint {stop}"
        );
        assert_eq!(memory.used(), before, "checkpoint {stop}");
    }
    // Vary budgets across the retained proof and temporary pass allocations.
    for limit in [1, 4096, 8192, 16384, 32768, 65536, 131072, 262144] {
        let constrained = WorkingMemory::new(limit).unwrap();
        match specialized(&f, 0x3000, &context, &constrained, &mut || Ok(())) {
            Ok(summary) => drop(summary),
            Err(error) => assert_eq!(error.code, ErrorCode::ResourceLimited),
        }
        assert_eq!(constrained.used(), 0, "budget {limit}");
    }
}

#[test]
fn cached_call_proofs_recheck_length_even_when_every_store_was_excluded() {
    let memory = WorkingMemory::new(16 * 1024 * 1024).unwrap();
    let f = Fixture {
        words: vec![
            addi(13, 0, 32),
            branch(12, 13, 8, 6),
            store(0, 10, 0, 2),
            RET,
        ],
        guard: 1,
        bulk: vec![2],
        back: 2,
    };
    let records = facts(&f.words, 0x3000, &memory);
    let generic = summary(&records, 0x3000, &memory);
    let code = spilled(&[addi(12, 0, 20), jal(1, 0x3000 - 0x1010)], 2);
    let before = facts(&code, 0x1000, &memory);
    let context = call_context(
        &index(&before, &memory),
        0x1000,
        call_site(&before),
        ABI,
        &generic,
        &memory,
        &mut || Ok(()),
    )
    .unwrap()
    .unwrap();
    let specialized = specialized(&f, 0x3000, &context, &memory, &mut || Ok(())).unwrap();
    let calls = stack_calls(
        &index(&before, &memory),
        0x1000,
        ABI,
        &[&specialized],
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    let same = facts_calls(&code, 0x1000, &memory, true, Some(&calls));
    assert!(
        summary(&same, 0x1000, &memory).reloads[0]
            .value
            .known_base()
    );
    for changed_instruction in [addi(12, 0, 128), lw(12, 11, 0)] {
        let mut changed = code.clone();
        changed[3] = changed_instruction;
        let after = facts_calls(&changed, 0x1000, &memory, true, Some(&calls));
        assert_eq!(
            summary(&after, 0x1000, &memory).reloads[0].value,
            WriteValue::UNKNOWN
        );
    }
    // Canonical expressions can be normalized by the summary algebra even
    // when the live value engine retains an expression. A cached proof must
    // not forget the length dependency just because its raw leaf is absent.
    let mut expression_inputs = before.to_vec();
    let id = expression_inputs
        .iter()
        .filter(|r| matches!(r, FunctionRecord::Expression { .. }))
        .count() as u32;
    expression_inputs.push(FunctionRecord::Expression {
        id,
        offset: 0x100c,
        expression: Expression::Integer {
            op: IntegerOp::Add,
            left: AbstractValue::Constant { value: 8 },
            right: AbstractValue::Constant { value: 12 },
        },
    });
    for record in &mut expression_inputs {
        if let FunctionRecord::CallInputs { registers, .. } = record {
            registers[12] = AbstractValue::Expression { id };
        }
    }
    let expr_index = index(&expression_inputs, &memory);
    let immediate = call_writes(
        &expr_index,
        0x1000,
        call_site(&expression_inputs),
        ABI,
        &[&specialized],
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    assert!(immediate.writes.is_empty());
    assert!(immediate.barriers.is_empty());
    let cached = stack_calls(
        &expr_index,
        0x1000,
        ABI,
        &[&specialized],
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    let after = facts_calls(&code, 0x1000, &memory, true, Some(&cached));
    assert_eq!(
        summary(&after, 0x1000, &memory).reloads[0].value,
        WriteValue::UNKNOWN
    );
}

#[test]
fn every_required_constant_and_each_target_alternative_keeps_its_own_scope() {
    let memory = WorkingMemory::new(16 * 1024 * 1024).unwrap();
    let f = fixture(4, false);
    let (context, generic) = direct_context(&f, 20, Some(0xfffffffd), &memory);
    let s = specialized(&f, 0x3000, &context, &memory, &mut || Ok(())).unwrap();
    for (destination, expected) in [(Some(0xfffffffd), 5), (Some(0xfffffffe), 14), (None, 14)] {
        let caller = caller(Some(20), destination, 0x3000, &memory);
        let out = call_writes(
            &index(&caller, &memory),
            0x1000,
            call_site(&caller),
            ABI,
            &[&s],
            &memory,
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(out.writes.len(), expected);
    }
    let unknown = caller(None, None, 0x3000, &memory);
    assert!(
        call_context(
            &index(&unknown, &memory),
            0x1000,
            call_site(&unknown),
            ABI,
            &generic,
            &memory,
            &mut || Ok(())
        )
        .unwrap()
        .is_none()
    );
    let other_records = facts(&[store(0, 10, 0, 2), RET], 0x4000, &memory);
    let other = summary(&other_records, 0x4000, &memory);
    let caller = caller(Some(20), Some(0xfffffffd), 0x3000, &memory);
    for length in [20, 128] {
        let mut changed = caller.to_vec();
        for record in &mut changed {
            match record {
                FunctionRecord::Transfer { target, .. } => {
                    *target = AbstractValue::Alternatives {
                        values: ValueAlternatives::new(vec![
                            ValueAlternative::ImageAddress { address: 0x3000 },
                            ValueAlternative::ImageAddress { address: 0x4000 },
                            ValueAlternative::Constant { value: 0 },
                        ])
                        .unwrap(),
                    }
                }
                FunctionRecord::CallInputs { registers, .. } => {
                    registers[12] = AbstractValue::Constant { value: length }
                }
                _ => (),
            }
        }
        let out = call_writes(
            &index(&changed, &memory),
            0x1000,
            call_site(&changed),
            ABI,
            &[&s, &other],
            &memory,
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(out.writes.len(), if length == 20 { 6 } else { 15 });
        assert!(
            out.writes
                .iter()
                .any(|w| w.callee == 0x4000 && w.write.site == 0x4000)
        );
        assert!(
            out.writes
                .iter()
                .all(|w| w.write.occurrence == WriteOccurrence::May)
        );
        assert!(
            out.barriers
                .iter()
                .any(|b| b.kind == WriteBarrierKind::UnknownCallee)
        );
    }
}
