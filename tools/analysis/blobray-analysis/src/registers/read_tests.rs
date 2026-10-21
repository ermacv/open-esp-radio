use super::*;

fn number(value: u32) -> AbstractValue {
    AbstractValue::Constant { value }
}
fn expr(id: u32) -> AbstractValue {
    AbstractValue::Expression { id }
}

fn graph(depth: u32) -> Vec<FunctionRecord> {
    let mut records = vec![FunctionRecord::Expression {
        id: 0,
        offset: 4,
        expression: Expression::Load {
            address: number(0x20000),
            width: 4,
            signed: false,
        },
    }];
    for id in 1..depth {
        records.push(FunctionRecord::Expression {
            id,
            offset: 8,
            expression: Expression::Integer {
                op: IntegerOp::And,
                left: expr(id - 1),
                right: number(u32::MAX),
            },
        });
    }
    records.push(FunctionRecord::Condition {
        offset: 12,
        test: BranchTest::Ne,
        left: expr(depth - 1),
        right: number(0),
    });
    records
}

#[test]
fn read_memo_and_sink_storage_are_admitted_reused_and_released() {
    let memory = WorkingMemory::new(1 << 20).expect("working memory");
    {
        let records = graph(128);
        let facts = Facts::new(&records, &memory, &mut || Ok(())).expect("facts");
        let before = memory.used();
        let mut stored = StoredBitsEvaluator::new(&facts, &memory);
        let mut reads = ReadUsesEvaluator::new(&facts, &memory);
        let mut steps = 0;
        reads
            .analyze(true, None, &mut stored, &mut || {
                steps += 1;
                Ok(())
            })
            .expect("read analysis");
        assert!(steps < 1024, "shared nodes are evaluated once");
        assert!(
            memory.used() > before,
            "dependencies and events have memory admission"
        );
        let mut repeated = 0;
        reads
            .analyze(true, None, &mut stored, &mut || {
                repeated += 1;
                Ok(())
            })
            .expect("reuse analysis");
        assert_eq!(repeated, 0);
        let result = reads
            .reads(4, &number(0x20000), 4, MemoryKind::Load, &mut || Ok(()))
            .expect("read uses");
        assert_eq!(
            result.uses,
            [ReadUse {
                bits: u32::MAX,
                sink: ReadSink::Branch { site: 12 },
                precision: ReadPrecision::Exact
            }]
        );
        assert_eq!(result.unused | result.preserved, 0);
        assert!(result.unknown.is_empty());
    }
    assert_eq!(memory.used(), 0);
}

#[test]
fn interrupted_or_refused_dependency_storage_is_unknown_and_cancellation_propagates() {
    for memory_limit in [4096, 1 << 20] {
        let memory = WorkingMemory::new(memory_limit).expect("working memory");
        {
            let records = graph(128);
            let facts = Facts::new(&records, &memory, &mut || Ok(())).expect("facts");
            let mut stored = StoredBitsEvaluator::new(&facts, &memory);
            let mut reads = ReadUsesEvaluator::new(&facts, &memory);
            let mut steps = 0;
            reads
                .analyze(true, None, &mut stored, &mut || {
                    steps += 1;
                    if memory_limit > 4096 && steps > 16 {
                        Err(Error::new(
                            ErrorCode::ResourceLimited,
                            "fixture work budget",
                        ))
                    } else {
                        Ok(())
                    }
                })
                .expect("bounded read analysis");
            let result = reads
                .reads(4, &number(0x20000), 4, MemoryKind::Load, &mut || Ok(()))
                .expect("limited read uses");
            assert_eq!(
                result.unknown,
                [ReadLimit {
                    bits: u32::MAX,
                    limit: ReadLimitKind::WorkLimit
                }]
            );
            assert_eq!(result.unused | result.preserved, 0);
        }
        assert_eq!(
            memory.used(),
            0,
            "admissions released after interrupted analysis"
        );
    }
    let memory = WorkingMemory::new(1 << 20).expect("working memory");
    let records = graph(2);
    let facts = Facts::new(&records, &memory, &mut || Ok(())).expect("facts");
    let mut stored = StoredBitsEvaluator::new(&facts, &memory);
    let mut reads = ReadUsesEvaluator::new(&facts, &memory);
    assert_eq!(
        reads
            .analyze(true, None, &mut stored, &mut || Err(Error::new(
                ErrorCode::Cancelled,
                "cancel fixture"
            )))
            .expect_err("cancellation")
            .code,
        ErrorCode::Cancelled
    );
}
