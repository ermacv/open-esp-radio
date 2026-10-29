//! Real decoder/ELF regressions for a table dispatch split by a conditional.
use super::*;
use oer_analysis_riscv_model::{FunctionRecord, FunctionSink};

fn branch(from: usize, to: usize, register: u32) -> u32 {
    let delta = (to as u32).wrapping_sub(from as u32).wrapping_mul(4);
    ((delta >> 12 & 1) << 31)
        | ((delta >> 5 & 0x3f) << 25)
        | (register << 15)
        | (1 << 12)
        | ((delta >> 1 & 0xf) << 8)
        | ((delta >> 11 & 1) << 7)
        | 0x63
}

fn root(unknown: bool) -> Symbol {
    Symbol {
        name: "dispatch",
        frame: Some(0),
        words: vec![
            branch(0, 4, 10),
            0x0000_2737, // lui a4, 2
            0x0007_0713, // addi a4, a4, 0
            call(TEXT + 12, TEXT + 28) & !(31 << 7),
            0x0000_2737,                                     // lui a4, 2
            if unknown { 0x0005_8713 } else { 0x0087_0713 }, // mv a4, a1 / addi a4, a4, 8
            0x0000_0013,
            // The real shape: lbu; slli; add; lw; bnez; add; lw; jr.
            0x0006_c783, // lbu a5, 0(a3)
            0x0027_9793, // slli a5, a5, 2
            0x00fc_8533, // add a0, s9, a5
            0x0605_2503, // lw a0, 96(a0)
            branch(11, 18, 10),
            0x00e7_87b3, // add a5, a5, a4
            0x0007_a503, // lw a0, 0(a5)
            0x0005_0067, // jalr zero, 0(a0)
            RET,
            RET,
            RET,
            RET,
        ],
    }
}

fn fixture(root: Symbol, entries: &[u32]) -> Vec<u8> {
    built(
        TEXT,
        &[root],
        &Rodata {
            words: entries,
            relocated: &[0, 1, 2, 3],
            objects: &[(".LJTI0_0", 0, 0), (".LJTI0_1", 8, 0)],
        },
    )
}

#[derive(Default)]
struct Records(Vec<FunctionRecord>);
impl FunctionSink for Records {
    fn record(&mut self, record: &FunctionRecord, _: &mut dyn RunControl) -> Result<()> {
        self.0.push(record.clone());
        Ok(())
    }
}

fn capture(elf: &[u8]) -> Vec<FunctionRecord> {
    with_facts(elf, |facts| {
        let memory = WorkingMemory::new(TEST_MEMORY)?;
        let mut records = Records::default();
        inspect_function(
            facts,
            &facts.functions()[0],
            &UnknownCallBehavior,
            oer_analysis_riscv_analysis::ResearchOptions::default(),
            &memory,
            &mut || Ok(()),
            &mut records,
        )?;
        Ok(records.0)
    })
    .unwrap()
}

fn targets(records: &[FunctionRecord]) -> Vec<u64> {
    records
        .iter()
        .filter_map(|record| match record {
            FunctionRecord::Edge {
                from,
                target,
                relation: oer_analysis_riscv_model::EdgeKind::Jump,
                ..
            } if *from == u64::from(TEXT + 56) => *target,
            _ => None,
        })
        .collect()
}

const ENTRIES: [u32; 4] = [TEXT + 60, TEXT + 64, TEXT + 64, TEXT + 68];

#[test]
fn the_real_split_dispatch_shape_retains_its_preheader_base() {
    let mut code = root(false);
    code.words[5] = 0x0007_0713; // the same table base on both incoming paths
    let records = capture(&fixture(code, &ENTRIES));
    assert_eq!(targets(&records), [TEXT + 60, TEXT + 64].map(u64::from));
    assert!(
        records
            .iter()
            .all(oer_analysis_riscv_analysis::writes::closes_local_graph)
    );
}

#[test]
fn split_dispatch_unites_every_proved_base_and_closes_the_common_cfg() {
    // Odd table words exercise JALR's low-bit rule before CFG decoding.
    let elf = fixture(root(false), &ENTRIES.map(|target| target | 1));
    let records = capture(&elf);
    assert_eq!(
        targets(&records),
        [TEXT + 60, TEXT + 64, TEXT + 68].map(u64::from)
    );
    assert!(
        records
            .iter()
            .all(oer_analysis_riscv_analysis::writes::closes_local_graph)
    );
    let analysis = analyze(&elf, &[], &[]).unwrap();
    assert!(analysis.functions[&TEXT].complete);
    assert_eq!(
        analysis
            .bound(TEXT, &mut crate::tests::ledger(), &mut || Ok(()))
            .unwrap()
            .bytes,
        Some(0)
    );
}

#[test]
fn one_unknown_incoming_base_blocks_closure() {
    let records = capture(&fixture(root(true), &ENTRIES));
    assert!(targets(&records).is_empty());
    assert!(
        !records
            .iter()
            .all(oer_analysis_riscv_analysis::writes::closes_local_graph)
    );
}

#[test]
fn a_discarded_target_load_does_not_prove_a_jump_through_zero() {
    let mut code = root(false);
    code.words[13] = 0x0007_a003; // lw zero, 0(a5)
    code.words[14] = 0x0000_0067; // jalr zero, 0(zero)
    let records = capture(&fixture(code, &ENTRIES));
    assert!(targets(&records).is_empty());
    assert!(
        !records
            .iter()
            .all(oer_analysis_riscv_analysis::writes::closes_local_graph)
    );
}

#[test]
fn an_unproved_table_alternative_blocks_the_entire_dispatch() {
    let mut code = root(false);
    code.words[5] = 0x0107_0713; // second base points beyond both known tables
    let records = capture(&fixture(code, &ENTRIES));
    assert!(targets(&records).is_empty());
    assert!(
        !records
            .iter()
            .all(oer_analysis_riscv_analysis::writes::closes_local_graph)
    );
}

#[test]
fn a_new_incoming_case_retracts_the_stale_dispatch_proof() {
    // The first pass sees a unique predecessor for the load. Following this
    // table creates a bypass with unknown a5; revalidation must retract it.
    let records = capture(&fixture(
        root(false),
        &[TEXT + 52, TEXT + 64, TEXT + 64, TEXT + 68],
    ));
    assert!(targets(&records).is_empty());
    assert!(
        !records
            .iter()
            .all(oer_analysis_riscv_analysis::writes::closes_local_graph)
    );
}

#[test]
fn proved_dispatch_does_not_hide_a_decode_gap_or_another_unknown_edge() {
    for instruction in [0xffff_ffff, 0x0003_0067] {
        // decode gap / jr t1 (t0 is an ISA return-address register)
        let mut code = root(false);
        code.words[18] = instruction;
        let records = capture(&fixture(code, &ENTRIES));
        assert_eq!(
            targets(&records),
            [TEXT + 60, TEXT + 64, TEXT + 68].map(u64::from)
        );
        assert!(
            !records
                .iter()
                .all(oer_analysis_riscv_analysis::writes::closes_local_graph)
        );
    }
}

#[test]
fn a_table_reachable_unimp_keeps_both_observers_open_at_unknown() {
    let mut code = root(false);
    code.words[17] = 0; // The table has an independent edge to c.unimp.
    let elf = fixture(code, &ENTRIES);
    let structural = capture(&elf);
    assert!(
        !structural
            .iter()
            .all(oer_analysis_riscv_analysis::writes::closes_local_graph)
    );
    assert!(structural.iter().any(|record| matches!(record,
        FunctionRecord::Edge { from, target: Some(target), .. }
            if *from == u64::from(TEXT + 56) && *target == u64::from(TEXT + 68)
    )));
    with_facts(&elf, |facts| {
        let memory = WorkingMemory::new(TEST_MEMORY)?;
        let function = &facts.functions()[0];
        let mut work = kept(&memory);
        // This split dispatch is discovered by DispatchTables, not the sweep.
        assert!(
            sweep::transfers(facts, function, &mut work, &mut || Ok(()))?
                .jumps
                .is_empty()
        );
        let tables = DispatchTables {
            recognized: RecognizedTables {
                entries: &ReadOnlyTables { image: facts },
                section: function.section as u32,
            },
            known: &[],
        };
        image::observing(
            facts,
            &UnknownCallBehavior,
            oer_analysis_riscv_analysis::TrapSemantics::Unknown,
            &memory,
            &mut || Ok(()),
            |observe, control| {
                let observed = observe(function, &[], &tables, &mut work, control)?;
                assert!(!observed.closed_graph());
                assert!(!observed.complete);
                assert!(observed.predecessors[&(TEXT + 68)].contains(&(TEXT + 56)));
                Ok(())
            },
        )
    })
    .unwrap();
}

/// The displaced state dispatch excludes byte values zero and one, then reads a
/// table whose label is eight bytes beyond the nominal indexed load start.
fn displaced_root() -> Symbol {
    Symbol {
        name: "displaced",
        frame: Some(0),
        words: vec![
            0x0005_4783, // lbu a5, 0(a0)
            0x0fe7_f713, // andi a4, a5, 0xfe
            0x0207_0063, // beq a4, zero, +32 (the excluded 0/1 states)
            0x0000_2737, // lui a4, 2
            0x0007_0713, // addi a4, a4, 0: labelled base RODATA
            0x0027_9793, // slli a5, a5, 2
            0x00e7_87b3, // add a5, a5, a4
            0xff87_a783, // lw a5, -8(a5)
            0x0007_8067, // jr a5
            RET,         // table entry zero
            RET,         // table entry one and the excluded states
        ],
    }
}

fn recognized(elf: &[u8]) -> (Vec<FunctionRecord>, Vec<JumpDispatch>) {
    with_facts(elf, |facts| {
        use oer_analysis_riscv_analysis::{
            FunctionInput, PreparedReferences, research_following_tables,
        };
        use oer_analysis_riscv_model::NoSummaries;
        let memory = WorkingMemory::new(TEST_MEMORY)?;
        let view = oer_analysis_riscv_program::ProgramView::new(
            facts.bytes(),
            facts.file(),
            &[],
            &memory,
            &mut || Ok(()),
        )?;
        let function = &facts.functions()[0];
        let references = PreparedReferences::empty();
        let mut records = Records::default();
        let (_, found) = research_following_tables(
            FunctionInput {
                traps: oer_analysis_riscv_analysis::TrapSemantics::Unknown,
                image: Some(&view),
                section: function.section as u32,
                extent: CodeRange {
                    start: function.address.into(),
                    length: function.size.into(),
                },
                bytes: image::function_bytes(facts.file(), function)?,
                relocations: &references,
                data_ranges: &[],
                jumps: &[],
                returns: &NoSummaries,
                call_behavior: &UnknownCallBehavior,
            },
            &RecognizedTables {
                entries: &ReadOnlyTables { image: facts },
                section: function.section as u32,
            },
            &oer_analysis_riscv_lift::StructuralRiscvDecoder,
            &memory,
            &mut || Ok(()),
            &mut records,
            None,
        )?;
        Ok((records.0, found.tables))
    })
    .unwrap()
}

#[test]
fn the_displaced_state_dispatch_proves_a_table_and_closes_the_stack_graph() {
    let elf = built(
        TEXT,
        &[displaced_root()],
        &Rodata {
            words: &[TEXT + 36, TEXT + 40, TEXT + 36],
            relocated: &[0, 1, 2],
            objects: &[(".LJTI0_0", 0, 0)],
        },
    );
    let (records, found) = recognized(&elf);
    assert!(
        records
            .iter()
            .all(oer_analysis_riscv_analysis::writes::closes_local_graph)
    );
    assert_eq!(
        found,
        [JumpDispatch::Table {
            site: (TEXT + 32).into(),
            first_case: Some(2),
            entries: vec![(TEXT + 36).into(), (TEXT + 40).into(), (TEXT + 36).into()],
        }]
    );
    assert_eq!(
        analyze(&elf, &[], &[])
            .unwrap()
            .bound(TEXT, &mut crate::tests::ledger(), &mut || Ok(()))
            .unwrap()
            .bytes,
        Some(0)
    );
}

#[test]
fn a_displaced_table_is_revalidated_after_following_its_targets() {
    // Entry at the load bypasses the add and introduces a second predecessor.
    let elf = built(
        TEXT,
        &[displaced_root()],
        &Rodata {
            words: &[TEXT + 28, TEXT + 40],
            relocated: &[0, 1],
            objects: &[(".LJTI0_0", 0, 0)],
        },
    );
    let (records, found) = recognized(&elf);
    assert!(found.is_empty());
    assert!(
        !records
            .iter()
            .all(oer_analysis_riscv_analysis::writes::closes_local_graph)
    );
    assert!(!records.iter().any(|record| matches!(record,
        FunctionRecord::Edge { from, target: Some(_), relation: oer_analysis_riscv_model::EdgeKind::Jump, external: false }
            if *from == u64::from(TEXT + 32)
    )));
}
