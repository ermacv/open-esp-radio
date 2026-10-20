use super::alignment::branch;
use super::*;

const QUERY: WordQuery = WordQuery {
    destination: 0,
    source: 1,
    length: 2,
    offset: 0,
};

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
