//! End-to-end RV32 facts, stack words, summaries and callsite substitution.
use oer_analysis_riscv_analysis::{
    FunctionInput, PreparedReferences, ResearchOptions, research_with_options, writes::*,
};
use oer_analysis_riscv_lift::RiscvDecoder;
use oer_analysis_riscv_model::*;

struct Saved<'m>(RecordBuffer<'m>);
impl FunctionSink for Saved<'_> {
    fn record(&mut self, record: &FunctionRecord, c: &mut dyn RunControl) -> Result<()> {
        self.0.push(record.clone(), c.position())
    }
}
struct NoMemory;
impl ImageMemory for NoMemory {
    fn read_constant(&self, _: u32, _: &mut [u8], _: &mut dyn RunControl) -> Result<bool> {
        Ok(false)
    }
}
fn facts<'m>(words: &[u32], start: u64, memory: &'m WorkingMemory) -> RecordBuffer<'m> {
    facts_profile(words, start, memory, true)
}
fn facts_profile<'m>(
    words: &[u32],
    start: u64,
    memory: &'m WorkingMemory,
    symbolic_spills: bool,
) -> RecordBuffer<'m> {
    facts_calls(words, start, memory, symbolic_spills, None)
}
fn facts_calls<'m>(
    words: &[u32],
    start: u64,
    memory: &'m WorkingMemory,
    symbolic_spills: bool,
    stack_calls: Option<&StackCalls<'_>>,
) -> RecordBuffer<'m> {
    let code: Vec<_> = words.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut saved = Saved(RecordBuffer::new(memory));
    research_with_options(
        FunctionInput {
            image: Some(&NoMemory),
            section: 1,
            extent: CodeRange {
                start,
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
        &mut || Ok(()),
        &mut saved,
        ResearchOptions {
            abi: Some(CallAbi::RiscvInteger),
            symbolic_spills,
            stack_calls,
        },
    )
    .unwrap();
    saved.0
}
fn index<'a, 'm>(
    records: &'a [FunctionRecord],
    memory: &'m WorkingMemory,
) -> FunctionIndex<'a, 'm> {
    FunctionIndex::new(records, RecordPolicy::Canonical, memory, &mut || Ok(())).unwrap()
}
fn summary<'m>(
    records: &[FunctionRecord],
    entry: u64,
    memory: &'m WorkingMemory,
) -> FunctionWrites<'m> {
    function_writes(
        &index(records, memory),
        entry,
        Some(CallAbi::RiscvInteger),
        memory,
        &mut || Ok(()),
    )
    .unwrap()
}
fn addi(rd: u32, rs: u32, n: i32) -> u32 {
    ((n as u32 & 0xfff) << 20) | (rs << 15) | (rd << 7) | 0x13
}
fn lw(rd: u32, base: u32, n: i32) -> u32 {
    ((n as u32 & 0xfff) << 20) | (base << 15) | (2 << 12) | (rd << 7) | 3
}
fn store(source: u32, base: u32, n: i32, width: u32) -> u32 {
    let n = n as u32 & 0xfff;
    ((n >> 5) << 25) | (source << 20) | (base << 15) | (width << 12) | ((n & 31) << 7) | 0x23
}
fn jal(rd: u32, n: i32) -> u32 {
    let n = n as u32;
    ((n >> 20 & 1) << 31)
        | ((n >> 1 & 0x3ff) << 21)
        | ((n >> 11 & 1) << 20)
        | (n & 0xff000)
        | (rd << 7)
        | 0x6f
}
const RET: u32 = 0x00008067;
const ABI: Option<CallAbi> = Some(CallAbi::RiscvInteger);
fn exact(value: u32) -> WriteValue {
    WriteValue::base(WriteBase::Address(value))
}

#[test]
fn two_callers_substitute_actual_argument_registers_and_store_displacement() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let callee = facts(&[store(11, 10, 12, 2), RET], 0x1000, &memory);
    let callee = summary(&callee, 0x1000, &memory);
    assert_eq!(callee.writes[0].occurrence, WriteOccurrence::Must);
    for (entry, pointer) in [(0x2000, 71), (0x3000, 93)] {
        let caller = facts(
            &[
                addi(10, 0, 512),
                addi(11, 0, pointer),
                jal(1, 0x1000 - entry - 8),
                RET,
            ],
            entry as u64,
            &memory,
        );
        let applied = call_writes(
            &index(&caller, &memory),
            entry as u64,
            entry as u64 + 8,
            ABI,
            &[&callee],
            &memory,
            &mut || Ok(()),
        )
        .unwrap();
        assert_eq!(applied.writes.len(), 1);
        assert_eq!(applied.writes[0].write.address, exact(524));
        assert_eq!(applied.writes[0].write.value, exact(pointer as u32));
        assert!(applied.barriers.is_empty());
    }
}

#[test]
fn a_conditional_store_keeps_the_no_write_path() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    // beq a2, zero, +8; sw a1,12(a0); ret
    let records = facts(&[0x00060463, store(11, 10, 12, 2), RET], 0x1000, &memory);
    let summary = summary(&records, 0x1000, &memory);
    assert!(summary.closed_graph);
    assert_eq!(summary.writes[0].occurrence, WriteOccurrence::May);
}

fn spilled(intervening: &[u32], load_width: u32) -> Vec<u32> {
    let mut words = vec![addi(2, 2, -16), lw(5, 10, 8), store(5, 2, 4, 2)];
    words.extend_from_slice(intervening);
    words.push((lw(6, 2, 4) & !(7 << 12)) | (load_width << 12));
    words.extend([store(6, 10, 12, 2), addi(2, 2, 16), RET]);
    words
}

#[test]
fn a_field_load_is_captured_through_a_proven_stack_word_and_disjoint_stack_write() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    for between in [
        vec![],
        vec![store(12, 2, 8, 2)],
        vec![0x00060463, addi(7, 0, 4)],
    ] {
        let records = facts(&spilled(&between, 2), 0x1000, &memory);
        let summary = summary(&records, 0x1000, &memory);
        let value = summary.writes.last().unwrap().value;
        assert_eq!(
            value,
            WriteValue::Field {
                function: 0x1000,
                read: 0x1004,
                base: WriteBase::Argument(0),
                displacement: 8,
                width: 4,
                signed: false,
                offset: 0
            }
        );
        assert_eq!(summary.reloads[0].value, value);
        assert_eq!(summary.reloads[0].slot, -12);
        assert_eq!(summary.reloads[0].width, 4);
    }
}

#[test]
fn spill_overlap_unknown_alias_and_unknown_call_break_captured_field_provenance() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    for between in [
        vec![store(12, 2, 6, 0)],
        vec![store(12, 13, 0, 2)],
        vec![jal(1, 0x800)],
    ] {
        let records = facts(&spilled(&between, 2), 0x1000, &memory);
        let summary = summary(&records, 0x1000, &memory);
        assert_eq!(summary.writes.last().unwrap().value, WriteValue::UNKNOWN);
        assert_eq!(summary.reloads[0].value, WriteValue::UNKNOWN);
        assert!(summary.has_unknown_memory_effects());
    }
    let records = facts(&spilled(&[], 1), 0x1000, &memory);
    let summary = summary(&records, 0x1000, &memory);
    assert_eq!(summary.writes.last().unwrap().value, WriteValue::UNKNOWN);
}

#[test]
fn overwriting_a_field_before_read_never_projects_its_entry_contents() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let mut code = vec![store(11, 10, 8, 2)];
    code.extend(spilled(&[], 2));
    let records = facts(&code, 0x1000, &memory);
    let summary = summary(&records, 0x1000, &memory);
    assert!(matches!(
        summary.writes.last().unwrap().value,
        WriteValue::Field {
            read: 0x1008,
            displacement: 8,
            ..
        }
    ));
    assert_ne!(
        summary.writes.last().unwrap().value,
        summary.writes[0].value
    );
    // A write after the capture kills the spill if it may alias it, too.
    let records = facts(&spilled(&[store(11, 10, 8, 2)], 2), 0x1000, &memory);
    assert_eq!(summary_for_last(&records, &memory), WriteValue::UNKNOWN);
}
fn summary_for_last(records: &[FunctionRecord], memory: &WorkingMemory) -> WriteValue {
    summary(records, 0x1000, memory)
        .writes
        .last()
        .unwrap()
        .value
}

#[test]
fn overlapping_and_unknown_stores_and_atomics_remain_separate_effects() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let records = facts(
        &[
            store(11, 10, 12, 2),
            store(12, 10, 14, 1),
            store(13, 14, 0, 2),
            0x00b5202f,
            RET,
        ],
        0x1000,
        &memory,
    );
    let summary = summary(&records, 0x1000, &memory);
    assert_eq!(summary.writes.len(), 3);
    assert_eq!(summary.writes[0].width, 4);
    assert_eq!(summary.writes[1].width, 2);
    assert_eq!(summary.writes[1].value, WriteValue::UNKNOWN);
    assert!(
        summary
            .barriers
            .iter()
            .any(|b| b.kind == WriteBarrierKind::Atomic)
    );
    assert!(summary.has_unknown_memory_effects());
}

#[test]
fn unknown_call_argument_callee_and_abi_cannot_create_exact_stores() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let records = facts(&[store(11, 10, 0, 2), RET], 0x1000, &memory);
    let summary = summary(&records, 0x1000, &memory);
    // andi a1,a2,3 is outside the supported affine argument form.
    let records = facts(
        &[addi(10, 0, 512), 0x00367593, jal(1, -0x1008), RET],
        0x2000,
        &memory,
    );
    let caller = index(&records, &memory);
    let applied = call_writes(
        &caller,
        0x2000,
        0x2008,
        ABI,
        &[&summary],
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    assert_eq!(applied.writes[0].write.value, WriteValue::UNKNOWN);
    assert!(!applied.barriers.is_empty());
    for (abi, summaries) in [(None, vec![&summary]), (ABI, vec![])] {
        let applied = call_writes(
            &caller,
            0x2000,
            0x2008,
            abi,
            &summaries,
            &memory,
            &mut || Ok(()),
        )
        .unwrap();
        assert!(applied.writes.is_empty());
        assert!(!applied.barriers.is_empty());
    }
    let records = facts(&[0x000280e7, RET], 0x2000, &memory);
    let applied = call_writes(
        &index(&records, &memory),
        0x2000,
        0x2000,
        ABI,
        &[&summary],
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
}

#[test]
fn x0_and_open_graphs_never_supply_a_field_pointer() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let records = facts(&[lw(0, 10, 8), store(0, 10, 12, 2), RET], 0x1000, &memory);
    assert_eq!(summary_for_last(&records, &memory), exact(0));
    let records = facts(&[store(11, 10, 12, 2), 0x00030067], 0x1000, &memory);
    let summary = summary(&records, 0x1000, &memory);
    assert!(!summary.closed_graph);
    assert_eq!(summary.writes[0].value, WriteValue::UNKNOWN);
    assert_eq!(summary.writes[0].proof, WriteProof::OpenGraph);
}

#[test]
fn all_callee_alternatives_are_unioned_and_a_missing_one_remains_unknown() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let a = facts(&[store(11, 10, 12, 2), RET], 0x1000, &memory);
    let b = facts(&[store(12, 10, 16, 2), RET], 0x1100, &memory);
    let (a, b) = (summary(&a, 0x1000, &memory), summary(&b, 0x1100, &memory));
    let records = facts(
        &[
            addi(10, 0, 512),
            addi(11, 0, 77),
            addi(12, 0, 88),
            jal(1, -0x100c),
            RET,
        ],
        0x2000,
        &memory,
    );
    let mut records = records.to_vec();
    for record in &mut records {
        if let FunctionRecord::Transfer {
            target,
            offset: 0x200c,
            ..
        } = record
        {
            *target = AbstractValue::Alternatives {
                values: ValueAlternatives::new(vec![
                    ValueAlternative::ImageAddress { address: 0x1000 },
                    ValueAlternative::ImageAddress { address: 0x1100 },
                ])
                .unwrap(),
            };
        }
    }
    let caller = index(&records, &memory);
    let both = call_writes(
        &caller,
        0x2000,
        0x200c,
        ABI,
        &[&a, &b],
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    assert_eq!(both.writes.len(), 2);
    assert_eq!(both.writes[0].write.value, exact(77));
    assert_eq!(both.writes[1].write.value, exact(88));
    assert!(
        both.writes
            .iter()
            .all(|w| w.write.occurrence == WriteOccurrence::May)
    );
    let missing =
        call_writes(&caller, 0x2000, 0x200c, ABI, &[&a], &memory, &mut || Ok(())).unwrap();
    assert_eq!(missing.writes.len(), 1);
    assert!(
        missing
            .barriers
            .iter()
            .any(|b| b.kind == WriteBarrierKind::UnknownCallee)
    );
}

#[test]
fn captured_spills_are_opt_in_and_the_old_profile_stays_unknown() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let code = spilled(&[], 2);
    let old = facts_profile(&code, 0x1000, &memory, false);
    let new = facts_profile(&code, 0x1000, &memory, true);
    assert_eq!(summary_for_last(&old, &memory), WriteValue::UNKNOWN);
    assert!(matches!(
        summary_for_last(&new, &memory),
        WriteValue::Field { read: 0x1004, .. }
    ));
}

#[test]
fn callee_field_reads_bind_the_pointer_without_claiming_entry_memory_values() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let records = facts(&spilled(&[], 2), 0x1000, &memory);
    let summary = summary(&records, 0x1000, &memory);
    for (setup, known) in [(addi(10, 0, 512), true), (0x00367513, false)] {
        let caller = facts(&[setup, jal(1, -0x1004), RET], 0x2000, &memory);
        let applied = call_writes(
            &index(&caller, &memory),
            0x2000,
            0x2004,
            ABI,
            &[&summary],
            &memory,
            &mut || Ok(()),
        )
        .unwrap();
        let result = applied.writes.last().unwrap();
        let WriteValue::Field {
            base,
            read,
            displacement,
            ..
        } = result.write.value
        else {
            panic!("lost read event")
        };
        assert_eq!(read, 0x1004);
        assert_eq!(displacement, 8);
        assert_eq!(
            base,
            if known {
                WriteBase::Address(512)
            } else {
                WriteBase::Unknown
            }
        );
        assert!(matches!(
            result.source.value,
            WriteValue::Field {
                base: WriteBase::Argument(0),
                ..
            }
        ));
        assert_eq!(result.write.value.known_base(), known);
    }
}

#[test]
fn an_open_caller_cannot_lend_partial_argument_facts_to_a_callee() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let callee = facts(&[store(11, 10, 0, 2), RET], 0x1000, &memory);
    let summary = summary(&callee, 0x1000, &memory);
    let caller = facts(
        &[
            addi(10, 0, 512),
            addi(11, 0, 77),
            jal(1, -0x1008),
            0x00030067,
        ],
        0x2000,
        &memory,
    );
    let result = call_writes(
        &index(&caller, &memory),
        0x2000,
        0x2008,
        ABI,
        &[&summary],
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    assert!(result.writes.is_empty());
    assert!(
        result
            .barriers
            .iter()
            .any(|b| b.kind == WriteBarrierKind::OpenGraph)
    );
}

fn cross_call<'m>(
    code: &[u32],
    callees: &[&FunctionWrites<'_>],
    memory: &'m WorkingMemory,
) -> FunctionWrites<'m> {
    let before = facts(code, 0x1000, memory);
    let calls = stack_calls(
        &index(&before, memory),
        0x1000,
        ABI,
        callees,
        memory,
        &mut || Ok(()),
    )
    .unwrap();
    let after = facts_calls(code, 0x1000, memory, true, Some(&calls));
    summary(&after, 0x1000, memory)
}

#[test]
fn complete_call_writes_preserve_only_disjoint_words_and_the_original_load_pc() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    // Caller slot is [-12,-8), a0 is sp+8 = entry-sp-8; its current
    // caller-saved t0 is deliberately used after the call as a separate check.
    let code = spilled(&[addi(10, 2, 8), jal(1, 0x2000 - 0x1010)], 2);
    for (body, preserved) in [
        (vec![store(0, 10, 0, 2), RET], true),
        (vec![store(11, 10, 0, 2), RET], true), // value may be unknown
        (vec![store(0, 10, -2, 2), RET], false), // partial overlap
        (vec![store(0, 10, -1, 0), RET], false), // one byte in word
        (vec![store(0, 2, 0, 2), RET], true),   // callee's own stack
        (vec![store(0, 2, 4, 2), RET], false),
        (vec![store(0, 11, 0, 2), RET], false), // unknown address
        (vec![jal(1, 0x800), RET], false),      // internal call
        (vec![0x00b5202f, RET], false),         // atomic
    ] {
        let callee = facts(&body, 0x2000, &memory);
        let callee = summary(&callee, 0x2000, &memory);
        let after = cross_call(&code, &[&callee], &memory);
        assert_eq!(
            matches!(
                after.reloads[0].value,
                WriteValue::Field { read: 0x1004, .. }
            ),
            preserved,
            "{body:x?}"
        );
        assert_eq!(after.writes.last().unwrap().value, after.reloads[0].value);
    }
    let callee = facts(&[RET], 0x2000, &memory);
    let callee = summary(&callee, 0x2000, &memory);
    let mut code = code;
    // Replace the final store's source by t0: memory survival must not
    // preserve this caller-saved register across even a no-write call.
    let store_at = code.len() - 3;
    code[store_at] = store(5, 10, 12, 2);
    let after = cross_call(&code, &[&callee], &memory);
    assert!(matches!(
        after.reloads[0].value,
        WriteValue::Field { read: 0x1004, .. }
    ));
    assert_eq!(after.writes.last().unwrap().value, WriteValue::UNKNOWN);
}

#[test]
fn every_actual_call_target_needs_a_complete_disjoint_summary() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let a = facts(&[store(0, 10, 0, 2), RET], 0x2000, &memory);
    let good = facts(&[store(0, 10, 4, 2), RET], 0x2100, &memory);
    let bad = facts(&[store(0, 10, -1, 0), RET], 0x2100, &memory);
    let (a, good, bad) = (
        summary(&a, 0x2000, &memory),
        summary(&good, 0x2100, &memory),
        summary(&bad, 0x2100, &memory),
    );
    let code = spilled(
        &[
            addi(10, 2, 8),
            0x00002337, // lui t1,2
            0x00060463, // beq a2,zero,+8
            addi(6, 6, 256),
            0x000300e7, // jalr ra,0(t1)
        ],
        2,
    );
    for (callees, preserved) in [
        (vec![&a, &good], true),
        (vec![&a, &bad], false),
        (vec![&a], false),
    ] {
        let after = cross_call(&code, &callees, &memory);
        assert_eq!(
            matches!(
                after.reloads[0].value,
                WriteValue::Field { read: 0x1004, .. }
            ),
            preserved
        );
    }
    let unknown = spilled(&[addi(10, 2, 8), 0x000700e7], 2); // jalr ra,0(a4)
    assert_eq!(
        cross_call(&unknown, &[&a, &good], &memory).reloads[0].value,
        WriteValue::UNKNOWN
    );
}

#[test]
fn prepared_call_proofs_are_rechecked_for_new_arguments_targets_and_callers() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let callee = facts(&[store(0, 10, 0, 2), RET], 0x2000, &memory);
    let callee = summary(&callee, 0x2000, &memory);
    let code = spilled(&[addi(10, 2, 8), jal(1, 0x2000 - 0x1010)], 2);
    let before = facts(&code, 0x1000, &memory);
    let calls = stack_calls(
        &index(&before, &memory),
        0x1000,
        ABI,
        &[&callee],
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    for (change, word) in [(3, addi(10, 2, 4)), (4, jal(1, 0x2100 - 0x1010))] {
        let mut changed = code.clone();
        changed[change] = word;
        let records = facts_calls(&changed, 0x1000, &memory, true, Some(&calls));
        assert_eq!(
            summary(&records, 0x1000, &memory).reloads[0].value,
            WriteValue::UNKNOWN
        );
    }
    let records = facts_calls(&code, 0x1100, &memory, true, Some(&calls));
    assert_eq!(
        summary(&records, 0x1100, &memory).reloads[0].value,
        WriteValue::UNKNOWN
    );
}

#[test]
fn overflowing_or_modular_stack_write_ranges_cannot_prove_exclusion() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let records = facts(&[store(0, 10, 0, 2), RET], 0x2000, &memory);
    let mut callee = summary(&records, 0x2000, &memory);
    let code = spilled(&[addi(10, 2, 8), jal(1, 0x2000 - 0x1010)], 2);
    for offset in [
        i64::MAX,
        i64::MIN,
        i64::from(i32::MAX) + 15,
        (1i64 << 32) - 12,
    ] {
        callee.writes[0].address = WriteValue::base(WriteBase::Stack {
            function: 0x2000,
            offset,
        });
        assert_eq!(
            cross_call(&code, &[&callee], &memory).reloads[0].value,
            WriteValue::UNKNOWN
        );
    }
}

#[test]
fn normalized_symbolic_stack_arguments_cannot_bypass_live_input_revalidation() {
    let memory = WorkingMemory::new(8 * 1024 * 1024).unwrap();
    let callee = facts(&[store(0, 10, 0, 2), RET], 0x2000, &memory);
    let callee = summary(&callee, 0x2000, &memory);
    // The write summary recognizes andi a0,a0,-1 as an identity on sp+8;
    // the exact-leaf proof profile must not ignore this symbolic dependency.
    let code = spilled(&[addi(10, 2, 8), 0xfff57513, jal(1, 0x2000 - 0x1014)], 2);
    let before = facts(&code, 0x1000, &memory);
    let calls = stack_calls(
        &index(&before, &memory),
        0x1000,
        ABI,
        &[&callee],
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    for op in [0xfff57513, lw(10, 11, 0)] {
        let mut changed = code.clone();
        changed[4] = op;
        let records = facts_calls(&changed, 0x1000, &memory, true, Some(&calls));
        assert_eq!(
            summary(&records, 0x1000, &memory).reloads[0].value,
            WriteValue::UNKNOWN
        );
    }
}
