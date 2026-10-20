//! Replay the stack observer's exact inputs, alongside the structural example.
use crate::{image, sweep};
use oer_analysis_riscv_analysis::{
    FunctionInput, JumpTableSource, KnownJump, PreparedReferences, ProvenTables, RecognizedTables,
    research_following_tables, writes::closes_local_graph,
};
use oer_analysis_riscv_lift::RiscvDecoder;
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::{ProgramView, facts::ImageFacts, tables::ImageTables};

// Same DispatchTables contract as the stack pipeline: the sweep owns known
// jumps, while recognized tables are re-proved on every common-engine pass.
struct DispatchTables<'a> {
    recognized: RecognizedTables<'a>,
    known: &'a [KnownJump],
}
impl JumpTableSource for DispatchTables<'_> {
    type Table = JumpDispatch;
    fn same(&self, earlier: &JumpDispatch, later: &JumpDispatch) -> bool {
        self.recognized.same(earlier, later)
    }
    fn tables<'m>(
        &self,
        records: &[FunctionRecord],
        extent: CodeRange,
        memory: &'m WorkingMemory,
        control: &mut dyn RunControl,
    ) -> Result<ProvenTables<'m, JumpDispatch>> {
        let mut found = self.recognized.tables(records, extent, memory, control)?;
        found
            .tables
            .retain(|table| !self.known.iter().any(|jump| jump.site == table.site()));
        Ok(found)
    }
    fn jump(&self, table: &JumpDispatch) -> KnownJump {
        self.recognized.jump(table)
    }
}

// Observer and diagnostics share the common closure predicate. The replay
// still checks its aggregate against the actual source-compiled Observer.
fn observer_closes(record: &FunctionRecord) -> bool {
    closes_local_graph(record)
}

struct Diagnostic {
    closed: bool,
    common_closed: bool,
    differences: usize,
}

fn location(record: &FunctionRecord) -> String {
    match record {
        FunctionRecord::Gap { start, reason, .. } => format!("Gap\t{start:#010x}\t{reason:?}"),
        FunctionRecord::Edge {
            from,
            target,
            relation,
            ..
        } => {
            format!("Edge\t{from:#010x}\t{relation:?}, target={target:#x?}")
        }
        _ => format!("{record:?}"),
    }
}

impl FunctionSink for Diagnostic {
    fn record(&mut self, record: &FunctionRecord, _: &mut dyn RunControl) -> Result<()> {
        let observer = observer_closes(record);
        let writes = closes_local_graph(record);
        if self.closed && !observer {
            println!(
                "stack-observation-first-break\t{}\tobserver={observer}\twrites={writes}",
                location(record)
            );
        }
        self.closed &= observer;
        self.common_closed &= writes;
        if observer != writes {
            self.differences += 1;
            println!(
                "stack-observation-difference\tobserver={observer}\twrites={writes}\t{}",
                location(record)
            );
        }
        Ok(())
    }
}

pub fn print(
    facts: &ImageFacts<'_>,
    function: &image::Function,
    call_behavior: &dyn CallBehavior,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<()> {
    let mut work = Ledger::new(memory, control)?;
    let swept = sweep::transfers(facts, function, &mut work, control)?;
    let mut jumps = Vec::new();
    for (site, entries) in &swept.jumps {
        let mut targets: Vec<u64> = entries
            .iter()
            .copied()
            .filter(|entry| *entry != 0)
            .map(|entry| entry & !1)
            .filter(|target| {
                *target >= function.address && *target - function.address < function.size
            })
            .map(u64::from)
            .collect();
        targets.sort_unstable();
        targets.dedup();
        jumps.push(KnownJump {
            site: (*site).into(),
            targets,
        });
    }
    let tables = DispatchTables {
        recognized: RecognizedTables {
            entries: &ImageTables { image: facts },
            section: function.section as u32,
        },
        known: &jumps,
    };
    let observed_closed = image::observing(
        facts,
        call_behavior,
        oer_analysis_riscv_analysis::TrapSemantics::Unknown,
        memory,
        control,
        |observe, control| {
            let observed = observe(function, &jumps, &tables, &mut work, control)?;
            println!(
                "stack-observation-closed_graph\t{}",
                observed.closed_graph()
            );
            Ok(observed.closed_graph())
        },
    )?;
    let view = ProgramView::new(facts.bytes(), facts.file(), &[], memory, control)?;
    let references =
        PreparedReferences::new(&[], function.section as u32, &RiscvDecoder, memory, control)?;
    let mut diagnostic = Diagnostic {
        closed: true,
        common_closed: true,
        differences: 0,
    };
    research_following_tables(
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
            jumps: &jumps,
            returns: &NoReturns,
            call_behavior,
        },
        &tables,
        &RiscvDecoder,
        memory,
        control,
        &mut diagnostic,
        Some(CallAbi::RiscvInteger),
    )?;
    if diagnostic.closed != observed_closed {
        return Err(Error::new(
            ErrorCode::Integrity,
            "diagnostic observer predicate differs from image::Observer",
        ));
    }
    if diagnostic.closed {
        println!("stack-observation-first-break\tnone");
    }
    println!("stack-observation-differences\t{}", diagnostic.differences);
    println!(
        "stack-observation-writes-closed_graph\t{}",
        diagnostic.common_closed
    );
    Ok(())
}
