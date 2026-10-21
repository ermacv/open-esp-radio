//! Argument summaries use the load-use CFG engine, with entry-register sources.
use super::*;

#[derive(Clone, Copy, Default)]
struct Consumed {
    exact: u32,
    may: u32,
}

#[derive(Clone, Copy)]
pub(super) struct ReturnedArgument {
    register: u8,
    dependency: ReadDependency,
}

/// Internal bit-flow contract; no register-access wire schema is added.
/// Return mappings retain output positions and inversion, so only pure wiring
/// can subsequently prove same-word preservation in the caller.
pub struct ArgumentSummary<'m> {
    consumed: [Consumed; 8],
    returned: AdmittedVec<'m, ReturnedArgument>,
}

/// A missing summary consumes every argument bit with May precision.
pub trait ArgumentSummaries {
    fn summary(&self, site: u64) -> Option<&ArgumentSummary<'_>>;
}
impl ArgumentSummaries for () {
    fn summary(&self, _: u64) -> Option<&ArgumentSummary<'_>> {
        None
    }
}

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
        let mut evaluator = ReadUsesEvaluator::new(facts, memory);
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

    pub fn calls(&self) -> impl Iterator<Item = u64> + '_ {
        self.instructions
            .iter()
            .filter(|i| i.arguments)
            .map(|i| i.site)
    }

    pub fn summarize(
        self,
        calls: &dyn ArgumentSummaries,
        memory: &'m WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Option<ArgumentSummary<'m>>> {
        if self.instructions.is_empty() {
            return Ok(None);
        }
        let facts = Facts::new(&[], memory, c)?;
        let mut evaluator = ReadUsesEvaluator::new(&facts, memory);
        evaluator.summary_mode = true;
        evaluator.edges = self.edges;
        evaluator.dispatches = self.dispatches;
        let mut work = ReadWork {
            caller: c,
            remaining: read_work_limit(self.instructions.len()),
        };
        let result = (|| {
            for instruction in self.instructions.iter().copied() {
                work.checkpoint(1)?;
                evaluator.nodes.push(
                    ReadNode {
                        instruction,
                        input: None,
                        queued: false,
                    },
                    work.position(),
                )?;
            }
            evaluator.solve(Some(CallAbi::RiscvInteger), calls, &mut work)?;
            let mut consumed = [Consumed::default(); 8];
            for source in evaluator.sources.iter() {
                let ReadOrigin::Entry(register) = source.origin else {
                    return Err(read_integrity());
                };
                let mut value = Consumed::default();
                let mut event = source.event;
                while event != 0 {
                    work.checkpoint(1)?;
                    let entry = evaluator.events.get(event - 1).ok_or_else(read_integrity)?;
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
                *consumed
                    .get_mut(usize::from(register - 10))
                    .ok_or_else(read_integrity)? = value;
            }
            Ok(ArgumentSummary {
                consumed,
                returned: evaluator.returned,
            })
        })();
        match result {
            Ok(summary) => Ok(Some(summary)),
            Err(error)
                if matches!(error.code, ErrorCode::ResourceLimited | ErrorCode::TimedOut) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }
}

impl ReadUsesEvaluator<'_, '_, '_> {
    pub(super) fn consume_arguments(
        &mut self,
        input: ReadState,
        site: u64,
        abi: CallAbi,
        calls: &dyn ArgumentSummaries,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        for register in abi.arguments() {
            let consumed = calls
                .summary(site)
                .and_then(|s| s.consumed.get(usize::from(register - 10)).copied())
                .unwrap_or(Consumed {
                    exact: 0,
                    may: u32::MAX,
                });
            let flow = input.register(register).flow;
            let sink = ReadSink::CallArgument {
                site,
                register,
                callee: None,
            };
            self.sink(flow, consumed.exact, sink.clone(), c)?;
            let len = self.flows.values(flow)?.len();
            for index in 0..len {
                c.checkpoint(1)?;
                let mut dependency = *self
                    .flows
                    .values(flow)?
                    .get(index)
                    .ok_or_else(read_integrity)?;
                for (exact, may) in dependency.exact.iter_mut().zip(&mut dependency.may) {
                    *may |= *exact;
                    *exact = 0;
                }
                self.use_dependency(dependency, consumed.may, sink.clone(), c)?;
            }
        }
        Ok(())
    }

    pub(super) fn return_value(
        &mut self,
        flow: usize,
        register: u8,
        c: &mut dyn RunControl,
    ) -> Result<()> {
        if !self.summary_mode {
            return self.sink(flow, u32::MAX, ReadSink::Return { register }, c);
        }
        let len = self.flows.values(flow)?.len();
        for index in 0..len {
            c.checkpoint(1)?;
            let dependency = *self
                .flows
                .values(flow)?
                .get(index)
                .ok_or_else(read_integrity)?;
            if let Some(existing) = self
                .returned
                .iter_mut()
                .find(|r| r.register == register && r.dependency.source == dependency.source)
            {
                existing.dependency = existing.dependency.join(dependency);
            } else {
                self.returned.push(
                    ReturnedArgument {
                        register,
                        dependency,
                    },
                    c.position(),
                )?;
            }
        }
        Ok(())
    }

    pub(super) fn call_result(
        &mut self,
        input: ReadState,
        summary: &ArgumentSummary<'_>,
        register: u8,
        c: &mut dyn RunControl,
    ) -> Result<usize> {
        let mut result = 0;
        for returned in summary.returned.iter().filter(|r| r.register == register) {
            let mapping = returned.dependency;
            let argument = 10 + mapping.source as u8;
            let flow = input.register(argument).flow;
            let len = self.flows.values(flow)?.len();
            for index in 0..len {
                let incoming = *self
                    .flows
                    .values(flow)?
                    .get(index)
                    .ok_or_else(read_integrity)?;
                let mut dependency = ReadDependency::empty(incoming.source);
                for (output, ((exact, may), (&map_exact, &map_may))) in dependency
                    .exact
                    .iter_mut()
                    .zip(&mut dependency.may)
                    .zip(mapping.exact.iter().zip(&mapping.may))
                    .enumerate()
                {
                    c.checkpoint(1)?;
                    let (e, m) = incoming.selected(map_exact);
                    let (a, b) = incoming.selected(map_may);
                    *may = m | a | b;
                    *exact = e & !*may;
                    if map_exact.count_ones() == 1 && map_may == 0 {
                        let bit = map_exact.trailing_zeros();
                        let inverted = (incoming.inverted >> bit) ^ (mapping.inverted >> output);
                        dependency.inverted |= (inverted & 1) << output;
                    }
                }
                if dependency.active() {
                    self.flows.scratch.push(dependency, c.position())?;
                }
            }
            let composed = self.flows.intern(c)?;
            result = self.binary_flow(result, composed, ReadFlowOp::Assemble, c)?;
        }
        Ok(result)
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
        let summary = returning(&memory)
            .summarize(&(), &memory, &mut || Ok(()))
            .expect("summary")
            .expect("complete");
        assert!(summary.consumed.iter().all(|c| c.exact == 0 && c.may == 0));
        assert_eq!(summary.returned.len(), 2);
        for returned in summary.returned.iter() {
            assert_eq!(
                returned.dependency.source,
                usize::from(returned.register - 10)
            );
            assert_eq!(returned.dependency.exact, std::array::from_fn(|i| 1 << i));
            assert_eq!(returned.dependency.may, [0; 32]);
        }
        assert!(memory.used() > 0);
        drop(summary);
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
                .summarize(&(), &memory, &mut || Ok(()))
                .expect("conservative summary")
                .is_none()
        );
        drop(blocker);
        assert_eq!(memory.used(), 0);
        let mut calls = 0;
        let result = returning(&memory)
            .summarize(&(), &memory, &mut || {
                calls += 1;
                if calls > 3 {
                    Err(Error::new(ErrorCode::ResourceLimited, "fixture work limit"))
                } else {
                    Ok(())
                }
            })
            .expect("bounded summary");
        assert!(result.is_none());
        assert_eq!(memory.used(), 0);
        let error = returning(&memory)
            .summarize(&(), &memory, &mut || {
                Err(Error::new(ErrorCode::Cancelled, "cancelled"))
            })
            .err()
            .expect("cancellation propagates");
        assert_eq!(error.code, ErrorCode::Cancelled);
        assert_eq!(memory.used(), 0);
    }
}
