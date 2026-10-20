use super::*;

#[test]
fn field_stack_rebasing_preserves_the_original_load_and_expression_owner() {
    let value = WriteValue::Field {
        function: 0x3000,
        read: 0x3004,
        base: WriteBase::Stack {
            function: 0x3000,
            offset: -8,
        },
        displacement: 4,
        width: 2,
        signed: true,
        offset: 3,
    };
    let arguments = [WriteValue::UNKNOWN; 8];
    let tail = substitute(
        value,
        &arguments,
        0x3000,
        WriteValue::base(WriteBase::Stack {
            function: 0x2000,
            offset: 0,
        }),
    );
    let caller = substitute(
        tail,
        &arguments,
        0x2000,
        WriteValue::base(WriteBase::Stack {
            function: 0x1000,
            offset: -16,
        }),
    );
    assert_eq!(
        caller,
        WriteValue::Field {
            function: 0x3000,
            read: 0x3004,
            base: WriteBase::Stack {
                function: 0x1000,
                offset: -24
            },
            displacement: 4,
            width: 2,
            signed: true,
            offset: 3,
        }
    );
    assert!(matches!(
        substitute(value, &arguments, 0x3000, WriteValue::UNKNOWN),
        WriteValue::Field {
            base: WriteBase::Unknown,
            function: 0x3000,
            read: 0x3004,
            ..
        }
    ));
}

#[test]
fn cancellation_and_allocation_failure_release_summary_scratch() {
    let memory = WorkingMemory::new(1024 * 1024).unwrap();
    let records = [FunctionRecord::Expression {
        id: 0,
        offset: u64::MAX,
        expression: Expression::EntryRegister { register: 10 },
    }];
    let facts =
        FunctionIndex::new(&records, RecordPolicy::Canonical, &memory, &mut || Ok(())).unwrap();
    let before = memory.used();
    let mut calls = 0;
    let result = function_writes(
        &facts,
        0,
        Some(CallAbi::RiscvInteger),
        &[],
        &memory,
        &mut || {
            calls += 1;
            if calls == 2 {
                Err(Error::new(ErrorCode::Cancelled, "test cancellation"))
            } else {
                Ok(())
            }
        },
    );
    assert_eq!(result.err().unwrap().code, ErrorCode::Cancelled);
    assert_eq!(memory.used(), before);
    let hold = memory
        .reserve(1024 * 1024 - before, RunPosition::default())
        .unwrap();
    assert_eq!(
        function_writes(
            &facts,
            0,
            Some(CallAbi::RiscvInteger),
            &[],
            &memory,
            &mut || Ok(())
        )
        .err()
        .unwrap()
        .code,
        ErrorCode::ResourceLimited
    );
    drop(hold);
    assert_eq!(memory.used(), before);
}

#[test]
fn prepared_stack_calls_release_memory_on_cancellation_and_failed_admission() {
    let memory = WorkingMemory::new(1024 * 1024).unwrap();
    let records = [FunctionRecord::Transfer {
        offset: 0x1000,
        target: AbstractValue::ImageAddress { address: 0x2000 },
        call: true,
    }];
    let facts =
        FunctionIndex::new(&records, RecordPolicy::Canonical, &memory, &mut || Ok(())).unwrap();
    let callee_records = [];
    let callee_facts = FunctionIndex::new(
        &callee_records,
        RecordPolicy::Canonical,
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    let callee = function_writes(
        &callee_facts,
        0x2000,
        Some(CallAbi::RiscvInteger),
        &[],
        &memory,
        &mut || Ok(()),
    )
    .unwrap();
    let before = memory.used();
    let mut steps = 0;
    stack_calls(
        &facts,
        0x1000,
        Some(CallAbi::RiscvInteger),
        &[&callee],
        &memory,
        &mut || {
            steps += 1;
            Ok(())
        },
    )
    .unwrap();
    for stop in 1..=steps {
        let mut visited = 0;
        let result = stack_calls(
            &facts,
            0x1000,
            Some(CallAbi::RiscvInteger),
            &[&callee],
            &memory,
            &mut || {
                visited += 1;
                if visited == stop {
                    Err(Error::new(ErrorCode::Cancelled, "test cancellation"))
                } else {
                    Ok(())
                }
            },
        );
        assert_eq!(result.err().unwrap().code, ErrorCode::Cancelled);
        assert_eq!(memory.used(), before);
    }
    let hold = memory
        .reserve(
            memory.observation().limit_bytes - before,
            RunPosition::default(),
        )
        .unwrap();
    let result = stack_calls(
        &facts,
        0x1000,
        Some(CallAbi::RiscvInteger),
        &[&callee],
        &memory,
        &mut || Ok(()),
    );
    assert_eq!(result.err().unwrap().code, ErrorCode::ResourceLimited);
    drop(hold);
    assert_eq!(memory.used(), before);
}
