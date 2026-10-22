//! Argument summaries use the load-use CFG engine, with entry-register sources.
use super::*;

#[derive(Clone, Copy)]
pub(super) struct ReturnedDependency {
    pub(super) register: u8,
    pub(super) dependency: ReadDependency,
}

/// Site resolution returns the shared model port's contract; this callback
/// does not produce or own callee facts.
pub trait CallAt<'s> {
    fn summary(&self, site: u64) -> Option<&'s CallSummary>;
    fn current(&self) -> Option<&'s CallSummary> {
        None
    }
}
impl<'s, F: Fn(u64) -> Option<&'s CallSummary>> CallAt<'s> for F {
    fn summary(&self, site: u64) -> Option<&'s CallSummary> {
        self(site)
    }
}

/// Origin of a contract audit, kept separate from the summary's ABI wiring.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArgumentSource {
    Entry(u8),
    MissingResult { site: u64, register: u8 },
}
type ObserveArgument<'v> =
    dyn FnMut(ArgumentSource, &ReadUse, &mut dyn RunControl) -> Result<()> + 'v;

/// A compact, owned recipe for the same CFG pass. Capture drops borrowed
/// expression/address facts and keeps only lifted operations, stack/constant
/// atoms, edges and dispatch producers. No instruction bytes or records persist.
pub struct ArgumentProgram<'m> {
    instructions: AdmittedVec<'m, ReadInstruction<'static>>,
    edges: AdmittedVec<'m, ReadEdge>,
    dispatches: AdmittedVec<'m, ReadDispatch>,
}
impl<'m> ArgumentProgram<'m> {
    pub fn capture(
        facts: &Facts<'_, 'm>,
        decoder: &dyn FunctionSemantics,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut evaluator = FlowEngine::<Reads>::new(facts, memory);
        evaluator.prepare(decoder, &mut |_, _, _| Ok(false), c)?;
        let mut instructions = AdmittedVec::new(memory);
        for node in evaluator.nodes.iter() {
            c.checkpoint(1)?;
            // The address is needed only for load/store identity, which is
            // irrelevant to entry sources. Its stack atom remains available.
            instructions.push(
                ReadInstruction {
                    address: None,
                    ..node.instruction
                },
                c.position(),
            )?;
        }
        Ok(Self {
            instructions,
            edges: evaluator.edges,
            dispatches: evaluator.dispatches,
        })
    }

    pub fn summarize(
        self,
        calls: &dyn CallAt<'_>,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<SummaryOutcome<(ArgumentSummary, MemoryReservation<'m>)>> {
        let mut work = self.work(c, false);
        self.summarize_inner(calls, memory, &mut work, None, false, u32::MAX)
    }

    pub fn summarize_observed(
        self,
        calls: &dyn CallAt<'_>,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
        observe: &mut ObserveArgument<'_>,
    ) -> Result<SummaryOutcome<(ArgumentSummary, MemoryReservation<'m>)>> {
        let mut work = self.work(c, false);
        self.summarize_inner(calls, memory, &mut work, Some(observe), false, u32::MAX)
    }

    /// A separate bounded audit tracks only the invalid entry words and absent
    /// results. Ordinary #317 summary production retains all eight entry sources.
    pub fn audit(
        self,
        calls: &dyn CallAt<'_>,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
        observe: &mut ObserveArgument<'_>,
        entry_registers: u32,
    ) -> Result<SummaryOutcome<(ArgumentSummary, MemoryReservation<'m>)>> {
        let mut work = self.work(c, true);
        self.summarize_inner(
            calls,
            memory,
            &mut work,
            Some(observe),
            true,
            entry_registers,
        )
    }

    fn work<'c>(&self, c: &'c mut dyn RunControl, audit: bool) -> ReadWork<'c> {
        // Audits have a separate 16x function allowance; all work still charges the operation.
        let limit =
            read_work_limit(self.instructions.len()).saturating_mul(if audit { 16 } else { 1 });
        ReadWork {
            caller: c,
            remaining: limit,
            budget: LocalBudgetExceeded {
                limit,
                scope: if audit {
                    LocalBudgetScope::SignatureAudit
                } else {
                    LocalBudgetScope::ArgumentSummary
                },
            },
            instruction_only: false,
        }
    }

    fn summarize_inner(
        self,
        calls: &dyn CallAt<'_>,
        memory: &'m WorkingMemory,
        work: &mut ReadWork<'_>,
        mut observe: Option<&mut ObserveArgument<'_>>,
        audit_results: bool,
        entry_registers: u32,
    ) -> Result<SummaryOutcome<(ArgumentSummary, MemoryReservation<'m>)>> {
        if self.instructions.is_empty() {
            return Ok(SummaryOutcome::NoProof);
        }
        let facts = Facts::new(&[], memory, work.caller)?;
        let mut evaluator = FlowEngine::<Reads>::new(&facts, memory);
        evaluator.summary_mode = true;
        evaluator.audit_results = audit_results;
        evaluator.entry_registers = entry_registers;
        evaluator.edges = self.edges;
        evaluator.dispatches = self.dispatches;

        let result = (|| {
            for instruction in self.instructions.iter().copied() {
                work.checkpoint(1)?;
                evaluator.nodes.push(
                    ReadNode {
                        instruction,
                        input: None,
                        queued: false,
                        unsettled: false,
                    },
                    work.position(),
                )?;
            }
            evaluator.solve(Some(CallAbi::RiscvInteger), calls, work)?;
            let mut consumed = [ArgumentConsumption::default(); 8];
            for source in evaluator.sources.iter() {
                let origin = match source.origin {
                    ReadOrigin::Entry(register) => ArgumentSource::Entry(register),
                    ReadOrigin::MissingResult { site, register } => {
                        ArgumentSource::MissingResult { site, register }
                    }
                    ReadOrigin::Load { .. } => return Err(read_integrity()),
                };
                let mut value = ArgumentConsumption::default();
                let mut event = source.event;
                while event != 0 {
                    work.checkpoint(1)?;
                    let entry = evaluator
                        .domain
                        .events
                        .get(event - 1)
                        .ok_or_else(read_integrity)?;
                    if let Some(observe) = observe.as_deref_mut() {
                        observe(origin, &entry.usage, work)?;
                    }
                    match entry.usage.precision {
                        ReadPrecision::Exact => value.exact |= entry.usage.bits,
                        ReadPrecision::May => value.may |= entry.usage.bits,
                    }
                    event = entry.next;
                }
                // Stack escape/unsupported dataflow remains consumption, never
                // a proof of an unused input. Structural gaps reject the recipe
                // in application before capture.
                value.may |= source.limits.iter().fold(0, |a, b| a | b);
                value.exact &= !value.may;
                if let ArgumentSource::Entry(register) = origin {
                    *consumed
                        .get_mut(usize::from(register - 10))
                        .ok_or_else(read_integrity)? = value;
                }
            }
            let mut ledger = Ledger::new(memory, work)?;
            let mut returned = Vec::new();
            for item in evaluator.domain.returned.iter() {
                work.checkpoint(1)?;
                let ReadOrigin::Entry(argument) = evaluator
                    .sources
                    .get(item.dependency.source)
                    .ok_or_else(read_integrity)?
                    .origin
                else {
                    continue;
                };
                ledger.push(
                    &mut returned,
                    ReturnedArgument {
                        argument,
                        register: item.register,
                        exact: item.dependency.exact,
                        may: item.dependency.may,
                        inverted: item.dependency.inverted,
                    },
                    work,
                )?;
            }
            returned.sort_unstable_by_key(|r| (r.argument, r.register));
            Ok((
                ArgumentSummary { consumed, returned },
                ledger.into_reservation(),
            ))
        })();
        match result {
            Ok(summary) => Ok(SummaryOutcome::Available(summary)),
            Err(Error {
                code: ErrorCode::LocalBudgetExceeded(reason),
                ..
            }) => Ok(SummaryOutcome::LocalBudgetExceeded(reason)),
            Err(error) => Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn returning(memory: &WorkingMemory) -> ArgumentProgram<'_> {
        let mut instructions = AdmittedVec::new(memory);
        instructions
            .push(
                ReadInstruction {
                    site: 0,
                    opaque: 0,
                    operation: SemanticOp::None,
                    branch: None,
                    flow: InstructionFlow::Return { base: 1 },
                    address: None,
                    value: None,
                    address_atom: ReadAtom::Unknown,
                    source: None,
                    arguments: false,
                    call: false,
                    dispatch: false,
                    edge_start: 0,
                    edge_end: 0,
                },
                RunPosition::default(),
            )
            .expect("return instruction");
        ArgumentProgram {
            instructions,
            edges: AdmittedVec::new(memory),
            dispatches: AdmittedVec::new(memory),
        }
    }

    #[test]
    fn return_contract_keeps_positions_without_consuming_entry_bits() {
        let memory = WorkingMemory::new(1 << 20).expect("memory");
        let SummaryOutcome::Available((summary, reservation)) = returning(&memory)
            .summarize(&|_| None, &memory, &mut || Ok(()))
            .expect("summary")
        else {
            panic!("complete summary")
        };
        assert!(summary.consumed.iter().all(|c| c.exact == 0 && c.may == 0));
        assert_eq!(summary.returned.len(), 2);
        for returned in summary.returned.iter() {
            assert_eq!(returned.argument, returned.register);
            assert_eq!(returned.exact, std::array::from_fn(|i| 1 << i));
            assert_eq!(returned.may, [0; 32]);
        }
        assert!(memory.used() > 0);
        drop(summary);
        drop(reservation);
        assert_eq!(memory.used(), 0);
    }

    #[test]
    fn failed_summary_admission_and_work_release_the_recipe() {
        let memory = WorkingMemory::new(1 << 20).expect("memory");
        let program = returning(&memory);
        let blocker = memory
            .reserve(
                memory.observation().limit_bytes - memory.used(),
                RunPosition::default(),
            )
            .expect("block remaining memory");
        assert!(
            program
                .summarize(&|_| None, &memory, &mut || Ok(()))
                .is_err_and(
                    |error| error.code == ErrorCode::ResourceLimited && error.memory.is_some()
                )
        );
        drop(blocker);
        assert_eq!(memory.used(), 0);
        let mut calls = 0;
        let result = returning(&memory)
            .summarize(&|_| None, &memory, &mut || {
                calls += 1;
                if calls > 3 {
                    Err(Error::new(ErrorCode::ResourceLimited, "fixture work limit"))
                } else {
                    Ok(())
                }
            })
            .err()
            .expect("operation failure");
        assert_eq!(
            result,
            Error::new(ErrorCode::ResourceLimited, "fixture work limit")
        );
        assert_eq!(memory.used(), 0);
        let error = returning(&memory)
            .summarize(&|_| None, &memory, &mut || {
                Err(Error::new(ErrorCode::Cancelled, "cancelled"))
            })
            .err()
            .expect("cancellation propagates");
        assert_eq!(error.code, ErrorCode::Cancelled);
        assert_eq!(memory.used(), 0);
    }
    #[test]
    fn local_summary_and_audit_exhaustion_are_distinct_from_no_proof() {
        let memory = WorkingMemory::new(1 << 20).expect("memory");
        for audit in [false, true] {
            let program = returning(&memory);
            let mut caller = || Ok(());
            let mut work = program.work(&mut caller, audit);
            work.remaining = 1;
            let reason = work.budget;
            let outcome = program
                .summarize_inner(&|_| None, &memory, &mut work, None, audit, u32::MAX)
                .expect("local degradation");
            assert_eq!(
                outcome.status(),
                SummaryOutcome::LocalBudgetExceeded(reason)
            );
            drop(outcome);
            assert_eq!(memory.used(), 0);
            assert!(matches!(
                returning(&memory)
                    .summarize(&|_| None, &memory, &mut caller)
                    .expect("next function"),
                SummaryOutcome::Available(_)
            ));
        }
        let empty = ArgumentProgram {
            instructions: AdmittedVec::new(&memory),
            edges: AdmittedVec::new(&memory),
            dispatches: AdmittedVec::new(&memory),
        };
        assert_eq!(
            empty
                .summarize(&|_| None, &memory, &mut || Ok(()))
                .expect("empty recipe")
                .status(),
            SummaryOutcome::NoProof
        );
        assert_eq!(memory.used(), 0);
    }

    #[test]
    fn parent_failures_inside_summary_and_audit_are_unchanged() {
        for expected in [
            Error::new(ErrorCode::ResourceLimited, "work units exhausted"),
            Error::new(ErrorCode::ResourceLimited, "deadline exceeded"),
            Error::new(ErrorCode::TimedOut, "deadline fixture"),
            Error::new(ErrorCode::Cancelled, "cancel fixture"),
        ] {
            for audit in [false, true] {
                let memory = WorkingMemory::new(1 << 20).expect("memory");
                let mut checkpoints = 0;
                let mut c = || {
                    checkpoints += 1;
                    if checkpoints > 3 {
                        Err(expected.clone())
                    } else {
                        Ok(())
                    }
                };
                let program = returning(&memory);
                let result = if audit {
                    program.audit(&|_| None, &memory, &mut c, &mut |_, _, _| Ok(()), u32::MAX)
                } else {
                    program.summarize(&|_| None, &memory, &mut c)
                };
                assert_eq!(result.err().expect("parent failure"), expected);
                assert_eq!(memory.used(), 0);
            }
        }
        for audit in [false, true] {
            let memory = WorkingMemory::new(1 << 20).expect("memory");
            let program = returning(&memory);
            let blocker = memory
                .reserve(
                    memory.observation().limit_bytes - memory.used(),
                    RunPosition::default(),
                )
                .expect("block memory");
            let result = if audit {
                program.audit(
                    &|_| None,
                    &memory,
                    &mut || Ok(()),
                    &mut |_, _, _| Ok(()),
                    u32::MAX,
                )
            } else {
                program.summarize(&|_| None, &memory, &mut || Ok(()))
            };
            let error = result.err().expect("memory failure");
            assert_eq!(error.code, ErrorCode::ResourceLimited);
            assert!(error.memory.is_some());
            drop(blocker);
            assert_eq!(memory.used(), 0);
        }
    }
}
