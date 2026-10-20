use super::alignment::branch;
use super::*;

const QUERY: WordQuery = WordQuery {
    destination: 0,
    source: 1,
    length: 2,
    offset: 0,
};

use oer_analysis_riscv_analysis::stack_context::*;

fn numerical_root<'m>(top: u32, root: u64, memory: &'m WorkingMemory) -> StackContext<'m> {
    let code = [
        (top.wrapping_add(0x800) & 0xfffff000) | (2 << 7) | 0x37,
        addi(2, 2, (top << 20) as i32 >> 20),
        jal(1, 0x1000 - root as i32 - 8),
        RET,
    ];
    let records = facts(&code, root, memory);
    call_stack_context(
        &index(&records, memory),
        root,
        root + 8,
        None,
        memory,
        &mut || Ok(()),
    )
    .unwrap()
    .unwrap()
}

fn bounded_copy<'m>(memory: &'m WorkingMemory) -> (Vec<u32>, FunctionWrites<'m>) {
    let body_code = [
        (12 << 20) | (10 << 15) | (13 << 7) | 0x33,
        lw(5, 11, 0),
        store(5, 10, 0, 2),
        addi(10, 10, 4),
        addi(11, 11, 4),
        branch(10, 13, -16, 6),
        RET,
    ];
    let mut code = caller_code();
    code[5] = addi(12, 0, 68);
    code[7] = lw(6, 2, 60); // destination + 44
    let records = facts(&code, 0x1000, memory);
    let generic = summary(&facts(&body_code, 0x3000, memory), 0x3000, memory);
    let tail_records = facts(&[jal(0, 0x1000)], 0x2000, memory);
    let generic_tail = summary_with_callees(&tail_records, 0x2000, &[&generic], memory);
    let context = call_context(
        &index(&records, memory),
        0x1000,
        0x1018,
        ABI,
        &generic_tail,
        memory,
        &mut || Ok(()),
    )
    .unwrap()
    .unwrap();
    let body = specialized_query(
        &body_code,
        &context,
        WordQuery {
            offset: 44,
            ..QUERY
        },
        memory,
        &mut || Ok(()),
    )
    .unwrap();
    let tail = summary_with_callees(&tail_records, 0x2000, &[&body], memory);
    (code, tail)
}

fn facts_in_context<'m>(
    code: &[u32],
    calls: &StackCalls<'_>,
    context: Option<&StackContext<'_>>,
    memory: &'m WorkingMemory,
) -> RecordBuffer<'m> {
    facts_in_capture(code, calls, context, memory, &NoMemory)
}

fn facts_in_capture<'m>(
    code: &[u32],
    calls: &StackCalls<'_>,
    context: Option<&StackContext<'_>>,
    memory: &'m WorkingMemory,
    image: &dyn ImageMemory,
) -> RecordBuffer<'m> {
    let bytes: Vec<_> = code.iter().flat_map(|w| w.to_le_bytes()).collect();
    let mut saved = Saved(RecordBuffer::new(memory));
    research_with_options(
        FunctionInput {
            image: Some(image),
            section: 1,
            extent: CodeRange {
                start: 0x1000,
                length: bytes.len() as u64,
            },
            bytes: &bytes,
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
            abi: ABI,
            symbolic_spills: true,
            stack_calls: Some(calls),
            stack_context: context,
        },
    )
    .unwrap();
    saved.0
}

fn is_call_word(records: &[FunctionRecord], memory: &WorkingMemory) -> bool {
    let AbstractValue::Expression { id } = loaded(records) else {
        return false;
    };
    matches!(
        index(records, memory)
            .expression(*id, &mut || Ok(()))
            .unwrap(),
        Some(Expression::CallWord { .. })
    )
}

#[test]
fn checked_stack_bounds_discharge_the_word_premise_and_require_a_live_context() {
    let memory = WorkingMemory::new(64 * 1024 * 1024).unwrap();
    let stack = numerical_root(0x50040000, 0x800, &memory);
    assert_eq!(stack.owner(), 0x1000);
    assert_eq!(stack.interval(), (0x50040000, 0x50040000));
    assert_eq!(stack.origins()[0].site, 0x808);
    let (code, tail) = bounded_copy(&memory);
    let caller = facts(&code, 0x1000, &memory);
    let idx = index(&caller, &memory);
    let misaligned = numerical_root(0x50040001, 0x900, &memory);
    assert_eq!(
        call_word_with_stack(
            &idx,
            0x1000,
            0x1018,
            ABI,
            &tail,
            Some(&misaligned),
            &memory,
            &mut || Ok(())
        )
        .unwrap()
        .unwrap_err(),
        WordBarrier::Alignment
    );
    assert!(
        call_word_with_stack(
            &idx,
            0x1000,
            0x1018,
            ABI,
            &tail,
            Some(&stack),
            &memory,
            &mut || Ok(())
        )
        .unwrap()
        .is_ok()
    );
    assert_eq!(
        call_word(&idx, 0x1000, 0x1018, ABI, &tail, &memory, &mut || Ok(()))
            .unwrap()
            .unwrap_err(),
        WordBarrier::DestinationBounds
    );
    let calls = stack_calls_with_stack(
        &idx,
        0x1000,
        ABI,
        &[&tail],
        Some(&stack),
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    assert!(is_call_word(
        &facts_in_context(&code, &calls, Some(&stack), &memory),
        &memory
    ));
    assert!(!is_call_word(
        &facts_in_context(&code, &calls, None, &memory),
        &memory
    ));
    // A newly checked startup, even at the same addresses, is a new scope.
    let current = numerical_root(0x50050000, 0x800, &memory);
    assert!(!is_call_word(
        &facts_in_context(&code, &calls, Some(&current), &memory),
        &memory
    ));
    struct FreshImage;
    impl ImageMemory for FreshImage {
        fn capture_id(&self) -> Option<u64> {
            Some(2)
        }
        fn read_constant(&self, _: u32, _: &mut [u8], _: &mut dyn RunControl) -> Result<bool> {
            Ok(false)
        }
    }
    // Even passing the old context object explicitly cannot cross an image
    // capture boundary. Its ancestor setup may have changed in the new ELF.
    assert!(!is_call_word(
        &facts_in_capture(&code, &calls, Some(&stack), &memory, &FreshImage),
        &memory
    ));
    // Changed local bytes cannot reuse the old all-incoming-path evidence.
    let mut changed = code.clone();
    changed[0] = addi(2, 2, -48);
    changed[9] = addi(2, 2, 48);
    assert!(!is_call_word(
        &facts_in_context(&changed, &calls, Some(&stack), &memory),
        &memory
    ));
    changed = code.clone();
    changed[5] = addi(12, 0, 64);
    assert!(!is_call_word(
        &facts_in_context(&changed, &calls, Some(&stack), &memory),
        &memory
    ));
}

#[test]
fn every_stack_context_and_every_incoming_sp_participates_in_bounds() {
    let memory = WorkingMemory::new(64 * 1024 * 1024).unwrap();
    let low = numerical_root(0x4000, 0x800, &memory);
    let high = numerical_root(0x50040000, 0x900, &memory);
    let merged = merge_stack_contexts(&[Some(&low), Some(&high)], &memory, &mut || Ok(()))
        .unwrap()
        .unwrap();
    assert_eq!(merged.interval(), (0x4000, 0x50040000));
    assert_eq!(merged.origins().len(), 2);
    assert!(
        merge_stack_contexts(&[Some(&low), None], &memory, &mut || Ok(()))
            .unwrap()
            .is_none()
    );
    let (code, tail) = bounded_copy(&memory);
    let caller = facts(&code, 0x1000, &memory);
    assert!(
        call_word_with_stack(
            &index(&caller, &memory),
            0x1000,
            0x1018,
            ABI,
            &tail,
            Some(&merged),
            &memory,
            &mut || Ok(())
        )
        .unwrap()
        .is_ok()
    );
    // D = entry-sp - 16 = 0xffffffc0 must remain excluded.
    let wrap = numerical_root(0xffffffd0, 0xa00, &memory);
    let merged = merge_stack_contexts(&[Some(&low), Some(&wrap)], &memory, &mut || Ok(()))
        .unwrap()
        .unwrap();
    assert_eq!(
        call_word_with_stack(
            &index(&caller, &memory),
            0x1000,
            0x1018,
            ABI,
            &tail,
            Some(&merged),
            &memory,
            &mut || Ok(())
        )
        .unwrap()
        .unwrap_err(),
        WordBarrier::DestinationBounds
    );
    let inputs = facts(
        &[
            branch(10, 0, 12, 0),
            addi(2, 2, -32),
            jal(0, 8),
            addi(2, 2, -48),
            jal(1, 0x2000 - 0x1010),
            RET,
        ],
        0x1000,
        &memory,
    );
    let joined = call_stack_context(
        &index(&inputs, &memory),
        0x1000,
        0x1010,
        Some(&low),
        &memory,
        &mut || Ok(()),
    )
    .unwrap()
    .unwrap();
    assert_eq!(joined.interval(), (0x4000 - 48, 0x4000 - 32));
}

#[test]
fn stack_transfers_fail_closed_on_unknown_control_and_rv32_overflow() {
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    let low = numerical_root(16, 0x800, &memory);
    let high = numerical_root(0xfffffff0, 0x900, &memory);
    for (code, parent) in [
        (vec![addi(2, 2, -32), jal(1, 0x2000 - 0x1004), RET], &low),
        (vec![addi(2, 2, 32), jal(0, 0x2000 - 0x1004)], &high),
        (
            vec![
                addi(2, 2, -32),
                addi(2, 2, 32),
                jal(1, 0x2000 - 0x1008),
                RET,
            ],
            &low,
        ),
        (
            vec![
                branch(10, 0, 8, 0),
                0x00070067,
                jal(1, 0x2000 - 0x1008),
                RET,
            ],
            &low,
        ),
    ] {
        let caller = facts(&code, 0x1000, &memory);
        let site = if code.len() == 4 { 0x1008 } else { 0x1004 };
        assert!(
            call_stack_context(
                &index(&caller, &memory),
                0x1000,
                site,
                Some(parent),
                &memory,
                &mut || Ok(())
            )
            .unwrap()
            .is_none()
        );
    }
    // A machine stack switch replaces the old coordinate with its new value.
    let records = facts(
        &[addi(2, 0, 1024), jal(0, 0x2000 - 0x1004)],
        0x1000,
        &memory,
    );
    let switched = call_stack_context(
        &index(&records, &memory),
        0x1000,
        0x1004,
        Some(&high),
        &memory,
        &mut || Ok(()),
    )
    .unwrap()
    .unwrap();
    assert_eq!(switched.interval(), (1024, 1024));
    assert!(
        call_stack_context(
            &index(&records, &memory),
            0x9000,
            0x1004,
            Some(&high),
            &memory,
            &mut || Ok(())
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn startup_terminal_refinement_needs_an_external_all_path_target() {
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    // fence.i preserves the entry sp; the terminal's all-path value comes
    // from the common evaluator, including the architectural upper write.
    let records = facts(&[0x0000100f, 0x00001317, 0x00030067], 0x1000, &memory);
    assert_eq!(
        external_terminal(
            &records,
            CodeRange {
                start: 0x1000,
                length: 12
            },
            &mut || Ok(())
        )
        .unwrap(),
        Some((0x1008, 0x2004))
    );
    let idx = index(&records, &memory);
    assert_eq!(
        idx.call_inputs(0x1008, &mut || Ok(()))
            .unwrap()
            .unwrap()
            .registers[2],
        AbstractValue::EntryStack { offset: 0 }
    );
    for code in [
        vec![branch(10, 0, 8, 0), 0x00001317, 0x00030067], // bypasses producer
        vec![0x00000317, 0x00030067],                      // target is local
        vec![branch(10, 0, 8, 0), 0x00070067, 0x00001317, 0x00030067], // another unknown edge
    ] {
        let records = facts(&code, 0x1000, &memory);
        assert!(
            external_terminal(
                &records,
                CodeRange {
                    start: 0x1000,
                    length: code.len() as u64 * 4
                },
                &mut || Ok(())
            )
            .unwrap()
            .is_none()
        );
    }
}

#[test]
fn numerical_context_allocations_release_on_cancellation_and_refused_admission() {
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    let root = numerical_root(0x50040000, 0x800, &memory);
    let records = facts(
        &[addi(2, 2, -32), jal(1, 0x2000 - 0x1004), RET],
        0x1000,
        &memory,
    );
    let idx = index(&records, &memory);
    let baseline = memory.used();
    let mut steps = 0;
    drop(
        call_stack_context(&idx, 0x1000, 0x1004, Some(&root), &memory, &mut || {
            steps += 1;
            Ok(())
        })
        .unwrap(),
    );
    assert_eq!(memory.used(), baseline);
    for stop in 1..=steps {
        let mut visited = 0;
        let result = call_stack_context(&idx, 0x1000, 0x1004, Some(&root), &memory, &mut || {
            visited += 1;
            if visited == stop {
                Err(Error::new(ErrorCode::Cancelled, "cancel numerical context"))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        drop(result);
        assert_eq!(memory.used(), baseline);
    }
    let limited = WorkingMemory::new(1).unwrap();
    assert!(
        call_stack_context(&idx, 0x1000, 0x1004, Some(&root), &limited, &mut || Ok(())).is_err()
    );
    assert_eq!(limited.used(), 0);
}

#[test]
fn stack_installation_checks_both_machine_values_and_the_return_predecessor() {
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    let code = [addi(2, 0, 1024), addi(5, 0, 512), 0x34029073, RET];
    let records = facts(&code, 0x4000, &memory);
    let setup = stack_setup(
        &index(&records, &memory),
        0x4000,
        &RiscvDecoder,
        &mut || Ok(()),
    )
    .unwrap()
    .unwrap();
    assert_eq!((setup.pointer(), setup.scratch()), (1024, 512));
    let changed = facts(
        &[addi(2, 0, 768), addi(5, 0, 256), 0x34029073, RET],
        0x4000,
        &memory,
    );
    let changed = stack_setup(
        &index(&changed, &memory),
        0x4000,
        &RiscvDecoder,
        &mut || Ok(()),
    )
    .unwrap()
    .unwrap();
    assert_eq!((changed.pointer(), changed.scratch()), (768, 256));
    let bypass = facts(
        &[
            addi(2, 0, 1024),
            addi(5, 0, 512),
            branch(10, 0, 8, 0),
            0x34029073,
            RET,
        ],
        0x4000,
        &memory,
    );
    assert!(
        stack_setup(&index(&bypass, &memory), 0x4000, &RiscvDecoder, &mut || Ok(
            ()
        ))
        .unwrap()
        .is_none()
    );
    let startup = facts(
        &[
            jal(1, 0x4000 - 0x5000),
            addi(5, 0, 0),
            jal(0, 0x1000 - 0x5008),
        ],
        0x5000,
        &memory,
    );
    let resumed = resume_stack_context(
        &setup,
        &index(&startup, &memory),
        0x5000,
        0x5000,
        &memory,
        &mut || Ok(()),
    )
    .unwrap()
    .unwrap();
    let root = call_stack_context(
        &index(&startup, &memory),
        0x5000,
        0x5008,
        Some(&resumed),
        &memory,
        &mut || Ok(()),
    )
    .unwrap()
    .unwrap();
    assert_eq!(root.interval(), (1024, 1024));
}

#[test]
fn resolved_contexts_keep_the_existing_executor_assumption() {
    let memory = WorkingMemory::new(64 * 1024 * 1024).unwrap();
    let root = numerical_root(0x50040000, 0x800, &memory);
    let executor = facts(&[0x000500e7, RET], 0x1000, &memory);
    let idx = index(&executor, &memory);
    assert!(
        call_stack_context(&idx, 0x1000, 0x1000, Some(&root), &memory, &mut || Ok(()))
            .unwrap()
            .is_none()
    );
    let mut targets = Resolutions::new();
    targets.add(0x1000, Fact::TaskPolls, [0x1000]);
    let context = call_stack_context_with_targets(
        &idx,
        0x1000,
        0x1000,
        Some(&root),
        Some(&targets),
        Some(0x1000),
        &memory,
        &mut || Ok(()),
    )
    .unwrap()
    .unwrap();
    assert_eq!(context.assumptions(), &[Assumption::ExecutorInvariant]);
    let (code, tail) = bounded_copy(&memory);
    let caller = facts(&code, 0x1000, &memory);
    let application = call_word_with_stack(
        &index(&caller, &memory),
        0x1000,
        0x1018,
        ABI,
        &tail,
        Some(&context),
        &memory,
        &mut || Ok(()),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        application.stack_assumption,
        Some(Assumption::ExecutorInvariant)
    );
    let calls = stack_calls_with_stack(
        &index(&caller, &memory),
        0x1000,
        ABI,
        &[&tail],
        Some(&context),
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    let records = facts_in_context(&code, &calls, Some(&context), &memory);
    let AbstractValue::Expression { id } = loaded(&records) else {
        panic!("missing call word");
    };
    assert!(matches!(
        index(&records, &memory)
            .expression(*id, &mut || Ok(()))
            .unwrap(),
        Some(Expression::CallWord {
            stack_assumption: Some(Assumption::ExecutorInvariant),
            ..
        })
    ));
}

fn caller_code() -> Vec<u32> {
    vec![
        addi(2, 2, -32),
        addi(5, 0, 77),
        store(5, 2, 8, 2),
        addi(10, 2, 16),
        addi(11, 2, 8),
        addi(12, 0, 4),
        jal(1, 0x2000 - 0x1018),
        lw(6, 2, 16),
        store(6, 2, 20, 2),
        addi(2, 2, 32),
        RET,
    ]
}

fn specialized<'m>(
    code: &[u32],
    context: &CallContext,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<FunctionWrites<'m>> {
    specialized_query(code, context, QUERY, memory, c)
}
fn specialized_query<'m>(
    code: &[u32],
    context: &CallContext,
    query: WordQuery,
    memory: &'m WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<FunctionWrites<'m>> {
    let bytes: Vec<_> = code.iter().flat_map(|word| word.to_le_bytes()).collect();
    contextual_word_function_writes(
        FunctionInput {
            image: Some(&NoMemory),
            section: 1,
            extent: CodeRange {
                start: 0x3000,
                length: bytes.len() as u64,
            },
            bytes: &bytes,
            relocations: &PreparedReferences::empty(),
            data_ranges: &[],
            jumps: &[],
            returns: &NoReturns,
            call_behavior: &UnknownCallBehavior,
        },
        &RiscvDecoder,
        context,
        query,
        &[],
        memory,
        c,
    )
}

fn context(memory: &WorkingMemory) -> CallContext {
    let body = summary(
        &facts(&[lw(5, 11, 0), store(5, 10, 0, 2), RET], 0x3000, memory),
        0x3000,
        memory,
    );
    let tail = summary_with_callees(
        &facts(&[jal(0, 0x1000)], 0x2000, memory),
        0x2000,
        &[&body],
        memory,
    );
    let caller = facts(&caller_code(), 0x1000, memory);
    call_context(
        &index(&caller, memory),
        0x1000,
        0x1018,
        ABI,
        &tail,
        memory,
        &mut || Ok(()),
    )
    .unwrap()
    .unwrap()
}

fn loaded(records: &[FunctionRecord]) -> &AbstractValue {
    records
        .iter()
        .find_map(|record| match record {
            FunctionRecord::Value {
                register: 6, value, ..
            } => Some(value),
            _ => None,
        })
        .unwrap()
}

#[test]
fn one_word_postcondition_crosses_tail_and_forwards_store_call_load() {
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    let code = [lw(5, 11, 0), store(5, 10, 0, 2), RET];
    let body = specialized(&code, &context(&memory), &memory, &mut || Ok(())).unwrap();
    let proof = body.word_postcondition().unwrap();
    assert_eq!(proof.contents().load, 0x3000);
    assert_eq!(proof.contents().store, 0x3004);
    assert!(proof.contents().load_order < proof.contents().store_order);
    assert_eq!(proof.contents().width, 4);
    assert!(proof.contents().source_unchanged_before_load);
    assert_eq!(proof.destination_bounds(), None);
    let generic = summary(&facts(&code, 0x3000, &memory), 0x3000, &memory);
    assert_eq!(body.writes.as_ref(), generic.writes.as_ref());
    assert_eq!(body.barriers.as_ref(), generic.barriers.as_ref());
    let tail = summary_with_callees(
        &facts(&[jal(0, 0x1000)], 0x2000, &memory),
        0x2000,
        &[&body],
        &memory,
    );
    assert_eq!(
        tail.word_postcondition()
            .unwrap()
            .contents()
            .origin
            .tail
            .unwrap()
            .target,
        0x3000
    );
    let caller = facts(&caller_code(), 0x1000, &memory);
    let applied = call_word(
        &index(&caller, &memory),
        0x1000,
        0x1018,
        ABI,
        &tail,
        &memory,
        &mut || Ok(()),
    )
    .unwrap()
    .unwrap();
    assert_eq!(
        applied.contents.destination,
        WriteValue::base(WriteBase::Stack {
            function: 0x1000,
            offset: -16
        })
    );
    let calls = stack_calls(
        &index(&caller, &memory),
        0x1000,
        ABI,
        &[&tail],
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    let after = facts_calls(&caller_code(), 0x1000, &memory, true, Some(&calls));
    assert_eq!(loaded(&after), &AbstractValue::Constant { value: 77 });
}

#[test]
fn callee_load_time_is_not_its_entry_memory() {
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    // a3 may alias the source, even though it is a different symbolic base.
    let body = specialized(
        &[store(0, 13, 0, 2), lw(5, 11, 0), store(5, 10, 0, 2), RET],
        &context(&memory),
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    assert!(
        !body
            .word_postcondition()
            .unwrap()
            .contents()
            .source_unchanged_before_load
    );
    let tail = summary_with_callees(
        &facts(&[jal(0, 0x1000)], 0x2000, &memory),
        0x2000,
        &[&body],
        &memory,
    );
    let caller = facts(&caller_code(), 0x1000, &memory);
    let calls = stack_calls(
        &index(&caller, &memory),
        0x1000,
        ABI,
        &[&tail],
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    let after = facts_calls(&caller_code(), 0x1000, &memory, true, Some(&calls));
    let AbstractValue::Expression { id } = loaded(&after) else {
        panic!("missing captured load-time word");
    };
    assert!(matches!(
        index(&after, &memory)
            .expression(*id, &mut || Ok(()))
            .unwrap(),
        Some(Expression::CallWord {
            owner: 0x3000,
            load: 0x3004,
            width: 4,
            ..
        })
    ));
}

#[test]
fn later_caller_writes_and_merges_invalidate_a_returned_word() {
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    let body = specialized(
        &[lw(5, 11, 0), store(5, 10, 0, 2), RET],
        &context(&memory),
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    let tail = summary_with_callees(
        &facts(&[jal(0, 0x1000)], 0x2000, &memory),
        0x2000,
        &[&body],
        &memory,
    );
    let caller = facts(&caller_code(), 0x1000, &memory);
    let calls = stack_calls(
        &index(&caller, &memory),
        0x1000,
        ABI,
        &[&tail],
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    for (between, known) in [
        (vec![store(0, 2, 20, 2)], true),
        (vec![store(0, 2, 17, 0)], false),
        (vec![store(0, 13, 0, 2)], false),
        (vec![branch(13, 0, 8, 0), store(0, 2, 16, 2)], false),
    ] {
        let mut code = caller_code();
        code.splice(7..7, between);
        let records = facts_calls(&code, 0x1000, &memory, true, Some(&calls));
        assert_eq!(
            loaded(&records) == &AbstractValue::Constant { value: 77 },
            known
        );
    }
}

#[test]
fn all_returns_and_all_overlapping_writers_must_agree() {
    let memory = WorkingMemory::new(64 * 1024 * 1024).unwrap();
    let context = context(&memory);
    let cases = [
        vec![lw(5, 11, 0), branch(13, 0, 8, 0), store(5, 10, 0, 2), RET],
        vec![lw(5, 11, 0), store(5, 10, 0, 2), store(0, 10, 1, 0), RET],
        vec![lw(5, 11, 0), store(5, 10, 0, 2), store(0, 13, 0, 2), RET],
        vec![
            (lw(5, 11, 0) & !(7 << 12)) | (1 << 12),
            store(5, 10, 0, 2),
            RET,
        ],
        vec![store(5, 10, 0, 2), lw(5, 11, 0), RET],
        vec![lw(5, 11, 0), store(5, 10, 0, 2), jal(1, 0x1000), RET],
        vec![
            branch(13, 0, 16, 0),
            lw(5, 11, 0),
            store(5, 10, 0, 2),
            RET,
            lw(5, 11, 0),
            store(5, 10, 0, 2),
            RET,
        ],
        vec![lw(5, 11, 0), store(5, 10, 0, 2), 0xffffffff, RET],
    ];
    for code in cases {
        assert!(
            specialized(&code, &context, &memory, &mut || Ok(()))
                .unwrap()
                .word_postcondition()
                .is_none(),
            "{code:x?}"
        );
    }
}

#[test]
fn prepared_word_rechecks_live_arguments_target_and_abi() {
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    let body = specialized(
        &[lw(5, 11, 0), store(5, 10, 0, 2), RET],
        &context(&memory),
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    let tail = summary_with_callees(
        &facts(&[jal(0, 0x1000)], 0x2000, &memory),
        0x2000,
        &[&body],
        &memory,
    );
    let caller = facts(&caller_code(), 0x1000, &memory);
    let caller_index = index(&caller, &memory);
    assert_eq!(
        call_word(
            &caller_index,
            0x1000,
            0x1018,
            None,
            &tail,
            &memory,
            &mut || Ok(())
        )
        .unwrap()
        .unwrap_err(),
        WordBarrier::Abi
    );
    let calls = stack_calls(
        &caller_index,
        0x1000,
        ABI,
        &[&tail],
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    for (at, replacement) in [
        (3, addi(10, 2, 12)),
        (4, addi(11, 2, 4)),
        (5, addi(12, 0, 8)),
        (6, jal(1, 0x2100 - 0x1018)),
    ] {
        let mut changed = caller_code();
        changed[at] = replacement;
        assert_ne!(
            loaded(&facts_calls(&changed, 0x1000, &memory, true, Some(&calls))),
            &AbstractValue::Constant { value: 77 }
        );
    }
}

#[test]
fn exact_affine_words_join_only_agreed_inputs_and_kill_unknown_aliases() {
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    let mut prefix = vec![addi(5, 0, 77), store(5, 10, 8, 2)];
    for (between, known) in [
        (vec![store(0, 10, 12, 2)], true),
        (vec![store(0, 10, 9, 0)], false),
        (vec![store(0, 11, 8, 2)], false),
        (vec![branch(13, 0, 8, 0), store(0, 10, 8, 2)], false),
        (vec![jal(1, 0x1000)], false),
    ] {
        let mut code = prefix.clone();
        code.extend(between);
        let pc = 0x1000 + code.len() as u64 * 4;
        code.extend([lw(6, 10, 8), RET]);
        let records = facts(&code, 0x1000, &memory);
        let value = records
            .iter()
            .find_map(|r| match r {
                FunctionRecord::Value {
                    offset,
                    register: 6,
                    value,
                    ..
                } if *offset == pc => Some(value),
                _ => None,
            })
            .unwrap();
        assert_eq!(value == &AbstractValue::Constant { value: 77 }, known);
    }
    prefix.extend([branch(13, 0, 8, 0), store(5, 10, 8, 2), lw(6, 10, 8), RET]);
    let records = facts(&prefix, 0x1000, &memory);
    assert!(records.iter().any(|r| matches!(
        r,
        FunctionRecord::Value {
            register: 6,
            value: AbstractValue::Constant { value: 77 },
            ..
        }
    )));
    // RV32 byte overlap is modular, even when offsets straddle zero.
    let records = facts(
        &[
            addi(5, 0, 77),
            store(5, 10, -1, 2),
            store(0, 10, 0, 0),
            lw(6, 10, -1),
            RET,
        ],
        0x1000,
        &memory,
    );
    assert_ne!(loaded(&records), &AbstractValue::Constant { value: 77 });
}

#[test]
fn postcondition_cancellation_and_failed_admission_release_every_allocation() {
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    let context = context(&memory);
    let code = [lw(5, 11, 0), store(5, 10, 0, 2), RET];
    let mut steps = 0;
    drop(
        specialized(&code, &context, &memory, &mut || {
            steps += 1;
            Ok(())
        })
        .unwrap(),
    );
    assert_eq!(memory.used(), 0);
    for stop in 1..=steps {
        let mut visited = 0;
        let result = specialized(&code, &context, &memory, &mut || {
            visited += 1;
            if visited == stop {
                Err(Error::new(ErrorCode::Cancelled, "cancel word proof"))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        drop(result);
        assert_eq!(memory.used(), 0, "cancel at {stop}");
    }
    for budget in [1, 128, 1024, 4096, 16384] {
        let limited = WorkingMemory::new(budget).unwrap();
        drop(specialized(&code, &context, &limited, &mut || Ok(())));
        assert_eq!(limited.used(), 0, "budget {budget}");
    }
}

#[test]
fn unsigned_loop_bounds_are_explicit_and_stack_coordinates_do_not_prove_them() {
    let memory = WorkingMemory::new(64 * 1024 * 1024).unwrap();
    // end = destination + length; copy a word, then unsigned pointer guard.
    let body_code = [
        (12 << 20) | (10 << 15) | (13 << 7) | 0x33,
        lw(5, 11, 0),
        store(5, 10, 0, 2),
        addi(10, 10, 4),
        addi(11, 11, 4),
        branch(10, 13, -16, 6),
        RET,
    ];
    let mut code = caller_code();
    code[5] = addi(12, 0, 68);
    let caller = facts(&code, 0x1000, &memory);
    let generic = summary(&facts(&body_code, 0x3000, &memory), 0x3000, &memory);
    let tail_records = facts(&[jal(0, 0x1000)], 0x2000, &memory);
    let generic_tail = summary_with_callees(&tail_records, 0x2000, &[&generic], &memory);
    let context = call_context(
        &index(&caller, &memory),
        0x1000,
        0x1018,
        ABI,
        &generic_tail,
        &memory,
        &mut || Ok(()),
    )
    .unwrap()
    .unwrap();
    let query = WordQuery {
        offset: 44,
        ..QUERY
    };
    let body = specialized_query(&body_code, &context, query, &memory, &mut || Ok(())).unwrap();
    assert_eq!(
        body.word_postcondition().unwrap().destination_bounds(),
        Some((0, u32::MAX - 68))
    );
    let tail = summary_with_callees(&tail_records, 0x2000, &[&body], &memory);
    assert_eq!(
        call_word(
            &index(&caller, &memory),
            0x1000,
            0x1018,
            ABI,
            &tail,
            &memory,
            &mut || Ok(())
        )
        .unwrap()
        .unwrap_err(),
        WordBarrier::DestinationBounds
    );
    for (destination, expected) in [
        (256, None),
        (257, Some(WordBarrier::Alignment)),
        (-64, Some(WordBarrier::DestinationBounds)),
    ] {
        let mut changed = code.clone();
        changed[3] = addi(10, 0, destination);
        let records = facts(&changed, 0x1000, &memory);
        assert_eq!(
            call_word(
                &index(&records, &memory),
                0x1000,
                0x1018,
                ABI,
                &tail,
                &memory,
                &mut || Ok(())
            )
            .unwrap()
            .err(),
            expected
        );
        // Under the wrapping concrete input, not even the conditional proof
        // generator can produce a word: this execution returns before offset 44.
        if destination == -64 {
            let wrapping_context = call_context(
                &index(&records, &memory),
                0x1000,
                0x1018,
                ABI,
                &generic_tail,
                &memory,
                &mut || Ok(()),
            )
            .unwrap()
            .unwrap();
            assert!(
                specialized_query(
                    &body_code,
                    &wrapping_context,
                    query,
                    &memory,
                    &mut || Ok(())
                )
                .unwrap()
                .word_postcondition()
                .is_none()
            );
        }
    }
}

#[test]
fn prepared_word_application_releases_admitted_storage_on_every_cancel() {
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    let body = specialized(
        &[lw(5, 11, 0), store(5, 10, 0, 2), RET],
        &context(&memory),
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    let tail = summary_with_callees(
        &facts(&[jal(0, 0x1000)], 0x2000, &memory),
        0x2000,
        &[&body],
        &memory,
    );
    let caller = facts(&caller_code(), 0x1000, &memory);
    let caller_index = index(&caller, &memory);
    let before = memory.used();
    let mut steps = 0;
    drop(
        stack_calls(&caller_index, 0x1000, ABI, &[&tail], &memory, &mut || {
            steps += 1;
            Ok(())
        })
        .unwrap(),
    );
    for stop in 1..=steps {
        let mut visited = 0;
        let result = stack_calls(&caller_index, 0x1000, ABI, &[&tail], &memory, &mut || {
            visited += 1;
            if visited == stop {
                Err(Error::new(ErrorCode::Cancelled, "cancel prepared word"))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        drop(result);
        assert_eq!(memory.used(), before);
    }
    let tail_records = facts(&[jal(0, 0x1000)], 0x2000, &memory);
    let tail_index = index(&tail_records, &memory);
    let before = memory.used();
    let mut steps = 0;
    drop(
        function_writes(&tail_index, 0x2000, ABI, &[&body], &memory, &mut || {
            steps += 1;
            Ok(())
        })
        .unwrap(),
    );
    for stop in 1..=steps {
        let mut visited = 0;
        let result = function_writes(&tail_index, 0x2000, ABI, &[&body], &memory, &mut || {
            visited += 1;
            if visited == stop {
                Err(Error::new(ErrorCode::Cancelled, "cancel word tail trace"))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        drop(result);
        assert_eq!(memory.used(), before);
    }
}

#[test]
fn evaluating_a_load_time_word_releases_memory_at_every_cancelled_step() {
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    let body = specialized(
        &[store(0, 13, 0, 2), lw(5, 11, 0), store(5, 10, 0, 2), RET],
        &context(&memory),
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    let tail = summary_with_callees(
        &facts(&[jal(0, 0x1000)], 0x2000, &memory),
        0x2000,
        &[&body],
        &memory,
    );
    let code = caller_code();
    let caller = facts(&code, 0x1000, &memory);
    let calls = stack_calls(
        &index(&caller, &memory),
        0x1000,
        ABI,
        &[&tail],
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    let before = memory.used();
    let mut steps = 0;
    drop(
        try_facts_calls(&code, 0x1000, &memory, true, Some(&calls), &mut || {
            steps += 1;
            Ok(())
        })
        .unwrap(),
    );
    for stop in 1..=steps {
        let mut visited = 0;
        let result = try_facts_calls(&code, 0x1000, &memory, true, Some(&calls), &mut || {
            visited += 1;
            if visited == stop {
                Err(Error::new(
                    ErrorCode::Cancelled,
                    "cancel load-time word evaluation",
                ))
            } else {
                Ok(())
            }
        });
        assert!(result.is_err());
        drop(result);
        assert_eq!(memory.used(), before);
    }
}

#[test]
fn cached_word_rejects_a_live_unknown_incoming_edge() {
    let memory = WorkingMemory::new(32 * 1024 * 1024).unwrap();
    let body = specialized(
        &[lw(5, 11, 0), store(5, 10, 0, 2), RET],
        &context(&memory),
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    let tail = summary_with_callees(
        &facts(&[jal(0, 0x1000)], 0x2000, &memory),
        0x2000,
        &[&body],
        &memory,
    );
    let baseline = facts(&caller_code(), 0x1000, &memory);
    let calls = stack_calls(
        &index(&baseline, &memory),
        0x1000,
        ABI,
        &[&tail],
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    assert_eq!(
        loaded(&facts_calls(
            &caller_code(),
            0x1000,
            &memory,
            true,
            Some(&calls)
        )),
        &AbstractValue::Constant { value: 77 },
    );
    let mut changed = caller_code();
    // The other branch can jump straight to the call at 0x1018, skipping
    // source initialization. Its target and argument entry values can match
    // those on the modeled path, while the source still contains 77.
    changed[1] = branch(13, 0, 40, 0);
    changed[2] = store(0, 2, 8, 2);
    changed.push(0x00070067); // jalr zero, 0(a4)
    let unassisted = facts(&changed, 0x1000, &memory);
    assert_eq!(
        call_word(
            &index(&unassisted, &memory),
            0x1000,
            0x1018,
            ABI,
            &tail,
            &memory,
            &mut || Ok(()),
        )
        .unwrap()
        .unwrap_err(),
        WordBarrier::MissingProof,
    );
    let live = facts_calls(&changed, 0x1000, &memory, true, Some(&calls));
    assert!(
        live.iter().any(|record| matches!(
            record,
            FunctionRecord::Edge {
                target: None,
                relation: EdgeKind::Indirect,
                ..
            }
        )),
        "fixture must retain an unproved incoming transfer"
    );
    assert_ne!(loaded(&live), &AbstractValue::Constant { value: 0 });
}
