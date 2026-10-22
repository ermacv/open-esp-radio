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
            Some(7) | Some(8) => SemanticOp::Integer {
                op: IntegerOp::Or,
                dest: 6,
                left: Operand::Register(6),
                right: Operand::Immediate(if bytes.first() == Some(&7) { 1 } else { 2 }),
            },
            Some(6) => SemanticOp::Memory {
                kind: MemoryKind::Store,
                base: 5,
                displacement: 0,
                width: 4,
                dest: None,
                source: Some(6),
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
        let mut reads = FlowEngine::<Reads>::new(&facts, &memory);
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
            let mut reads = FlowEngine::<Reads>::new(&facts, &memory);
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
    let mut reads = FlowEngine::<Reads>::new(&facts, &memory);
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
    let mut reads = FlowEngine::<Reads>::new(&facts, &memory);
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
                preserved: 0,
                limits: [0; 4],
                event: 0,
            },
            c.position(),
        )
        .expect("source");
    let original = Reads::seed(&mut reads, 0, 4, false, u32::MAX, &mut c).expect("load seed");
    assert_eq!(
        Reads::join(&mut reads, original, 0, &mut c).expect("join absent"),
        original
    );
    let shifted = Reads::map(&mut reads, original, |i| i.checked_sub(1), &mut c).expect("shift");
    let join = Reads::join(&mut reads, original, shifted, &mut c).expect("conflicting join");
    let dependency = reads
        .domain
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
    let mut reads = FlowEngine::<Reads>::new(&facts, &memory);
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
        let mut reads = FlowEngine::<Reads>::new(&facts, &memory);
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
        let mut reads = FlowEngine::<Reads>::new(&facts, &memory);
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

/// A dependency means that a source reaches a sink on paths carrying it.
/// Word provenance agrees across distinct physical loads of the same word.
#[test]
fn distinct_loads_at_a_join_preserve_the_same_word() {
    let memory = WorkingMemory::new(1 << 20).expect("memory");
    {
        let mut records = Vec::new();
        for (site, byte) in [(0, 3), (4, 1), (8, 0), (12, 1), (16, 6), (20, 0)] {
            records.push(FunctionRecord::Instruction {
                offset: site,
                bytes: vec![byte],
                decoded: DecodedOp {
                    length: 4,
                    text: String::new(),
                    flow: if site == 20 {
                        InstructionFlow::Return { base: 1 }
                    } else {
                        InstructionFlow::Next
                    },
                },
            });
        }
        for (from, target, relation) in [
            (0, 4, EdgeKind::Fallthrough),
            (0, 12, EdgeKind::Taken),
            (4, 8, EdgeKind::Fallthrough),
            (8, 16, EdgeKind::Jump),
            (12, 16, EdgeKind::Fallthrough),
            (16, 20, EdgeKind::Fallthrough),
        ] {
            records.push(FunctionRecord::Edge {
                from,
                target: Some(target),
                relation,
                external: false,
            });
        }
        records.push(FunctionRecord::Edge {
            from: 20,
            target: None,
            relation: EdgeKind::Return,
            external: true,
        });
        for site in [4, 12, 16] {
            records.push(FunctionRecord::MemoryAccess {
                offset: site,
                access: if site == 16 {
                    MemoryKind::Store
                } else {
                    MemoryKind::Load
                },
                address: number(0x20000),
                width: 4,
                value: None,
                relocation: None,
            });
        }
        let facts = Facts::new(&records, &memory, &mut || Ok(())).expect("facts");
        let mut reads = ReadUsesEvaluator::new(&facts, &memory);
        reads
            .analyze(
                None,
                &Decoder,
                &mut |_, _, _| Ok(true),
                &|_| None,
                &mut || Ok(()),
            )
            .expect("read domain");
        for site in [4, 12] {
            let result = reads
                .reads(site, &number(0x20000), 4, MemoryKind::Load, &mut || Ok(()))
                .expect("read uses");
            assert_eq!(result.preserved, u32::MAX);
            assert!(result.uses.is_empty());
            assert!(result.unknown.is_empty());
        }
        let mut provenance = StoredProvenanceEvaluator::new(&facts, &memory);
        provenance
            .analyze(None, &Decoder, &|_| None, &mut || Ok(()))
            .expect("provenance domain");
        assert!(!provenance.work_limited());
        assert_eq!(
            provenance
                .stored(16, &number(0x20000), 4, None, &mut || Ok(()))
                .expect("projection"),
            [StoredBits {
                low: 0,
                width: 32,
                source: StoredBitsSource::Load {
                    address: Some(0x20000),
                    width: 4,
                    low: 0,
                    same_word: true
                }
            }]
        );
    }
    assert_eq!(memory.used(), 0);
}

#[test]
fn provenance_work_counts_instruction_visits_and_checks_cancellation_between_them() {
    #[derive(Default)]
    struct Units {
        units: u64,
    }
    impl RunControl for Units {
        fn checkpoint(&mut self, units: u64) -> Result<()> {
            self.units += units;
            Ok(())
        }
    }
    let mut caller = Units::default();
    {
        let mut work = ReadWork {
            caller: &mut caller,
            remaining: 2,
            instruction_only: true,
        };
        work.checkpoint(1_000_000)
            .expect("tag and bookkeeping work");
        work.visit().expect("first instruction");
        work.checkpoint(1_000_000).expect("more bookkeeping");
        work.visit().expect("second instruction");
        assert_eq!(
            work.visit().expect_err("instruction budget").code,
            ErrorCode::ResourceLimited
        );
    }
    assert_eq!(caller.units, 2);
    let mut cancel = || Err(Error::new(ErrorCode::Cancelled, "cancel fixture"));
    let mut work = ReadWork {
        caller: &mut cancel,
        remaining: 2,
        instruction_only: true,
    };
    assert_eq!(
        work.checkpoint(32)
            .expect_err("cancellation outside visit")
            .code,
        ErrorCode::Cancelled
    );
}

#[test]
fn a_word_and_a_forced_bit_at_a_join_remain_unknown_while_other_bits_are_kept() {
    let memory = WorkingMemory::new(1 << 20).expect("memory");
    let mut records = Vec::new();
    for (site, byte) in [
        (0, 3),
        (4, 1),
        (8, 7),
        (12, 0),
        (16, 1),
        (20, 8),
        (24, 6),
        (28, 0),
    ] {
        records.push(FunctionRecord::Instruction {
            offset: site,
            bytes: vec![byte],
            decoded: DecodedOp {
                length: 4,
                text: String::new(),
                flow: if site == 28 {
                    InstructionFlow::Return { base: 1 }
                } else {
                    InstructionFlow::Next
                },
            },
        });
    }
    for (from, target, relation) in [
        (0, 4, EdgeKind::Fallthrough),
        (0, 16, EdgeKind::Taken),
        (4, 8, EdgeKind::Fallthrough),
        (8, 12, EdgeKind::Fallthrough),
        (12, 24, EdgeKind::Jump),
        (16, 20, EdgeKind::Fallthrough),
        (20, 24, EdgeKind::Fallthrough),
        (24, 28, EdgeKind::Fallthrough),
    ] {
        records.push(FunctionRecord::Edge {
            from,
            target: Some(target),
            relation,
            external: false,
        });
    }
    records.push(FunctionRecord::Edge {
        from: 28,
        target: None,
        relation: EdgeKind::Return,
        external: true,
    });
    for site in [4, 16, 24] {
        records.push(FunctionRecord::MemoryAccess {
            offset: site,
            access: if site == 24 {
                MemoryKind::Store
            } else {
                MemoryKind::Load
            },
            address: number(0x20000),
            width: 4,
            value: None,
            relocation: None,
        });
    }
    let facts = Facts::new(&records, &memory, &mut || Ok(())).expect("facts");
    let mut reads = ReadUsesEvaluator::new(&facts, &memory);
    reads
        .analyze(
            None,
            &Decoder,
            &mut |_, _, _| Ok(true),
            &|_| None,
            &mut || Ok(()),
        )
        .expect("reads");
    for (site, forced) in [(4, 1), (16, 2)] {
        assert_eq!(
            reads
                .reads(site, &number(0x20000), 4, MemoryKind::Load, &mut || Ok(()))
                .expect("read")
                .preserved,
            !forced
        );
    }
    let mut provenance = StoredProvenanceEvaluator::new(&facts, &memory);
    provenance
        .analyze(None, &Decoder, &|_| None, &mut || Ok(()))
        .expect("provenance");
    assert!(!provenance.work_limited());
    let stored = provenance
        .stored(24, &number(0x20000), 4, None, &mut || Ok(()))
        .expect("store");
    assert_eq!(
        stored,
        [
            StoredBits {
                low: 0,
                width: 2,
                source: StoredBitsSource::Unknown
            },
            StoredBits {
                low: 2,
                width: 30,
                source: StoredBitsSource::Load {
                    address: Some(0x20000),
                    width: 4,
                    low: 2,
                    same_word: true
                }
            }
        ]
    );
    assert_eq!(
        write_mask(4, &stored),
        Some(RegisterMask {
            kind: RegisterMaskKind::WriteReplacement,
            bits: 3
        })
    );
}
