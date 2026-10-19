//! Structural timeline admission; physical execution remains the application/backend's authority.
use super::*;
pub(super) fn validate(input: &Invocation, event: &ExecutionEvent) -> Result<()> {
    let pc_valid = |pc: u32| pc & 1 == 0 && pc < u32::MAX - 1;
    match event {
        ExecutionEvent::Memory { site, transaction } => {
            if !pc_valid(*site)
                || !input.observe_timeline.selects(transaction)
                || matches!(transaction, MemoryTransaction::InitializeZeroed { .. })
            {
                return Err(integrity(
                    "unrequested memory timeline or invalid instruction identity",
                ));
            }
            transaction.validate()?;
        }
        ExecutionEvent::Branch {
            site,
            target,
            fallthrough,
            ..
        } if !input.observe_timeline.branches
            || !pc_valid(*site)
            || target & 1 != 0
            || !pc_valid(*fallthrough)
            || !matches!(fallthrough.checked_sub(*site), Some(2 | 4)) =>
        {
            return Err(integrity(
                "unrequested or invalid conditional branch observation",
            ));
        }
        _ => (),
    }
    Ok(())
}
#[cfg(test)]
pub(super) mod tests {
    use super::*;
    pub(in crate::records) fn input() -> Invocation {
        Invocation {
            entry: 0x1000,
            goal: ExecutionGoal::Return,
            arguments: vec![],
            memory: vec![],
            preload: vec![],
            models: vec![],
            calls: vec![],
            observe_memory: vec![],
            observe_calls: None,
            observe_timeline: TimelineCapture {
                reads: true,
                writes: true,
                atomics: true,
                branches: true,
                written: false,
            },
        }
    }
    #[test]
    fn unrequested_and_malformed_physical_timeline_records_are_rejected() {
        let mut input = input();
        let event = |transaction| ExecutionEvent::Memory {
            site: 0x1000,
            transaction,
        };
        for bad in [
            MemoryTransaction::InitializeZeroed {
                address: 0x3000,
                length: 8,
            },
            MemoryTransaction::Read {
                address: 0x3000,
                width: 0,
                value: MemoryReadValue::Unknown,
            },
            MemoryTransaction::LoadReserved {
                address: 0x3001,
                order: ExecutionOrdering {
                    acquire: false,
                    release: false,
                },
                value: 1,
            },
            MemoryTransaction::Write {
                address: 0x3000,
                width: 1,
                value: 256,
            },
            MemoryTransaction::Write {
                address: u32::MAX - 3,
                width: 4,
                value: 1,
            },
        ] {
            assert_eq!(
                validate(&input, &event(bad)).unwrap_err().code,
                ErrorCode::Integrity
            );
        }
        let good = event(MemoryTransaction::Read {
            address: 0x3001,
            width: 4,
            value: MemoryReadValue::Unknown,
        });
        validate(&input, &good).unwrap();
        input.observe_timeline.reads = false;
        assert!(validate(&input, &good).is_err());
        for (target, fallthrough) in [(0x1001, 0x1004), (0x1000, 0x1006)] {
            assert!(
                validate(
                    &input,
                    &ExecutionEvent::Branch {
                        site: 0x1000,
                        target,
                        fallthrough,
                        taken: true
                    }
                )
                .is_err()
            );
        }
        input.observe_timeline.branches = false;
        assert!(
            validate(
                &input,
                &ExecutionEvent::Branch {
                    site: 0x1000,
                    target: 0x1000,
                    fallthrough: 0x1004,
                    taken: true
                }
            )
            .is_err()
        );
    }
}
