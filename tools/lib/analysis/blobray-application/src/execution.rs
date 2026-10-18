//! Concrete execution of resolved requests over loaded executables.
use crate::execution_memory::Session;
use crate::*;
use std::collections::BTreeMap;
pub const EXECUTION_ENVIRONMENT: &str = "static-elf/boot-data-1/entry-registers-1/byte-addressed-memory-1/phased-regions-1/physical-goals-1/stack-words-1/single-hart-atomics-1/devices-4/external-calls-2/final-memory-1/physical-calls-1/internal-timeline-1/reviewed-projections-1/reviewed-effects-1";
pub(crate) struct LoadedSegment<'a> {
    pub address: u32,
    pub memory_size: usize,
    pub flags: u32,
    pub bytes: ScratchBytes<'a>,
}
/// Executable segments of every executable of a request, validated and loaded
/// once. Each fresh session copies them, so cold resets never reload them.
pub(crate) struct Sources<'a> {
    loaded: BTreeMap<ArtifactId, Vec<LoadedSegment<'a>>>,
}
impl<'a> Sources<'a> {
    /// Segments of the executables `targets` map, found in `executables` by
    /// content.
    pub(crate) fn from_executables(
        targets: &[&ExecutionTarget],
        executables: &[crate::in_process::Executable],
        memory: &'a WorkingMemory,
        c: &mut dyn RunControl,
    ) -> Result<Self> {
        let mut loaded = BTreeMap::new();
        for id in targets.iter().flat_map(|t| &t.executables) {
            if loaded.contains_key(id) {
                continue;
            }
            let executable = crate::in_process::find(executables, id)?;
            loaded.insert(id.clone(), segments(&executable.bytes(), memory, c)?);
        }
        Ok(Self { loaded })
    }
    /// Every loaded segment of `target`'s executables, in load order.
    pub(crate) fn target_segments(
        &self,
        target: &ExecutionTarget,
    ) -> impl Iterator<Item = &LoadedSegment<'a>> {
        target
            .executables
            .iter()
            .filter_map(|id| self.loaded.get(id))
            .flatten()
    }
}
fn segments<'a>(
    source: &dyn ByteSource,
    memory: &'a WorkingMemory,
    c: &mut dyn RunControl,
) -> Result<Vec<LoadedSegment<'a>>> {
    let mut result = Vec::new();
    oer_riscv_program::execution_segments(source, memory, c, &mut |segment, bytes, c| {
        let mut copy = memory.bytes(bytes.len(), c.position())?;
        for (chunk, source) in copy.chunks_mut(WORK_BLOCK).zip(bytes.chunks(WORK_BLOCK)) {
            c.checkpoint(1)?;
            chunk.copy_from_slice(source);
        }
        result
            .try_reserve(1)
            .map_err(|_| Error::new(ErrorCode::ResourceLimited, "segment allocation refused"))?;
        result.push(LoadedSegment {
            address: segment.address as u32,
            memory_size: segment.memory_size as usize,
            flags: segment.flags,
            bytes: copy,
        });
        Ok(())
    })?;
    Ok(result)
}
fn session<'a>(
    target: &ExecutionTarget,
    sources: &Sources<'_>,
    max_events: u32,
    memory: &'a WorkingMemory,
    coverage: crate::execution_coverage::CodeCoverage<'a>,
    c: &mut dyn RunControl,
) -> Result<Session<'a>> {
    let mut session = Session::new(memory, max_events, coverage, c)?;
    for id in &target.executables {
        for segment in sources
            .loaded
            .get(id)
            .ok_or_else(|| Error::new(ErrorCode::Integrity, "execution source was not prepared"))?
        {
            session.segment(
                segment.address,
                segment.memory_size,
                segment.flags,
                &segment.bytes,
                c,
            )?;
        }
    }
    Ok(session)
}
/// Receives each evidence record in stream order.
pub(crate) type Emit<'e> = dyn FnMut(ExecutionEvidence, &mut dyn RunControl) -> Result<()> + 'e;

fn evidence(
    emit: &mut Emit<'_>,
    case: u32,
    replacement: bool,
    observation: &ExecutionObservation,
    c: &mut dyn RunControl,
) -> Result<()> {
    for event in &observation.events {
        c.checkpoint(1)?;
        emit(
            ExecutionEvidence::Event {
                case,
                replacement,
                event: event.clone(),
            },
            c,
        )?;
    }
    for chunk in &observation.final_memory {
        c.checkpoint(1)?;
        emit(
            ExecutionEvidence::FinalMemory {
                case,
                replacement,
                chunk: *chunk,
            },
            c,
        )?;
    }
    for range in &observation.written {
        c.checkpoint(1)?;
        emit(
            ExecutionEvidence::Written {
                case,
                replacement,
                range: *range,
            },
            c,
        )?;
    }
    for model in &observation.models {
        c.checkpoint(1)?;
        emit(
            ExecutionEvidence::Model {
                case,
                replacement,
                observation: model.clone(),
            },
            c,
        )?;
    }
    for call in &observation.calls {
        c.checkpoint(1)?;
        emit(
            ExecutionEvidence::CallModel {
                case,
                replacement,
                observation: call.clone(),
            },
            c,
        )?;
    }
    emit(
        ExecutionEvidence::Outcome {
            case,
            replacement,
            stop: observation.stop.clone(),
            steps: observation.steps,
        },
        c,
    )
}
/// Resolved inputs of one execution request.
pub(crate) struct Resolved<'r, 'm> {
    pub request: &'r ExecutionRequest,
    pub goals: &'r [[Option<ResolvedExecutionGoal>; 2]],
    pub projections: &'r [ResolvedProjection],
    pub effects: &'r [ResolvedEffectContract],
    pub sources: &'r Sources<'m>,
    /// Byte patches applied to every replacement session's loaded image.
    pub patches: &'r [crate::in_process::ImagePatch],
}

/// Receives the step log of each finished replacement session.
pub(crate) type StepSink<'s, 'm> =
    dyn FnMut(crate::execution_steps::StepLog<'m>, &mut dyn RunControl) -> Result<()> + 's;

/// How the vendor side of a resolved request is obtained.
pub(crate) enum VendorSide<'v> {
    /// Execute it; with a sink, keep each case's observation.
    Execute(Option<&'v mut Vec<ExecutionObservation>>),
    /// Reuse the observations and coverage of an earlier execution of the
    /// same vendor side.
    Reuse {
        cases: &'v [ExecutionObservation],
        coverage: &'v [ExecutionCoverage],
    },
}

/// Aggregate outcome of a resolved request.
pub(crate) struct RunOutcome {
    pub verdict: Option<ComparisonVerdict>,
    pub complete: bool,
    /// False when an incomplete replacement case blocked the vendor side of
    /// the next warm case, so the vendor observations depend on the
    /// replacement. Reuse then stops at that case, before emitting it.
    pub vendor_independent: bool,
}

/// Execute every case of a resolved request, handing each evidence record to
/// `emit` in stream order.
pub(crate) fn run_resolved<'m>(
    resolved: &Resolved<'_, 'm>,
    vendor: &mut VendorSide<'_>,
    mut steps: Option<&mut StepSink<'_, 'm>>,
    executor: &dyn Executor,
    memory: &'m WorkingMemory,
    emit: &mut Emit<'_>,
    control: &mut dyn RunControl,
) -> Result<RunOutcome> {
    let request = resolved.request;
    let mut sides: [Side<'m>; 2] = Default::default();
    sides[1].record = steps.is_some();
    sides[1].patched = true;
    let mut blocked = false;
    // Whether `blocked` holds only because the replacement did not complete.
    let mut blocked_by_replacement = false;
    let mut vendor_independent = true;
    let mut complete = true;
    let mut verdict = request
        .replacement
        .as_ref()
        .map(|_| ComparisonVerdict::Match);
    for (index, case) in request.cases.iter().enumerate() {
        if case.reset == SessionReset::Cold {
            blocked = false;
            blocked_by_replacement = false;
            // Release both previous address spaces before acquiring either new one.
            for side in &mut sides {
                side.release();
            }
            if let Some(sink) = steps.as_mut() {
                for log in sides[1].steps.drain(..) {
                    sink(log, control)?;
                }
            }
        }
        let mut position = control.position();
        position.table = Some(index as u64);
        control.set_position(position);
        control.checkpoint(1)?;
        let engine = Engine {
            sources: resolved.sources,
            request,
            memory,
            executor,
            reset: case.reset,
            stack_fill: case.stack_fill,
            patches: resolved.patches,
            close_chain: request
                .cases
                .get(index + 1)
                .is_none_or(|next| next.reset == SessionReset::Cold),
        };
        if blocked_by_replacement {
            vendor_independent = false;
            if matches!(vendor, VendorSide::Reuse { .. }) {
                break;
            }
        }
        let left = match vendor {
            VendorSide::Reuse { cases, .. } => {
                std::borrow::Cow::Borrowed(cases.get(index).ok_or_else(|| {
                    Error::new(ErrorCode::Integrity, "reused vendor case is missing")
                })?)
            }
            VendorSide::Execute(_) => std::borrow::Cow::Owned(engine.invoke(
                &request.vendor,
                &case.vendor,
                resolved_goal(resolved.goals[index][0])?,
                &mut sides[0],
                blocked,
                control,
            )?),
        };
        let right = match (&request.replacement, &case.replacement) {
            (Some(t), Some(i)) => Some(engine.invoke(
                t,
                i,
                resolved_goal(resolved.goals[index][1])?,
                &mut sides[1],
                blocked,
                control,
            )?),
            _ => None,
        };
        evidence(emit, index as u32, false, &left, control)?;
        if let Some(right) = &right {
            evidence(emit, index as u32, true, right, control)?;
        }
        // A paired case without a relation is setup: both sides run and
        // nothing of it is compared or observed.
        if right.is_some()
            && case.relation.is_none()
            && let Some(steps) = sides[1].session.as_mut().and_then(|s| s.steps())
        {
            steps.end_case(Default::default(), control)?;
        }
        if let (Some(right), Some(relation)) = (&right, &case.relation) {
            control.phase(RunPhase::Compare)?;
            let comparison = blobray_verification::compare(
                &left,
                right,
                relation,
                selected_projection(case.relation.as_ref(), resolved.projections)?.map(
                    |resolved| blobray_verification::ProjectionComparison {
                        resolved,
                        vendor: &case.vendor,
                        replacement: case.replacement.as_ref().unwrap(),
                    },
                ),
                selected_effect_contract(case.relation.as_ref(), resolved.effects)?,
                control,
            )?;
            verdict = Some(match (verdict.unwrap(), comparison.verdict) {
                (ComparisonVerdict::Diff, _) | (_, ComparisonVerdict::Diff) => {
                    ComparisonVerdict::Diff
                }
                (ComparisonVerdict::Incomplete, _) | (_, ComparisonVerdict::Incomplete) => {
                    ComparisonVerdict::Incomplete
                }
                _ => ComparisonVerdict::Match,
            });
            if let Some(steps) = sides[1].session.as_mut().and_then(|s| s.steps()) {
                let compared = blobray_verification::compared_observations(
                    right,
                    case.replacement.as_ref().unwrap(),
                    relation,
                    selected_projection(Some(relation), resolved.projections)?
                        .map(|p| &p.projection),
                    selected_effect_contract(Some(relation), resolved.effects)?,
                    control,
                )?;
                steps.end_case(
                    crate::execution_steps::CaseSinks::of(&compared, right),
                    control,
                )?;
            }
            emit(
                ExecutionEvidence::Comparison {
                    case: index as u32,
                    result: comparison,
                },
                control,
            )?;
        }
        let phase_complete =
            left.completed() && right.as_ref().is_none_or(ExecutionObservation::completed);
        complete &= phase_complete;
        if !phase_complete && !blocked {
            blocked = true;
            blocked_by_replacement = left.completed();
        }
        if let VendorSide::Execute(Some(sink)) = vendor {
            sink.push(left.as_ref().clone());
        }
        if let (Some(session), std::borrow::Cow::Owned(left)) = (&mut sides[0].session, left) {
            session.recycle(left);
        }
        if let (Some(session), Some(right)) = (&mut sides[1].session, right) {
            session.recycle(right);
        }
    }
    if !vendor_independent && matches!(vendor, VendorSide::Reuse { .. }) {
        return Ok(RunOutcome {
            verdict,
            complete,
            vendor_independent,
        });
    }
    for (index, side) in sides.iter_mut().enumerate() {
        side.release();
        if index == 1 && request.replacement.is_none() {
            break;
        }
        let parts = match vendor {
            VendorSide::Reuse { coverage, .. } if index == 0 => coverage.to_vec(),
            _ => side.coverage.finish(memory, control)?.0.split(),
        };
        for part in parts {
            control.checkpoint(1)?;
            emit(
                ExecutionEvidence::Coverage {
                    replacement: index == 1,
                    coverage: part,
                },
                control,
            )?;
        }
    }
    if let Some(sink) = steps.as_mut() {
        for log in sides[1].steps.drain(..) {
            sink(log, control)?;
        }
    }
    Ok(RunOutcome {
        verdict,
        complete,
        vendor_independent,
    })
}

/// One side's live session and the coverage and step logs that outlive its
/// sessions.
#[derive(Default)]
struct Side<'a> {
    session: Option<Session<'a>>,
    coverage: crate::execution_coverage::CodeCoverage<'a>,
    steps: Vec<crate::execution_steps::StepLog<'a>>,
    /// Whether this side's sessions record step logs.
    record: bool,
    /// Whether this side's sessions apply the request's image patches.
    patched: bool,
}
impl Side<'_> {
    /// Drop the session, keeping what it reached and recorded.
    fn release(&mut self) {
        if let Some(mut session) = self.session.take() {
            self.steps.extend(session.take_steps());
            self.coverage = session.into_coverage();
        }
    }
}

struct Engine<'a, 'm> {
    sources: &'a Sources<'m>,
    request: &'a ExecutionRequest,
    memory: &'m WorkingMemory,
    executor: &'a dyn Executor,
    reset: SessionReset,
    stack_fill: Option<u8>,
    patches: &'a [crate::in_process::ImagePatch],
    close_chain: bool,
}
impl<'m> Engine<'_, 'm> {
    fn invoke(
        &self,
        target: &ExecutionTarget,
        invocation: &Invocation,
        goal: ResolvedExecutionGoal,
        side: &mut Side<'m>,
        blocked: bool,
        c: &mut dyn RunControl,
    ) -> Result<ExecutionObservation> {
        let case = c.position().table;
        if blocked {
            let machine = side.session.as_mut().ok_or_else(|| {
                Error::new(ErrorCode::Integrity, "blocked phase has no prior session")
            })?;
            return machine.observation(ExecutionStop::BlockedByPriorPhase, 0, self.close_chain, c);
        }
        if side.session.is_none() {
            if self.reset == SessionReset::Warm {
                return Err(Error::new(
                    ErrorCode::Integrity,
                    "warm phase has no prior session",
                ));
            }
            side.session = Some(session(
                target,
                self.sources,
                self.request.max_events,
                self.memory,
                std::mem::take(&mut side.coverage),
                c,
            )?);
            let created = side.session.as_mut().unwrap();
            if side.patched {
                for patch in self.patches {
                    created.patch(patch, c)?;
                }
            }
            if side.record {
                created.record_steps();
            }
        }
        let machine = side.session.as_mut().unwrap();
        let stack = machine.phase(target, self.stack_fill, invocation, c)?;
        c.set_position(RunPosition {
            phase: RunPhase::Execute,
            table: case,
            ..Default::default()
        });
        c.checkpoint(0)?;
        let (stop, steps) = self.executor.execute(
            &ExecutionStart {
                entry: invocation.entry,
                stack,
                arguments: invocation.register_arguments(),
                goal,
            },
            machine,
            c,
        )?;
        machine.capture_final_memory(invocation, c)?;
        machine.observation(stop, steps, self.close_chain, c)
    }
}

fn resolved_goal(goal: Option<ResolvedExecutionGoal>) -> Result<ResolvedExecutionGoal> {
    goal.ok_or_else(|| Error::new(ErrorCode::Integrity, "execution goal unresolved"))
}
