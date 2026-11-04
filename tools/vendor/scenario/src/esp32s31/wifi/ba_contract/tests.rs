use super::*;

fn fixture(expected: Expected) -> Case {
    let bank = Bank {
        control: 0x100,
        sequence: 0x104,
        low: 0x108,
        high: 0x10c,
        valid: 1,
    };
    case(
        "contract".into(),
        invocation(
            0x1000,
            bank,
            true,
            vec![],
            MemorySeed {
                address: STATE,
                length: 40,
                fill: Some(0),
                bytes: vec![],
            },
        ),
        bank,
        expected,
        None,
    )
}

fn events(writes: &[(u32, u32)], stop: ExecutionStop) -> Vec<ExecutionEvidence> {
    let mut result: Vec<_> = writes
        .iter()
        .map(|(address, value)| ExecutionEvidence::Event {
            case: 0,
            replacement: false,
            event: ExecutionEvent::Write {
                address: *address,
                width: 4,
                value: *value,
            },
        })
        .collect();
    result.push(ExecutionEvidence::Outcome {
        case: 0,
        replacement: false,
        stop,
        steps: 20,
    });
    result
}

#[test]
fn a_completed_call_must_publish_the_selected_sequence_and_both_bitmaps() {
    let case = fixture(Expected::Loaded {
        sequence: 100,
        low: 2,
        high: 3,
    });
    let mut writes = [
        (case.bank.control, 0),
        (case.bank.low, 2),
        (case.bank.high, 3),
        (case.bank.sequence, 100),
        (case.bank.control, 1),
    ];
    let stop = ExecutionStop::Returned {
        low: None,
        high: None,
    };
    assert!(check(&case, 0, &events(&writes, stop.clone())).is_ok());
    writes.get_mut(3).expect("sequence write").1 = 0;
    assert!(check(&case, 0, &events(&writes, stop)).is_err());
}

#[test]
fn an_expected_unknown_stop_is_retained_and_cannot_replace_a_completed_call() {
    let case = fixture(Expected::UnknownHigh);
    let stop = ExecutionStop::Incomplete {
        pc: 0x1020,
        reason: ExecutionGap::UnknownRegister { register: 13 },
    };
    let records = events(&[(case.bank.control, 0), (case.bank.low, 0)], stop.clone());
    let observation = check(&case, 0, &records).expect("reviewed unknown-data boundary");
    assert_eq!(observation.stop, stop);
    assert!(!observation.stop.completed());
    assert!(check(&fixture(Expected::Inactive), 0, &records).is_err());
}
