use super::*;

mod exit_values;

pub(super) fn branch(left: u32, right: u32, delta: i32, test: u32) -> u32 {
    let n = delta as u32;
    ((n >> 12 & 1) << 31)
        | ((n >> 5 & 63) << 25)
        | (right << 20)
        | (left << 15)
        | (test << 12)
        | ((n >> 1 & 15) << 8)
        | ((n >> 11 & 1) << 7)
        | 0x63
}
pub(super) fn andi(dest: u32, base: u32, mask: u32) -> u32 {
    addi(dest, base, mask as i32) | (7 << 12)
}
fn loop_code(pointer: u32) -> Vec<u32> {
    vec![
        addi(pointer, 10, 0),
        jal(0, 20), // Enter the shared alignment test before the first store.
        (lw(14, 11, 0) & !(7 << 12)) | (4 << 12), // lbu
        addi(pointer, pointer, 1),
        addi(11, 11, 1),
        store(14, pointer, -1, 0),
        andi(14, pointer, 3),
        branch(14, 0, -20, 1), // bne residue, zero, body
        RET,
    ]
}

#[test]
fn alignment_induction_is_symbolic_and_independent_of_pcs_and_register_choice() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    for (entry, pointer) in [(0x1000, 15), (0x3720, 6), (0x802000, 28)] {
        let records = facts(&loop_code(pointer), entry, &memory);
        let out = summary(&records, entry, &memory);
        assert!(out.closed_graph);
        assert_eq!(out.writes.len(), 3);
        for (n, write) in out.writes.iter().enumerate() {
            assert_eq!(write.site, entry + 20);
            assert_eq!(write.origin.function, entry);
            assert_eq!(
                write.address,
                WriteValue::Base {
                    base: WriteBase::Argument(0),
                    offset: n as u32,
                }
            );
            assert_eq!(write.value, WriteValue::UNKNOWN);
            assert_eq!(write.width, 1);
            assert_eq!(write.occurrence, WriteOccurrence::May);
            assert_eq!(
                write.proof,
                WriteProof::AlignmentLoop(AlignmentLoopProof {
                    header: entry + 8,
                    guard: entry + 24,
                    back_edge: entry + 28,
                    step: entry + 12,
                    entry_edge: entry + 4,
                    pointer: pointer as u8,
                    mask: 3,
                })
            );
        }
        assert_eq!(out.barriers.len(), 1);
        assert_eq!(out.barriers[0].kind, WriteBarrierKind::UnknownValue);
    }
    // Exhaust the residue quotient, not a sample of RV32 input words. With
    // A=4 dividing 2^32, wrapping +1 preserves this transition for every word.
    for residue in 0u32..4 {
        let rank = (4 - residue) & 3;
        if rank == 0 {
            assert_eq!(residue, 0); // the entry guard bypasses all stores
        } else {
            assert_ne!(residue, 0);
            assert_eq!((4 - ((residue + 1) & 3)) & 3, rank - 1);
            assert!(rank <= 3);
        }
    }
}

#[test]
fn alignment_addresses_substitute_for_all_residues_and_wrap_at_rv32_boundary() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let records = facts(&loop_code(15), 0x3000, &memory);
    let body = summary(&records, 0x3000, &memory);
    let records = facts(&[jal(0, 0x1000)], 0x2000, &memory);
    let tail = summary_with_callees(&records, 0x2000, &[&body], &memory);
    for address in [
        0x100u32, 0x101, 0x102, 0x103, 0xfffffffc, 0xfffffffd, 0xfffffffe, 0xffffffff,
    ] {
        let records = facts(
            &[
                (address.wrapping_add(0x800) & 0xfffff000) | (10 << 7) | 0x37,
                addi(10, 10, ((address << 20) as i32) >> 20),
                jal(1, 0x2000 - 0x1008),
                RET,
            ],
            0x1000,
            &memory,
        );
        let applied = call_writes(
            &index(&records, &memory),
            0x1000,
            0x1008,
            ABI,
            &[&tail],
            &memory,
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(applied.writes.len(), 3);
        for (n, write) in applied.writes.iter().enumerate() {
            assert_eq!(write.write.address, exact(address.wrapping_add(n as u32)));
            assert_eq!(write.source, tail.writes[n]);
            assert_eq!(write.write.proof, body.writes[n].proof);
            assert_eq!(write.write.origin.tail, tail.tail);
            assert_eq!(write.write.occurrence, WriteOccurrence::May);
        }
    }
}

#[test]
fn alignment_proof_rejects_bypass_extra_entries_step_pointer_exit_and_width_changes() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let baseline = loop_code(15);
    let mut variants = Vec::new();
    for (at, replacement) in [
        (1, jal(0, 4)), // bypass the initial alignment check
        (3, addi(15, 15, 2)),
        (3, addi(15, 15, -1)),
        (4, addi(15, 15, 1)), // second pointer definition
        (3, (12 << 20) | (15 << 15) | (15 << 7) | 0x33), // variable step
        (6, andi(14, 15, 2)), // not a power-of-two alignment mask
        (6, andi(14, 10, 3)), // test the wrong pointer
        (6, andi(15, 15, 3)), // test changes the pointer
        (7, branch(14, 0, -20, 0)), // continue on equality
        (7, branch(14, 12, -20, 1)), // compare to a nonzero/unknown value
        (5, store(14, 15, -1, 1)),
        (5, store(14, 15, -1, 2)),
        (4, jal(1, 0x1000)), // opaque call in the body
        (4, 0x00000073),     // unsupported effect
        (0, 0xc00027f3),     // opaque initial pointer (cycle CSR)
    ] {
        let mut words = baseline.clone();
        words[at] = replacement;
        variants.push(words);
    }
    // Two distinct predecessors, both after the same initialization and both
    // targeting the test: no unproved choice of a preferred incoming edge.
    let mut extra = vec![addi(15, 10, 0), branch(12, 0, 8, 1), jal(0, 24), jal(0, 20)];
    extra.extend_from_slice(&baseline[2..]);
    variants.push(extra);
    // An additional entry directly into the body through a conditional path.
    let mut entered = baseline.clone();
    entered[0] = branch(12, 0, 12, 1);
    variants.push(entered);
    for (case, words) in variants.iter().enumerate() {
        let records = facts(words, 0x1000, &memory);
        let out = summary(&records, 0x1000, &memory);
        assert!(
            !records
                .iter()
                .any(|r| matches!(r, FunctionRecord::StoreAddressRange { .. })),
            "case {case}"
        );
        assert!(
            out.barriers.iter().any(|b| matches!(
                b.kind,
                WriteBarrierKind::UnknownStore | WriteBarrierKind::OpenGraph
            )),
            "case {case}"
        );
    }
}

#[test]
fn alignment_accounts_for_store_order_and_signed_displacement() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    for (before, displacement, first) in [(false, -1, 0), (false, 7, 8), (true, -1, u32::MAX)] {
        let mut words = loop_code(15);
        words[5] = store(14, 15, displacement, 0);
        if before {
            words.swap(3, 5);
        }
        let records = facts(&words, 0x1000, &memory);
        let out = summary(&records, 0x1000, &memory);
        assert_eq!(out.writes.len(), 3);
        for (n, write) in out.writes.iter().enumerate() {
            assert_eq!(
                write.address,
                WriteValue::Base {
                    base: WriteBase::Argument(0),
                    offset: first.wrapping_add(n as u32),
                }
            );
        }
    }
}

#[test]
fn alignment_initial_value_must_agree_on_every_path_before_the_entry_edge() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    for delta in [0, 4] {
        let mut words = vec![
            branch(12, 0, 12, 1),
            addi(15, 10, 0),
            jal(0, 8),
            addi(15, 10, delta),
            jal(0, 20),
        ];
        words.extend_from_slice(&loop_code(15)[2..]);
        let records = facts(&words, 0x1000, &memory);
        let out = summary(&records, 0x1000, &memory);
        if delta == 0 {
            assert_eq!(out.writes.len(), 3);
            assert!(
                out.writes
                    .iter()
                    .all(|w| matches!(w.proof, WriteProof::AlignmentLoop(_)))
            );
        } else {
            assert_eq!(out.writes.len(), 1);
            assert_eq!(out.writes[0].address, WriteValue::UNKNOWN);
            assert!(
                out.barriers
                    .iter()
                    .any(|b| b.kind == WriteBarrierKind::UnknownStore)
            );
        }
    }
}

#[test]
fn alignment_expansion_is_bounded_to_small_power_of_two_masks() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    for mask in [1, 3, 7, 15] {
        let mut words = loop_code(15);
        words[6] = andi(14, 15, mask);
        let records = facts(&words, 0x1000, &memory);
        let out = summary(&records, 0x1000, &memory);
        if mask < 8 {
            assert_eq!(out.writes.len(), mask as usize);
            assert!(out.writes.iter().all(|w| matches!(w.proof,
                WriteProof::AlignmentLoop(proof) if u32::from(proof.mask) == mask)));
        } else {
            assert_eq!(out.writes[0].address, WriteValue::UNKNOWN);
            assert!(
                out.barriers
                    .iter()
                    .any(|b| b.kind == WriteBarrierKind::UnknownStore)
            );
        }
    }
}

#[test]
fn alignment_bytes_reach_caller_stack_but_other_unknown_writes_keep_the_barrier() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let trampoline = facts(&[jal(0, 0x1000)], 0x2000, &memory);
    for other_unknown in [false, true] {
        let mut words = loop_code(15);
        if other_unknown {
            words.insert(8, store(0, 13, 0, 2));
        }
        let records = facts(&words, 0x3000, &memory);
        let body = summary(&records, 0x3000, &memory);
        let tail = summary_with_callees(&trampoline, 0x2000, &[&body], &memory);
        let records = facts(
            &[addi(10, 2, -400), jal(1, 0x2000 - 0x1004), RET],
            0x1000,
            &memory,
        );
        let applied = call_writes(
            &index(&records, &memory),
            0x1000,
            0x1004,
            ABI,
            &[&tail],
            &memory,
            &mut || Ok(()),
        )
        .unwrap();
        for (n, write) in applied.writes.iter().take(3).enumerate() {
            assert_eq!(
                write.write.address,
                WriteValue::Base {
                    base: WriteBase::Stack {
                        function: 0x1000,
                        offset: -400
                    },
                    offset: n as u32,
                }
            );
        }
        assert_eq!(
            applied
                .preserves_stack_word(0x1000, -600, &mut || Ok(()))
                .unwrap(),
            !other_unknown
        );
        assert!(
            !applied
                .preserves_stack_word(0x1000, -400, &mut || Ok(()))
                .unwrap()
        );
    }
}

#[test]
fn open_graph_discards_an_alignment_address_proof() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let records = facts(&loop_code(15), 0x1000, &memory);
    let mut records = records.to_vec();
    records.push(FunctionRecord::Edge {
        from: 0x1000,
        target: None,
        relation: EdgeKind::Indirect,
        external: true,
    });
    let out = summary(&records, 0x1000, &memory);
    assert!(!out.closed_graph);
    assert!(
        out.writes
            .iter()
            .all(|w| w.address == WriteValue::UNKNOWN && w.proof == WriteProof::OpenGraph)
    );
}

#[test]
fn alignment_analysis_and_expansion_release_memory_at_every_cancelled_step() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let code: Vec<_> = exit_values::joined_code(15, 12)
        .iter()
        .flat_map(|w| w.to_le_bytes())
        .collect();
    let run = |memory: &WorkingMemory, c: &mut dyn RunControl| {
        let mut saved = Saved(RecordBuffer::new(memory));
        research_with_options(
            FunctionInput {
                image: Some(&NoMemory),
                section: 1,
                extent: CodeRange {
                    start: 0x1000,
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
            memory,
            c,
            &mut saved,
            ResearchOptions {
                abi: ABI,
                symbolic_spills: true,
                stack_calls: None,
            },
        )
        .map(|_| ())
    };
    let mut steps = 0;
    run(&memory, &mut || {
        steps += 1;
        Ok(())
    })
    .unwrap();
    assert_eq!(memory.used(), 0);
    for stop in 1..=steps {
        let mut seen = 0;
        let error = run(&memory, &mut || {
            seen += 1;
            if seen == stop {
                Err(Error::new(ErrorCode::Cancelled, "test"))
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::Cancelled);
        assert_eq!(memory.used(), 0);
    }
    for limit in [1, 1024, 4096, 16384] {
        let limited = WorkingMemory::new(limit).unwrap();
        assert_eq!(
            run(&limited, &mut || Ok(())).unwrap_err().code,
            ErrorCode::ResourceLimited
        );
        assert_eq!(limited.used(), 0);
    }
    let records = facts(&loop_code(15), 0x1000, &memory);
    let index = index(&records, &memory);
    let before = memory.used();
    let mut steps = 0;
    function_writes(&index, 0x1000, ABI, &[], &memory, &mut || {
        steps += 1;
        Ok(())
    })
    .unwrap();
    for stop in 1..=steps {
        let mut seen = 0;
        let result = function_writes(&index, 0x1000, ABI, &[], &memory, &mut || {
            seen += 1;
            if seen == stop {
                Err(Error::new(ErrorCode::Cancelled, "test"))
            } else {
                Ok(())
            }
        });
        assert_eq!(result.err().unwrap().code, ErrorCode::Cancelled);
        assert_eq!(memory.used(), before);
    }
}
