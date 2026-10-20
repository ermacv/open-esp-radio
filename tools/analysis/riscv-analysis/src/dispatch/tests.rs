use super::*;

const TEXT: u32 = 1;

/// The review reproducer: neither source's entry zero is the smallest target.
struct TwoTables;
impl TableEntries for TwoTables {
    fn entries<'m>(
        &self,
        base: &AbstractValue,
        count: Option<u32>,
        section: u32,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Option<TableWords<'m>>> {
        let AbstractValue::Symbol { symbol, addend } = base else {
            return Ok(None);
        };
        if *symbol != table_symbol() || count != Some(3) || section != TEXT {
            return Ok(None);
        }
        let entries = match *addend {
            0 => vec![0x30, 0x20, 0x30],
            12 => vec![0x38, 0x30, 0x20],
            _ => return Ok(None),
        };
        let reserved = memory.reserve(24, c.position())?;
        Ok(Some(TableWords { entries, reserved }))
    }
}

#[test]
fn a_union_is_a_target_set_without_an_entry_index_mapping() {
    let mut records = records(true);
    for record in &mut records {
        if let FunctionRecord::Dispatch { base, .. } = record {
            *base = AbstractValue::Alternatives {
                values: ValueAlternatives::new(
                    [0, 12]
                        .map(|addend| ValueAlternative::Symbol {
                            symbol: table_symbol(),
                            addend,
                        })
                        .to_vec(),
                )
                .unwrap(),
            };
        }
    }
    let found = jump_tables(&records, TEXT, EXTENT, &TwoTables, memory(), &mut || Ok(())).unwrap();
    assert_eq!(
        found.tables,
        [JumpDispatch::TargetSet {
            site: 0x14,
            targets: vec![0x20, 0x30, 0x38]
        }]
    );
    let source = RecognizedTables {
        entries: &TwoTables,
        section: TEXT,
    };
    assert_eq!(
        source.jump(&found.tables[0]),
        KnownJump {
            site: 0x14,
            targets: vec![0x20, 0x30, 0x38]
        }
    );
    assert!(
        !source.same(
            &found.tables[0],
            &JumpDispatch::Table {
                site: 0x14,
                first_case: None,
                entries: vec![0x20, 0x30, 0x38],
            }
        ),
        "a target set is never an indexed table, even with equal addresses"
    );
}

#[test]
fn a_single_table_keeps_original_order_and_duplicate_entries() {
    let found = jump_tables(
        &records(true),
        TEXT,
        EXTENT,
        &TwoTables,
        memory(),
        &mut || Ok(()),
    )
    .unwrap();
    assert_eq!(
        found.tables,
        [JumpDispatch::Table {
            site: 0x14,
            first_case: Some(0),
            entries: vec![0x30, 0x20, 0x30],
        }]
    );
}
const SYMTAB: u32 = 10;
const TABLE: u64 = 7;

fn table_symbol() -> SymbolId {
    SymbolId {
        object: ObjectId {
            artifact: ArtifactId::of_bytes(b"object"),
            location: ObjectLocation::Standalone,
        },
        table: SymbolTableKind::Static,
        table_section: SYMTAB,
        index: TABLE,
    }
}

/// `bltu 2, a1` at 4 (when `bounded`), whose fall-through is the only way
/// into the dispatch block 8..0x18; there `a5 = load4[(a1 << 2) + .Ltable]`
/// and `jalr zero, 0(a5)` at 0x14.
fn records(bounded: bool) -> Vec<FunctionRecord> {
    let expression = |id, expression| FunctionRecord::Expression {
        id,
        offset: 0,
        expression,
    };
    let edge = |from, target, relation| FunctionRecord::Edge {
        from,
        target: Some(target),
        relation,
        external: false,
    };
    let mut records = vec![
        FunctionRecord::Block {
            id: 0,
            start: 0,
            end: 8,
        },
        FunctionRecord::Block {
            id: 1,
            start: 8,
            end: 0x18,
        },
        edge(4, 8, EdgeKind::Fallthrough),
        edge(4, 0x3c, EdgeKind::Taken),
        expression(0, Expression::EntryRegister { register: 11 }),
        expression(
            1,
            Expression::Integer {
                op: IntegerOp::Shl,
                left: AbstractValue::Expression { id: 0 },
                right: AbstractValue::Constant { value: 2 },
            },
        ),
        expression(
            2,
            Expression::Integer {
                op: IntegerOp::Add,
                left: AbstractValue::Expression { id: 1 },
                right: AbstractValue::Symbol {
                    symbol: table_symbol(),
                    addend: 0,
                },
            },
        ),
        expression(
            3,
            Expression::Load {
                address: AbstractValue::Expression { id: 2 },
                width: 4,
                signed: true,
            },
        ),
        FunctionRecord::CallInputs {
            offset: 0x14,
            registers: (0..32)
                .map(|register| {
                    if register == 15 {
                        AbstractValue::Expression { id: 3 }
                    } else {
                        AbstractValue::Unknown
                    }
                })
                .collect(),
        },
        FunctionRecord::Dispatch {
            offset: 0x14,
            base: AbstractValue::Symbol {
                symbol: table_symbol(),
                addend: 0,
            },
            index: AbstractValue::Expression { id: 0 },
            scale: 2,
        },
        FunctionRecord::Instruction {
            offset: 0x14,
            bytes: vec![0x67, 0x80, 0x07, 0x00],
            decoded: DecodedOp {
                length: 4,
                text: "jalr zero, 0(a5)".into(),
                flow: InstructionFlow::Indirect {
                    base: 15,
                    offset: 0,
                    link: false,
                },
            },
        },
    ];
    if bounded {
        records.push(FunctionRecord::Condition {
            offset: 4,
            test: BranchTest::Ltu,
            left: AbstractValue::Constant { value: 2 },
            right: AbstractValue::Expression { id: 0 },
        });
    }
    records
}

fn memory() -> &'static WorkingMemory {
    Box::leak(Box::new(WorkingMemory::new(1 << 20).unwrap()))
}

/// A build whose `.Ltable` symbol holds `entries` words naming case labels
/// at 0x20, 0x30, 0x20, … of `.text` (`last` for the last).
struct Facts {
    words: Vec<u64>,
}

impl TableEntries for Facts {
    fn entries<'m>(
        &self,
        base: &AbstractValue,
        count: Option<u32>,
        section: u32,
        memory: &'m WorkingMemory,
        control: &mut dyn RunControl,
    ) -> Result<Option<TableWords<'m>>> {
        let (AbstractValue::Symbol { symbol, addend: 0 }, Some(count)) = (base, count) else {
            return Ok(None);
        };
        if *symbol != table_symbol() || section != TEXT {
            return Ok(None);
        }
        let reserved = memory.reserve(u64::from(count) * 8, control.position())?;
        let mut entries = Vec::new();
        for &word in self.words.iter().take(count as usize) {
            control.checkpoint(1)?;
            entries.push(word);
        }
        Ok(Some(TableWords { entries, reserved }))
    }
}

fn facts(entries: u64, last: u64) -> Facts {
    Facts {
        words: (0..entries)
            .map(|entry| {
                if entry + 1 == entries {
                    last
                } else if entry % 2 == 0 {
                    0x20
                } else {
                    0x30
                }
            })
            .collect(),
    }
}

fn run(records: &[FunctionRecord], facts: &Facts) -> Vec<JumpDispatch> {
    jump_tables(records, TEXT, EXTENT, facts, memory(), &mut || Ok(()))
        .unwrap()
        .tables
}

const EXTENT: CodeRange = CodeRange {
    start: 0,
    length: 0x40,
};

#[test]
fn a_bounded_relocated_table_yields_its_cases_in_index_order() {
    let tables = run(&records(true), &facts(3, 0x38));
    assert_eq!(
        tables,
        [JumpDispatch::Table {
            site: 0x14,
            first_case: Some(0),
            entries: vec![0x20, 0x30, 0x38],
        }],
        "`bltu 2, a1` admits indices 0..=2"
    );
}

#[test]
fn an_unbounded_index_stays_an_indirect_gap() {
    assert!(run(&records(false), &facts(3, 0x38)).is_empty());
}

#[test]
fn a_selected_entry_without_its_relocation_stays_an_indirect_gap() {
    assert!(run(&records(true), &facts(2, 0x30)).is_empty());
}

#[test]
fn an_entry_outside_the_function_stays_an_indirect_gap() {
    assert!(run(&records(true), &facts(3, 0x80)).is_empty());
}

#[test]
fn a_check_that_does_not_guard_the_dispatch_states_no_bound() {
    // The same comparison, but its in-range edge is not the only way in.
    let mut second_way_in = records(true);
    second_way_in.push(FunctionRecord::Edge {
        from: 0x2c,
        target: Some(8),
        relation: EdgeKind::Jump,
        external: false,
    });
    assert!(run(&second_way_in, &facts(3, 0x38)).is_empty());
    // The comparison's taken edge, not its fall-through, enters the block.
    let mut wrong_side = records(true);
    for record in &mut wrong_side {
        if let FunctionRecord::Edge {
            relation, target, ..
        } = record
        {
            *relation = match *target {
                Some(8) => EdgeKind::Taken,
                _ => EdgeKind::Fallthrough,
            };
        }
    }
    assert!(run(&wrong_side, &facts(3, 0x38)).is_empty());
}

#[test]
fn a_rebased_switch_value_reports_its_first_case() {
    let mut rebased = records(true);
    // index = entry a1 + (-8): `switch (x) { case 8: … }`.
    rebased.push(FunctionRecord::Expression {
        id: 4,
        offset: 0,
        expression: Expression::Integer {
            op: IntegerOp::Add,
            left: AbstractValue::Expression { id: 0 },
            right: AbstractValue::Constant {
                value: (-8_i32) as u32,
            },
        },
    });
    for record in &mut rebased {
        match record {
            FunctionRecord::Expression {
                id: 1,
                expression: Expression::Integer { left, .. },
                ..
            } => *left = AbstractValue::Expression { id: 4 },
            FunctionRecord::Condition { right, .. }
            | FunctionRecord::Dispatch { index: right, .. } => {
                *right = AbstractValue::Expression { id: 4 }
            }
            _ => {}
        }
    }
    let tables = run(&rebased, &facts(3, 0x38));
    assert_eq!(tables.len(), 1);
    assert!(matches!(
        &tables[..],
        [JumpDispatch::Table {
            first_case: Some(8),
            ..
        }]
    ));
}

#[test]
fn the_recognizer_stops_when_the_work_budget_does() {
    let mut exhausted = || Err(Error::new(ErrorCode::ResourceLimited, "work budget"));
    let error = jump_tables(
        &records(true),
        TEXT,
        EXTENT,
        &facts(3, 0x38),
        memory(),
        &mut exhausted,
    )
    .err()
    .unwrap();
    assert_eq!(error.code, ErrorCode::ResourceLimited);
}

#[test]
fn the_first_case_is_read_through_narrow_extensions_or_left_unknown() {
    use super::first_case;
    let value = |id| AbstractValue::Expression { id };
    let number = |value: u32| AbstractValue::Constant { value };
    let integer = |op, left, right| Expression::Integer { op, left, right };
    let expressions = [
        Expression::EntryRegister { register: 10 },
        // `addi a0, a0, -8; andi a0, a0, 0xff`: `switch ((uint8_t) x)`.
        integer(IntegerOp::Add, value(0), number((-8_i32) as u32)),
        integer(IntegerOp::And, value(1), number(0xff)),
        // `slli 16; srli 16` around a constant-first addition.
        integer(IntegerOp::Add, number((-3_i32) as u32), value(0)),
        integer(IntegerOp::Shl, value(3), number(16)),
        integer(IntegerOp::Shr, value(4), number(16)),
        // A product says nothing about the case values.
        integer(IntegerOp::Mul, value(0), number(3)),
        integer(IntegerOp::And, value(0), number(0xff)),
    ];
    let expressions: Vec<&Expression> = expressions.iter().collect();
    assert_eq!(first_case(&value(2), &expressions), Some(8));
    assert_eq!(first_case(&value(5), &expressions), Some(3));
    assert_eq!(first_case(&value(6), &expressions), None);
    assert_eq!(first_case(&value(7), &expressions), Some(0));
    assert_eq!(first_case(&value(0), &expressions), Some(0));
    // An unknown index claims no case value.
    assert_eq!(first_case(&AbstractValue::Unknown, &expressions), None);
}

#[test]
fn an_unknown_index_proves_only_a_delimited_table_and_claims_no_case() {
    let mut unknown = records(true);
    for record in &mut unknown {
        if let FunctionRecord::Dispatch { index, .. } = record {
            *index = AbstractValue::Unknown;
        }
    }
    // The object's facts need a bound: no table.
    assert!(run(&unknown, &facts(3, 0x38)).is_empty());
    // Facts that delimit the table themselves, as an image's labelled table:
    // the whole table, with no case value claimed.
    struct Delimited;
    impl TableEntries for Delimited {
        fn entries<'m>(
            &self,
            _: &AbstractValue,
            count: Option<u32>,
            _: u32,
            memory: &'m WorkingMemory,
            control: &mut dyn RunControl,
        ) -> Result<Option<TableWords<'m>>> {
            assert_eq!(count, None);
            control.checkpoint(3)?;
            Ok(Some(TableWords {
                entries: vec![0x20, 0x30, 0x38],
                reserved: memory.reserve(24, control.position())?,
            }))
        }
    }
    let tables = jump_tables(&unknown, TEXT, EXTENT, &Delimited, memory(), &mut || Ok(()))
        .unwrap()
        .tables;
    assert_eq!(
        tables,
        [JumpDispatch::Table {
            site: 0x14,
            first_case: None,
            entries: vec![0x20, 0x30, 0x38],
        }]
    );
}

#[test]
fn a_table_is_the_same_whatever_case_value_a_pass_shows() {
    use crate::tables::JumpTableSource;
    let facts = facts(3, 0x38);
    let source = RecognizedTables {
        entries: &facts,
        section: TEXT,
    };
    let known = JumpDispatch::Table {
        site: 0x14,
        first_case: Some(0),
        entries: vec![0x20, 0x30, 0x38],
    };
    // A later pass whose index merged shows no case value: the same table.
    let mut merged = known.clone();
    if let JumpDispatch::Table { first_case, .. } = &mut merged {
        *first_case = None;
    }
    assert!(source.same(&known, &merged));
    // Other entries are another table.
    let other = JumpDispatch::Table {
        site: 0x14,
        first_case: Some(0),
        entries: vec![0x20, 0x30],
    };
    assert!(!source.same(&known, &other));
}
