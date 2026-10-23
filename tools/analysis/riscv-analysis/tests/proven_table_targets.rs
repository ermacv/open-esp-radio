use oer_analysis_riscv_analysis::*;
use oer_analysis_riscv_model::*;

const TABLE: u32 = 0x1000;
const SITE: u64 = 60;
const ENTRIES: [u64; 6] = [72, 62, 70, 64, 68, 66];
const INDICES: [u32; 8] = [0, 1, 2, 3, 4, 5, 254, 255];

#[derive(Default)]
struct Records(Vec<FunctionRecord>);

impl FunctionSink for Records {
    fn record(&mut self, record: &FunctionRecord, _: &mut dyn RunControl) -> Result<()> {
        self.0.push(record.clone());
        Ok(())
    }
}

/// Eight paths merge before `bltu 5, index`; only its fall-through enters
/// `table + (index << 2); lw target; jalr zero, 0(target)`. Values retain
/// 254/255 across the check, but the table proof admits only entries 0..5.
struct Fixture {
    bounded: bool,
    local_extras: bool,
}

impl FunctionDecoder for Fixture {
    fn identity(&self) -> &'static str {
        "bounded-table-fixture"
    }

    fn decode(&self, bytes: &[u8]) -> Option<DecodedOp> {
        let at = u64::from(*bytes.first()?) * 2;
        let flow = match at {
            0..=12 => InstructionFlow::Branch {
                displacement: (16 + 2 * at) as i32 - at as i32,
            },
            14 => InstructionFlow::Jump {
                displacement: 30,
                link: false,
            },
            16..=46 if at % 4 == 2 => InstructionFlow::Jump {
                displacement: 48 - at as i32,
                link: false,
            },
            50 if self.bounded => InstructionFlow::Branch { displacement: 24 },
            SITE => InstructionFlow::Indirect {
                base: 15,
                offset: 0,
                link: false,
            },
            62..=74 => InstructionFlow::Return { base: 1 },
            _ => InstructionFlow::Next,
        };
        Some(DecodedOp {
            length: 2,
            text: String::new(),
            flow,
        })
    }

    fn reference(
        &self,
        _: &FunctionRelocation,
        _: &[FunctionRelocation],
        _: u32,
        _: &mut dyn RunControl,
    ) -> Result<NormalizedReference> {
        unreachable!("fixture has no relocations")
    }
}

fn set(dest: u8, value: u32) -> SemanticOp {
    SemanticOp::Integer {
        op: IntegerOp::Add,
        dest,
        left: Operand::Register(0),
        right: Operand::Immediate(value),
    }
}

impl FunctionSemantics for Fixture {
    fn semantic_identity(&self) -> &'static str {
        "bounded-table-fixture-values"
    }

    fn lift(&self, bytes: &[u8]) -> SemanticOp {
        match u64::from(bytes[0]) * 2 {
            at @ 16..=44 if at % 4 == 0 => set(11, INDICES[((at - 16) / 4) as usize]),
            48 => set(12, 5),
            52 => set(13, TABLE),
            54 => SemanticOp::Integer {
                op: IntegerOp::Shl,
                dest: 14,
                left: Operand::Register(11),
                right: Operand::Immediate(2),
            },
            56 => SemanticOp::Integer {
                op: IntegerOp::Add,
                dest: 14,
                left: Operand::Register(14),
                right: Operand::Register(13),
            },
            58 => SemanticOp::Memory {
                kind: MemoryKind::Load,
                base: 14,
                displacement: 0,
                width: 4,
                dest: Some(15),
                source: None,
                swap: false,
                signed: false,
            },
            _ => SemanticOp::None,
        }
    }

    fn branch(&self, bytes: &[u8]) -> Option<(BranchTest, Operand, Operand)> {
        match u64::from(bytes[0]) * 2 {
            0..=12 => Some((BranchTest::Eq, Operand::Register(10), Operand::Register(0))),
            50 if self.bounded => Some((
                BranchTest::Ltu,
                Operand::Register(12),
                Operand::Register(11),
            )),
            _ => None,
        }
    }

    fn value_relocation(&self, _: &FunctionRelocation, _: SemanticOp) -> ValueRelocation {
        unreachable!("fixture has no relocations")
    }
}

impl ImageMemory for Fixture {
    fn read_constant(
        &self,
        address: u32,
        bytes: &mut [u8],
        _: &mut dyn RunControl,
    ) -> Result<bool> {
        let Some(index) = address.checked_sub(TABLE).filter(|offset| offset % 4 == 0) else {
            return Ok(false);
        };
        let target = match index / 4 {
            index @ 0..=5 => ENTRIES[index as usize] as u32,
            254 => {
                if self.local_extras {
                    0
                } else {
                    0x2000
                }
            }
            255 => {
                if self.local_extras {
                    2
                } else {
                    0x3000
                }
            }
            _ => return Ok(false),
        };
        bytes.copy_from_slice(&target.to_le_bytes());
        Ok(true)
    }
}

impl TableEntries for Fixture {
    fn entries<'m>(
        &self,
        base: &AbstractValue,
        count: Option<u32>,
        _: u32,
        memory: &'m WorkingMemory,
        control: &mut dyn RunControl,
    ) -> Result<Option<TableWords<'m>>> {
        if *base != (AbstractValue::Constant { value: TABLE }) || count != Some(6) {
            return Ok(None);
        }
        control.checkpoint(6)?;
        Ok(Some(TableWords {
            source: TableSource::Bound,
            entries: ENTRIES.to_vec(),
            reserved: memory.reserve(48, control.position())?,
        }))
    }
}

#[test]
fn proven_table_edges_replace_unfiltered_value_targets() {
    for (bounded, local_extras) in [(false, false), (true, false), (true, true)] {
        let fixture = Fixture {
            bounded,
            local_extras,
        };
        let bytes: Vec<_> = (0..38).flat_map(|index| [index, 0]).collect();
        let memory = WorkingMemory::new(1024 * 1024).expect("fixture memory");
        let mut records = Records::default();
        let (_, tables) = research_following_tables(
            FunctionInput {
                traps: TrapSemantics::Unknown,
                image: Some(&fixture),
                section: 1,
                extent: CodeRange {
                    start: 0,
                    length: 76,
                },
                bytes: &bytes,
                relocations: &PreparedReferences::empty(),
                data_ranges: &[],
                jumps: &[],
                returns: &NoSummaries,
                call_behavior: &UnknownCallBehavior,
            },
            &RecognizedTables {
                entries: &fixture,
                section: 1,
            },
            &fixture,
            &memory,
            &mut || Ok(()),
            &mut records,
            None,
        )
        .expect("table research");
        let facts =
            FunctionIndex::new(&records.0, RecordPolicy::Canonical, &memory, &mut || Ok(()))
                .expect("canonical dispatch facts");
        assert!(
            facts
                .call_inputs(SITE, &mut || Ok(()))
                .expect("pre-jump inputs")
                .is_some(),
            "retain the registers needed to revalidate the dispatch"
        );
        let transfer = records.0.iter().find_map(|record| match record {
            FunctionRecord::Transfer {
                offset: SITE,
                target,
                call: false,
            } => Some(target),
            _ => None,
        });
        let targets: Vec<_> = records
            .0
            .iter()
            .filter_map(|record| match record {
                FunctionRecord::Edge {
                    from: SITE,
                    target,
                    relation: EdgeKind::Jump,
                    ..
                } => *target,
                _ => None,
            })
            .collect();
        let gap = records
            .0
            .iter()
            .any(|record| matches!(record, FunctionRecord::SemanticGap { offset: SITE, .. }));
        if bounded {
            assert_eq!(
                tables.tables,
                [JumpDispatch::Table {
                    site: SITE,
                    first_case: None,
                    entries: ENTRIES.to_vec(),
                }]
            );
            assert_eq!(targets, [62, 64, 66, 68, 70, 72]);
            assert!(
                transfer.is_none(),
                "proven local edges own the targets: {transfer:?}"
            );
            assert!(!gap, "a proven dispatch has no transfer gap");
        } else {
            assert!(tables.tables.is_empty());
            assert!(targets.is_empty());
            let Some(AbstractValue::Alternatives { values }) = transfer else {
                panic!("unproven dispatch retains its value targets: {transfer:?}")
            };
            assert_eq!(values.values().len(), 8);
            assert!(gap, "without a bound the dispatch remains incomplete");
        }
        assert!(records.0.iter().any(|record| matches!(record,
            FunctionRecord::Dispatch { offset: SITE, index: AbstractValue::Alternatives { values }, .. }
                if values.values().len() == 8
        )), "the fixture must retain the two out-of-range indices");
    }
}
