use super::*;

struct Decoder;
impl FunctionDecoder for Decoder {
    fn identity(&self) -> &'static str {
        "read-flow-fixture"
    }
    fn decode(&self, _bytes: &[u8]) -> Option<DecodedOp> {
        None
    }
    fn reference(
        &self,
        _relocation: &FunctionRelocation,
        _all: &[FunctionRelocation],
        _section: u32,
        _c: &mut dyn RunControl,
    ) -> Result<NormalizedReference> {
        Err(read_integrity())
    }
}
impl FunctionSemantics for Decoder {
    fn semantic_identity(&self) -> &'static str {
        "read-flow-fixture"
    }
    fn branch(&self, bytes: &[u8]) -> Option<(BranchTest, Operand, Operand)> {
        (bytes.first() == Some(&3)).then_some((
            BranchTest::Ne,
            Operand::Register(6),
            Operand::Immediate(0),
        ))
    }
    fn lift(&self, bytes: &[u8]) -> SemanticOp {
        match bytes.first() {
            Some(1) => SemanticOp::Memory {
                kind: MemoryKind::Load,
                base: 5,
                displacement: 0,
                width: 4,
                dest: Some(6),
                source: None,
                signed: false,
                swap: false,
            },
            Some(2) => SemanticOp::Integer {
                op: IntegerOp::And,
                dest: 6,
                left: Operand::Register(6),
                right: Operand::Immediate(u32::MAX),
            },
            Some(4) => SemanticOp::Memory {
                kind: MemoryKind::Load,
                base: 5,
                displacement: 0,
                width: 4,
                dest: Some(7),
                source: None,
                signed: false,
                swap: false,
            },
            Some(5) => SemanticOp::Integer {
                op: IntegerOp::Add,
                dest: 6,
                left: Operand::Register(6),
                right: Operand::Register(7),
            },
            _ => SemanticOp::None,
        }
    }
    fn value_relocation(
        &self,
        _relocation: &FunctionRelocation,
        _operation: SemanticOp,
    ) -> ValueRelocation {
        ValueRelocation::Ignore
    }
}

fn number(value: u32) -> AbstractValue {
    AbstractValue::Constant { value }
}
fn graph(depth: u64) -> Vec<FunctionRecord> {
    let mut records = Vec::new();
    for id in 0..depth + 2 {
        let offset = id * 4;
        records.push(FunctionRecord::Instruction {
            offset,
            bytes: vec![if id == 0 {
                1
            } else if id == depth {
                3
            } else {
                2
            }],
            decoded: DecodedOp {
                length: 4,
                text: String::new(),
                flow: if id == depth + 1 {
                    InstructionFlow::Return { base: 1 }
                } else {
                    InstructionFlow::Next
                },
            },
        });
        records.push(FunctionRecord::Edge {
            from: offset,
            target: (id < depth + 1).then_some(offset + 4),
            relation: if id == depth + 1 {
                EdgeKind::Return
            } else {
                EdgeKind::Fallthrough
            },
            external: id == depth + 1,
        });
    }
    records.push(FunctionRecord::MemoryAccess {
        offset: 0,
        access: MemoryKind::Load,
        width: 4,
        address: number(0x20000),
        value: None,
        relocation: None,
    });
    records
}

#[test]
fn read_states_and_sink_storage_are_admitted_reused_and_released() {
    let memory = WorkingMemory::new(1 << 20).expect("working memory");
    {
        let records = graph(128);
        let facts = Facts::new(&records, &memory, &mut || Ok(())).expect("facts");
        let before = memory.used();
        let mut reads = ReadUsesEvaluator::new(&facts, &memory);
        let mut steps = 0;
        reads
            .analyze(
                None,
                &Decoder,
                &mut |_, _, _| Ok(true),
                &|_| None,
                &mut || {
                    steps += 1;
                    Ok(())
                },
            )
            .expect("read analysis");
        assert!(steps < 4096, "straight-line dependencies take linear work");
        assert!(
            memory.used() > before,
            "states and dependencies have memory admission"
        );
        let mut repeated = 0;
        reads
            .analyze(
                None,
                &Decoder,
                &mut |_, _, _| Ok(true),
                &|_| None,
                &mut || {
                    repeated += 1;
                    Ok(())
                },
            )
            .expect("reuse analysis");
        assert_eq!(repeated, 0);
        let result = reads
            .reads(0, &number(0x20000), 4, MemoryKind::Load, &mut || Ok(()))
            .expect("read uses");
        assert_eq!(
            result.uses,
            [ReadUse {
                bits: u32::MAX,
                sink: ReadSink::Branch { site: 512 },
                precision: ReadPrecision::Exact
            }]
        );
        assert_eq!(result.unused | result.preserved, 0);
        assert!(result.unknown.is_empty());
    }
    assert_eq!(memory.used(), 0);
}

#[test]
fn interrupted_or_refused_state_storage_is_unknown_and_cancellation_propagates() {
    for memory_limit in [4096, 1 << 20] {
        let memory = WorkingMemory::new(memory_limit).expect("working memory");
        {
            let records = graph(128);
            let facts = Facts::new(&records, &memory, &mut || Ok(())).expect("facts");
            let mut reads = ReadUsesEvaluator::new(&facts, &memory);
            let mut steps = 0;
            reads
                .analyze(
                    None,
                    &Decoder,
                    &mut |_, _, _| Ok(true),
                    &|_| None,
                    &mut || {
                        steps += 1;
                        if memory_limit > 4096 && steps > 16 {
                            Err(Error::new(
                                ErrorCode::ResourceLimited,
                                "fixture work budget",
                            ))
                        } else {
                            Ok(())
                        }
                    },
                )
                .expect("bounded read analysis");
            let result = reads
                .reads(0, &number(0x20000), 4, MemoryKind::Load, &mut || Ok(()))
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
    let mut reads = ReadUsesEvaluator::new(&facts, &memory);
    assert_eq!(
        reads
            .analyze(
                None,
                &Decoder,
                &mut |_, _, _| Ok(true),
                &|_| None,
                &mut || Err(Error::new(ErrorCode::Cancelled, "cancel fixture"))
            )
            .expect_err("cancellation")
            .code,
        ErrorCode::Cancelled
    );
}

#[test]
fn conflicting_mappings_demote_but_a_missing_source_does_not_demote() {
    let memory = WorkingMemory::new(1 << 20).expect("working memory");
    let facts = Facts::new(&[], &memory, &mut || Ok(())).expect("facts");
    let mut reads = ReadUsesEvaluator::new(&facts, &memory);
    let mut c = || Ok(());
    let address = number(0x20000);
    reads
        .sources
        .push(
            ReadSource {
                origin: ReadOrigin::Load {
                    site: 0,
                    address: &address,
                    width: 4,
                },
                observed: true,
                preserved: 0,
                limits: [0; 4],
                event: 0,
            },
            c.position(),
        )
        .expect("source");
    let original = reads
        .seed(0, 4, false, u32::MAX, &mut c)
        .expect("load seed");
    assert_eq!(
        reads.join_flow(original, 0, &mut c).expect("join absent"),
        original
    );
    let shifted = reads
        .map_flow(original, |i| i.checked_sub(1), &mut c)
        .expect("shift");
    let join = reads
        .join_flow(original, shifted, &mut c)
        .expect("conflicting join");
    let dependency = reads
        .flows
        .values(join)
        .expect("flow")
        .get(0)
        .expect("source");
    assert_eq!(dependency.selected(u32::MAX), (0, u32::MAX));
}

#[test]
fn a_large_straight_line_function_gets_a_size_proportional_budget() {
    let memory = WorkingMemory::new(64 << 20).expect("working memory");
    let records = graph(8192);
    let facts = Facts::new(&records, &memory, &mut || Ok(())).expect("large facts");
    let mut reads = ReadUsesEvaluator::new(&facts, &memory);
    let mut work = 0;
    struct Count<'a>(&'a mut u64);
    impl RunControl for Count<'_> {
        fn checkpoint(&mut self, units: u64) -> Result<()> {
            *self.0 += units;
            Ok(())
        }
    }
    reads
        .analyze(
            None,
            &Decoder,
            &mut |_, _, _| Ok(true),
            &|_| None,
            &mut Count(&mut work),
        )
        .expect("large straight line");
    assert!(
        work > READ_MIN_FUNCTION_WORK,
        "fixture exceeds the old flat limit"
    );
    let result = reads
        .reads(0, &number(0x20000), 4, MemoryKind::Load, &mut || Ok(()))
        .expect("large result");
    assert!(result.unknown.is_empty());
    assert_eq!(result.uses[0].bits, u32::MAX);
    assert_eq!(result.uses[0].sink, ReadSink::Branch { site: 8192 * 4 });
}

/// Many distinct loads accumulate around a cycle. Unlike straight-line code,
/// dependency sets grow on repeated visits and must still hit the local cap.
fn accumulating_cycle(depth: u64) -> Vec<FunctionRecord> {
    let mut records = graph(depth);
    for record in &mut records {
        match record {
            FunctionRecord::Instruction { offset, bytes, .. } if *offset < depth * 4 => {
                bytes[0] = if *offset % 8 == 0 { 4 } else { 5 };
            }
            FunctionRecord::Edge { from, target, .. } if *from == depth * 4 => *target = Some(0),
            _ => {}
        }
    }
    for id in (2..depth).step_by(2) {
        records.push(FunctionRecord::MemoryAccess {
            offset: id * 4,
            access: MemoryKind::Load,
            width: 4,
            address: number(0x20000),
            value: None,
            relocation: None,
        });
    }
    records
}

#[test]
fn a_function_work_limit_leaves_budget_for_the_next_function() {
    struct Budget(u64);
    impl RunControl for Budget {
        fn checkpoint(&mut self, units: u64) -> Result<()> {
            self.0 = self.0.checked_sub(units).ok_or_else(|| {
                Error::new(ErrorCode::ResourceLimited, "operation fixture budget")
            })?;
            Ok(())
        }
    }
    let memory = WorkingMemory::new(64 << 20).expect("working memory");
    let records = accumulating_cycle(256);
    let local_limit = read_work_limit(258);
    let mut budget = Budget(local_limit * 3);
    {
        let facts = Facts::new(&records, &memory, &mut || Ok(())).expect("large facts");
        let mut reads = ReadUsesEvaluator::new(&facts, &memory);
        reads
            .analyze(
                None,
                &Decoder,
                &mut |_, _, _| Ok(true),
                &|_| None,
                &mut budget,
            )
            .expect("per-function limit is reported");
        assert!(
            budget.0 >= local_limit,
            "a function cannot spend the entire operation budget"
        );
        let result = reads
            .reads(0, &number(0x20000), 4, MemoryKind::Load, &mut budget)
            .expect("limited result");
        assert_eq!(
            result.unknown,
            [ReadLimit {
                bits: u32::MAX,
                limit: ReadLimitKind::WorkLimit
            }]
        );
    }
    assert_eq!(memory.used(), 0);
    {
        let records = graph(2);
        let facts = Facts::new(&records, &memory, &mut || Ok(())).expect("small facts");
        let mut reads = ReadUsesEvaluator::new(&facts, &memory);
        reads
            .analyze(
                None,
                &Decoder,
                &mut |_, _, _| Ok(true),
                &|_| None,
                &mut budget,
            )
            .expect("next function still analyzed");
        let result = reads
            .reads(0, &number(0x20000), 4, MemoryKind::Load, &mut budget)
            .expect("next result");
        assert!(result.unknown.is_empty());
        assert_eq!(result.uses.first().expect("branch use").bits, u32::MAX);
    }
    assert_eq!(memory.used(), 0);
}
