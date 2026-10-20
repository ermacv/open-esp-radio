//! Image proof made with unknown trap behavior, before terminal traps can
//! close any function. Platform callers supply every hart's exception entry.
use crate::{Analysis, TrapEntry, TrapSemantics, inspect_function_as, trap_entry};
use oer_analysis_riscv_analysis::{KnownJump, ResearchOptions, writes::closes_local_graph};
use oer_analysis_riscv_lift::RiscvDecoder;
use oer_analysis_riscv_model::*;
use oer_analysis_riscv_program::facts::ImageFacts;
use std::{collections::BTreeMap, fmt};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TrapFailure {
    /// No platform exception entries, or an unknown entry for a hart (a).
    Entry {
        hart: Option<u32>,
        entry: Option<u32>,
    },
    /// The strict graph does not prove the handler cannot return (b).
    Returns { hart: u32, handler: u32 },
    /// The exception root reaches a level-lowering instruction (c).
    LevelDrop { hart: u32, handler: u32 },
}

/// The checked entries and first failed obligation of one image.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrapFact {
    pub semantics: TrapSemantics,
    pub entries: Vec<(u32, TrapEntry)>,
    pub failure: Option<TrapFailure>,
}

impl fmt::Display for TrapFact {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "traps: {:?}", self.semantics)?;
        if let Some(failure) = self.failure {
            match failure {
                TrapFailure::Entry { hart, entry } => {
                    write!(f, " (a: unchecked exception entry ")?;
                    if let Some(entry) = entry {
                        write!(f, "{entry:#010x}")?;
                    } else {
                        write!(f, "missing")?;
                    }
                    write!(f, " for hart {hart:?})")
                }
                TrapFailure::Returns { hart, handler } => write!(
                    f,
                    " (b: hart {hart} handler {handler:#010x} not proven noreturn)"
                ),
                TrapFailure::LevelDrop { hart, handler } => write!(
                    f,
                    " (c: hart {hart} exception root {handler:#010x} lowers the level)"
                ),
            }?;
        }
        for (hart, entry) in &self.entries {
            write!(
                f,
                "; hart {hart} entry {:#010x} -> handler {:#010x}",
                entry.entry, entry.handler
            )?;
        }
        Ok(())
    }
}

/// Derive terminal illegal-instruction semantics from every hart. `unknown`
/// must have been analyzed at Unknown, including the exception root's bound.
/// Missing entries and ordinary proof failures retain Unknown; cancellation
/// and exhausted resources remain errors.
pub fn prove_traps(
    image: &ImageFacts<'_>,
    unknown: &Analysis,
    entries: &[(u32, Option<u32>)],
    call_behavior: &dyn CallBehavior,
    keep: &mut Ledger<'_>,
    control: &mut dyn RunControl,
) -> Result<TrapFact> {
    if unknown.traps != TrapSemantics::Unknown {
        return Err(Error::new(
            ErrorCode::Integrity,
            "trap proof requires Unknown semantics",
        ));
    }
    let mut fact = TrapFact::default();
    if entries.is_empty() {
        fact.failure = Some(TrapFailure::Entry {
            hart: None,
            entry: None,
        });
        return Ok(fact);
    }
    for &(hart, address) in entries {
        control.checkpoint(1)?;
        let entry = if let Some(address) = address {
            match trap_entry(image, address, control) {
                Ok(entry) => Some(entry),
                Err(error) if error.code == ErrorCode::Integrity => None,
                Err(error) => return Err(error),
            }
        } else {
            None
        };
        let Some(entry) = entry else {
            fact.failure = Some(TrapFailure::Entry {
                hart: Some(hart),
                entry: address,
            });
            return Ok(fact);
        };
        keep.push(&mut fact.entries, (hart, entry), control)?;
        if !unknown.functions.contains_key(&entry.handler) {
            fact.failure = Some(TrapFailure::Returns {
                hart,
                handler: entry.handler,
            });
            return Ok(fact);
        }
        // Check the conservative swept bound as well as the strict reachable
        // graph: trap returns and CSR writes must never justify noreturn.
        if !unknown.bound(entry.handler)?.level_drops.is_empty() {
            fact.failure = Some(TrapFailure::LevelDrop {
                hart,
                handler: entry.handler,
            });
            return Ok(fact);
        }
        if !non_returning(image, entry.handler, call_behavior, keep.memory(), control)? {
            fact.failure = Some(TrapFailure::Returns {
                hart,
                handler: entry.handler,
            });
            return Ok(fact);
        }
    }
    fact.semantics = TrapSemantics::Terminal;
    Ok(fact)
}

struct Exits<'l, 'm> {
    closed: bool,
    returned: bool,
    tails: Vec<u32>,
    refinements: Vec<KnownJump>,
    extent: CodeRange,
    ledger: &'l mut Ledger<'m>,
}
impl FunctionSink for Exits<'_, '_> {
    fn record(&mut self, record: &FunctionRecord, control: &mut dyn RunControl) -> Result<()> {
        self.closed &= closes_local_graph(record);
        match record {
            FunctionRecord::Transfer {
                offset,
                target,
                call: false,
            } => {
                let mut targets = Vec::new();
                match target {
                    AbstractValue::ImageAddress { address } => {
                        self.ledger
                            .push(&mut targets, u64::from(*address), control)?
                    }
                    AbstractValue::Alternatives { values } => {
                        for value in values.values() {
                            control.checkpoint(1)?;
                            let ValueAlternative::ImageAddress { address } = value else {
                                return Ok(());
                            };
                            self.ledger
                                .push(&mut targets, u64::from(*address), control)?;
                        }
                    }
                    _ => return Ok(()),
                }
                if !targets.is_empty()
                    && targets.iter().all(|t| {
                        *t < self.extent.start || *t - self.extent.start >= self.extent.length
                    })
                {
                    targets.sort_unstable();
                    targets.dedup();
                    self.ledger.push(
                        &mut self.refinements,
                        KnownJump {
                            site: *offset,
                            targets,
                        },
                        control,
                    )?;
                }
            }
            FunctionRecord::Edge {
                relation: EdgeKind::Return,
                ..
            } => self.returned = true,
            FunctionRecord::Edge {
                relation:
                    EdgeKind::Jump
                    | EdgeKind::Taken
                    | EdgeKind::Fallthrough
                    | EdgeKind::PossibleContinuation,
                target: Some(target),
                external: true,
                ..
            } => {
                self.ledger.push(&mut self.tails, *target as u32, control)?;
            }
            FunctionRecord::SemanticGap {
                reason: SemanticGapReason::UnexpandedControlFlow,
                ..
            } => self.closed = false,
            _ => {}
        }
        Ok(())
    }
}

/// Use canonical CFG/noreturn pruning with the strict decoder. A closed
/// graph without Return may loop locally; every external exit must itself
/// be proven noreturn. No function is accepted solely for its own DWARF flag.
fn non_returning(
    image: &ImageFacts<'_>,
    root: u32,
    behavior: &dyn CallBehavior,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<bool> {
    let mut work = Ledger::new(memory, control)?;
    let mut pending = Vec::new();
    let mut graphs = BTreeMap::new();
    work.push(&mut pending, root, control)?;
    while let Some(address) = pending.pop() {
        control.checkpoint(1)?;
        if graphs.contains_key(&address) {
            continue;
        }
        let mut function = None;
        for candidate in image.functions() {
            control.checkpoint(1)?;
            if candidate.address == address {
                function = Some(candidate);
                break;
            }
        }
        let Some(function) = function else {
            return Ok(false);
        };
        let extent = CodeRange {
            start: address.into(),
            length: function.size.into(),
        };
        let options = ResearchOptions {
            abi: Some(CallAbi::RiscvInteger),
            ..Default::default()
        };
        let mut exits = Exits {
            closed: true,
            returned: false,
            tails: Vec::new(),
            refinements: Vec::new(),
            extent,
            ledger: &mut work,
        };
        inspect_function_as(
            image,
            function,
            behavior,
            options,
            &[],
            &RiscvDecoder,
            TrapSemantics::Unknown,
            memory,
            control,
            &mut exits,
        )?;
        if !exits.closed && !exits.refinements.is_empty() {
            // A value-proven external tail has no local successor in either
            // pass. Rebuild its canonical edge and recheck the exact same
            // all-path targets, rather than relaxing the closure predicate.
            let refinements = exits.refinements;
            exits = Exits {
                closed: true,
                returned: false,
                tails: Vec::new(),
                refinements: Vec::new(),
                extent,
                ledger: &mut work,
            };
            inspect_function_as(
                image,
                function,
                behavior,
                options,
                &refinements,
                &RiscvDecoder,
                TrapSemantics::Unknown,
                memory,
                control,
                &mut exits,
            )?;
            if refinements != exits.refinements {
                return Ok(false);
            }
        }
        if !exits.closed || exits.returned {
            return Ok(false);
        }
        let tails = exits.tails;
        for &tail in &tails {
            if !behavior.noreturn(Callee::Address(tail)) {
                work.push(&mut pending, tail, control)?;
            }
        }
        work.insert(&mut graphs, address, (), control)?;
    }
    // Every explored graph has no return, no unknown exit and only edges to
    // other such graphs or admitted noreturn callees. Cycles are infinite.
    Ok(true)
}
