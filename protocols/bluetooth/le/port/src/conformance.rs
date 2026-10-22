//! Shared allocator-free backend scenarios and actual-owner trace checks.
//!
//! Fixtures drive real port calls and capture returned leases and contexts.
//! Expected identities must never be substituted for returned identities. The
//! runner works with any executor policy; neither it nor its traces allocate.
use super::*;
use oer_radio_port::{ContextLoan, ContextProjection, SlotIdentity};

/// Plain fixture context. Its value is recovered from the actual returned
/// PortContext, independently of the submitted correlation identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Tag {
    /// Fixture-selected owner identity.
    pub owner: u32,
}
impl<Id: SlotIdentity> ContextProjection<Id> for Tag {
    type View<'a, 'p>
        = u32
    where
        Self: 'p,
        Id: 'p,
        'p: 'a;
    type Parts<'p>
        = u32
    where
        Self: 'p,
        Id: 'p;
    fn view<'a, 'p>(&'a self, _: ContextLoan<'p, Id>) -> u32
    where
        Self: 'p,
        Id: 'p,
        'p: 'a,
    {
        self.owner
    }
    fn into_parts<'p>(self, _: ContextLoan<'p, Id>) -> u32
    where
        Self: 'p,
        Id: 'p,
    {
        self.owner
    }
}
/// Why deterministic control of an internal boundary is needed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelOnlyReason {
    /// Exact packet loss is a controlled propagation input.
    PropagationLoss,
    /// Queue admission/publication is held independently of P1.
    HeldPublication,
    /// Fault, withdrawal or epoch discontinuity is injected.
    InjectedFault,
}
/// Classification independent of whether this backend implements an operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Execution {
    /// A fixture uses real air and ordinary lifecycle progress.
    Hardware,
    /// Explicit exclusion from physical execution, with its reason.
    ModelOnly(ModelOnlyReason),
}
/// Cases in the minimum DTM contract. Foreign paths, contexts, body loans and
/// repeated release are checked by positive/negative UI twins separately.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Case {
    /// DTM TX to RX, then End with received count and a quiet interval.
    Dtm,
    /// Same operation on 2M; unsupported is distinct from ModelOnly.
    Dtm2M,
    /// Propagation drops every second test packet.
    Loss,
    /// Refusal returns the original command, capacity, session and context.
    Refusal,
    /// FIFO waiters, including cancellation before admission.
    Capacity,
    /// Drop an event wait after admission; P2 still executes and returns owners.
    DropAfterAdmission,
    /// Disable settles an unexecuted queued request.
    DisableQueued,
    /// Disable delivers an executed terminal still in its queue.
    DisableExecuted,
    /// Confirmed withdrawal returns the original session.
    Aborted,
    /// Test-air admission denial preserves the session with a lost-air fact.
    AirDenied,
    /// Unknown reachability retains owners and returns no reusable lease.
    Indeterminate,
    /// Ordinary fault settles all admitted terminals before Drained.
    DrainedFault,
    /// Rejecting a terminal sink retains the entire returned owner in quarantine.
    RejectSink,
    /// A discontinuity between admission and P2 publication returns NotExecuted.
    StaleEpoch,
    /// Ordinary uninstall followed by a fresh installation with empty queues.
    Reinstall,
}
impl Case {
    /// Stable suite order.
    pub const ALL: [Self; 15] = [
        Self::Dtm,
        Self::Dtm2M,
        Self::Loss,
        Self::Refusal,
        Self::Capacity,
        Self::DropAfterAdmission,
        Self::DisableQueued,
        Self::DisableExecuted,
        Self::Aborted,
        Self::AirDenied,
        Self::Indeterminate,
        Self::DrainedFault,
        Self::RejectSink,
        Self::StaleEpoch,
        Self::Reinstall,
    ];
    /// Physical execution class; an unsupported PHY retains Hardware status.
    pub const fn execution(self) -> Execution {
        match self {
            Self::Dtm
            | Self::Dtm2M
            | Self::Reinstall
            | Self::Refusal
            | Self::Capacity
            | Self::DropAfterAdmission
            | Self::DisableExecuted => Execution::Hardware,
            Self::Loss => Execution::ModelOnly(ModelOnlyReason::PropagationLoss),
            Self::DisableQueued => Execution::ModelOnly(ModelOnlyReason::HeldPublication),
            _ => Execution::ModelOnly(ModelOnlyReason::InjectedFault),
        }
    }
    /// Required operation profile; capability exclusions are never counted as passes.
    pub fn supported(self, caps: &LeRadioCapabilities) -> bool {
        caps.direct_test_mode
            && caps.test_phys.contains(if self == Self::Dtm2M {
                oer_bluetooth_le_radio::LePhy::Le2M
            } else {
                oer_bluetooth_le_radio::LePhy::Le1M
            })
    }
}
/// Resource identity observed before submission or from a returned owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Identity {
    /// Actual session slot index.
    pub slot: u8,
    /// Actual installation generation.
    pub epoch: u64,
    /// Actual caller context recovered through its public projection.
    pub context: u32,
}
impl Identity {
    /// Capture an original affine owner without consuming it.
    pub fn session<P: PoolPath, S>(session: &Session<'_, P, S>, context: u32) -> Self {
        Self {
            slot: session.index(),
            epoch: session.epoch(),
            context,
        }
    }
}
/// One observation read from a real terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Observation {
    /// Actual terminal correlation and installation identity.
    pub admitted: Admitted,
    /// Actual returned session identity; unknown execution has none.
    pub returned: Option<Identity>,
    /// Actual context projection, also available on an Indeterminate terminal.
    pub context: u32,
    /// Actual execution class.
    pub result: EventResult,
    /// Actual execution or lost-air reason.
    pub cause: Cause,
    /// Actual End packet count.
    pub count: Option<PacketCount>,
}
impl Observation {
    /// Read the terminal's owners without replacing them by expected IDs.
    pub fn capture<'p, P: PoolPath>(terminal: &Terminal<'p, P, Tag>) -> Self
    where
        P::Identity: 'p,
    {
        let context = terminal.context.view();
        let Completion::DirectTest { session, count } = &terminal.completion;
        let returned = match session {
            ReturnedSession::Idle(s) => Some(Identity::session(s, context)),
            ReturnedSession::Running(s) => Some(Identity::session(s, context)),
            ReturnedSession::Quarantined => None,
        };
        Self {
            admitted: terminal.admitted,
            returned,
            context,
            result: terminal.result,
            cause: terminal.cause,
            count: *count,
        }
    }
}
/// Fixed-size fixture evidence; all values come from actual port calls or
/// inspected retained owners. Unused observations remain None.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Trace {
    /// Owners recorded before submission.
    pub original: Option<Identity>,
    /// Actual initial owner of the next joined installation, when present.
    pub reinstalled: Option<Identity>,
    /// Actual receipts recorded at synchronous admission.
    pub submitted: [Option<Admitted>; 4],
    /// Returned terminal observations in stream order.
    pub events: [Option<Observation>; 4],
    /// Original owners recovered from a refusal or quarantine (not expected IDs).
    pub retained: Option<Identity>,
    /// FIFO service order, read from completed waiter futures.
    pub fifo: [Option<u8>; 3],
    /// Actual number of terminal events accepted by a disable sink.
    pub disabled: u8,
    /// Actual final stream status after all observations, when it ended.
    pub end: Option<bool>,
    /// Actual ordinary lifecycle joins observed by the fixture.
    pub joins: u8,
    /// Count observed after a quiet interval with TX stopped.
    pub quiet: Option<PacketCount>,
}
/// A trace violated the shared behavioral/ownership contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Violation {
    /// A resource or its caller context changed identity.
    Owners,
    /// Terminal execution class, reason or count was wrong or missing.
    Terminal,
    /// A capacity waiter passed an earlier waiter.
    Fifo,
    /// Ordinary join or drained/quarantined stream accounting was wrong.
    Lifecycle,
}
/// Validate the fixture's returned owners, execution class and lifecycle evidence.
pub fn verify(case: Case, trace: &Trace) -> Result<(), Violation> {
    let original = trace.original.ok_or(Violation::Owners)?;
    for (index, event) in trace
        .events
        .iter()
        .enumerate()
        .filter_map(|(i, e)| e.as_ref().map(|e| (i, e)))
    {
        let original = if case == Case::Reinstall && index == 1 {
            trace.reinstalled.ok_or(Violation::Owners)?
        } else {
            original
        };
        if trace.submitted.get(index).copied().flatten() != Some(event.admitted) {
            return Err(Violation::Terminal);
        }
        if !matches!(event.result, EventResult::Indeterminate) && event.returned != Some(original) {
            return Err(Violation::Owners);
        }
        if !matches!(event.result, EventResult::Executed { .. }) && event.count.is_some() {
            return Err(Violation::Terminal);
        }
        if event.context != original.context
            || event.admitted.epoch != original.epoch
            || event.returned.is_some_and(|returned| returned != original)
        {
            return Err(Violation::Owners);
        }
    }
    if trace.retained.is_some_and(|retained| retained != original) {
        return Err(Violation::Owners);
    }
    let first = || trace.events[0].ok_or(Violation::Terminal);
    match case {
        Case::Dtm | Case::Dtm2M | Case::Loss => {
            let first = first()?;
            let end = trace.events[1].ok_or(Violation::Terminal)?;
            if !matches!(first.result, EventResult::Executed { .. })
                || !matches!(end.result, EventResult::Executed { .. })
                || end.count
                    != Some(PacketCount {
                        packets: if case == Case::Loss { 5 } else { 10 },
                        saturated: false,
                    })
                || trace.quiet != Some(PacketCount::default())
            {
                return Err(Violation::Terminal);
            }
        }
        Case::Refusal => {
            if trace.retained != Some(original) {
                return Err(Violation::Owners);
            }
        }
        Case::Capacity => {
            if trace.fifo != [Some(1), Some(2), Some(0)] {
                return Err(Violation::Fifo);
            }
        }
        Case::DropAfterAdmission | Case::DisableExecuted => {
            if !matches!(first()?.result, EventResult::Executed { .. }) {
                return Err(Violation::Terminal);
            }
        }
        Case::DisableQueued => {
            let e = first()?;
            if e.result != EventResult::NotExecuted || e.cause != Cause::Disabled {
                return Err(Violation::Terminal);
            }
        }
        Case::Aborted => {
            let e = first()?;
            if e.result != EventResult::Aborted
                || e.cause != Cause::Withdrawn
                || e.returned != Some(original)
            {
                return Err(Violation::Terminal);
            }
        }
        Case::AirDenied => {
            let event = first()?;
            if event.result != EventResult::NotExecuted || event.cause != Cause::AirDenied {
                return Err(Violation::Terminal);
            }
        }
        Case::Indeterminate => {
            let e = first()?;
            if e.result != EventResult::Indeterminate
                || e.returned.is_some()
                || trace.retained != Some(original)
            {
                return Err(Violation::Owners);
            }
            if trace.end != Some(false) || trace.joins != 0 {
                return Err(Violation::Lifecycle);
            }
        }
        Case::DrainedFault => {
            if first()?.result != EventResult::NotExecuted || trace.end != Some(true) {
                return Err(Violation::Terminal);
            }
        }
        Case::RejectSink => {
            if trace.retained != Some(original) || trace.end != Some(false) || trace.joins != 0 {
                return Err(Violation::Lifecycle);
            }
        }
        Case::StaleEpoch => {
            let e = first()?;
            if e.result != EventResult::NotExecuted || e.cause != Cause::StaleEpoch {
                return Err(Violation::Terminal);
            }
        }
        Case::Reinstall => {
            let next = trace.reinstalled.ok_or(Violation::Owners)?;
            if trace.joins != 2
                || next.epoch != original.epoch.checked_add(1).ok_or(Violation::Owners)?
                || next.slot != original.slot
                || next.context != original.context
            {
                return Err(Violation::Lifecycle);
            }
            if !matches!(first()?.result, EventResult::Executed { .. })
                || !matches!(
                    trace.events[1].ok_or(Violation::Terminal)?.result,
                    EventResult::Executed { .. }
                )
            {
                return Err(Violation::Terminal);
            }
        }
    }
    if matches!(case, Case::DisableQueued | Case::DisableExecuted) && trace.disabled != 1 {
        return Err(Violation::Lifecycle);
    }
    Ok(())
}
/// Backend fixture entrypoint. Its returned trace must be captured from real
/// operations; unsupported and ModelOnly classifications are made by the runner.
pub trait Fixture {
    /// Backend capabilities used to select physically supported cases.
    fn capabilities(&self) -> LeRadioCapabilities;
    /// Execute one shared scenario with real queue, air and lifecycle progress.
    fn run(&mut self, case: Case) -> impl Future<Output = Result<Trace, Violation>>;
}
/// Explicit physical-versus-model fixture selection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Target {
    /// Execute injected cases as well as ordinary air cases.
    Model,
    /// Exclude injected cases with their declared reasons.
    Hardware,
}
/// Counts distinguish passes, model-only exclusions and unsupported profiles.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Report {
    /// Cases validated against actual traces.
    pub passed: u8,
    /// Cases excluded because their execution requires an injected boundary.
    pub model_only: u8,
    /// Cases requiring an unimplemented capability.
    pub unsupported: u8,
}
/// Run every supported case without an allocator, executor or runtime dependency.
pub async fn run(fixture: &mut impl Fixture, target: Target) -> Result<Report, (Case, Violation)> {
    let mut report = Report::default();
    for case in Case::ALL {
        if !case.supported(&fixture.capabilities()) {
            report.unsupported = report.unsupported.saturating_add(1);
            continue;
        }
        if target == Target::Hardware && matches!(case.execution(), Execution::ModelOnly(_)) {
            report.model_only = report.model_only.saturating_add(1);
            continue;
        }
        let trace = fixture.run(case).await.map_err(|v| (case, v))?;
        verify(case, &trace).map_err(|v| (case, v))?;
        report.passed = report.passed.saturating_add(1);
    }
    Ok(report)
}

/// Exercise synchronous admission and the single owned event stream. The caller
/// supplies the affine command and its original context. A cancelled wait after
/// admission leaves all command ownership on P2; subsequent waits retrieve it.
pub async fn transact<'p, P: PoolPath, X, L: LePhase<'p, P, X>>(
    phase: &mut L,
    seat: &mut Seat<'p, CommandCapacity, Child<P, 0>>,
    id: LeOperationId,
    command: DtmCommand<'p, P, L::Caps>,
    context: PortContext<'p, P::Identity, X>,
) -> Result<(Admitted, Terminal<'p, P, X>), Violation> {
    let capacity = phase.take(seat).await;
    let admitted = phase
        .submit(Request {
            id,
            capacity,
            command: Command::DirectTest(command),
            context,
        })
        .map_err(|_| Violation::Terminal)?
        .map_err(|_| Violation::Terminal)?;
    let Event::Terminal(terminal) = phase
        .next_event()
        .await
        .map_err(|_| Violation::Terminal)?
        .map_err(|_| Violation::Terminal)?;
    if admitted != terminal.admitted {
        return Err(Violation::Terminal);
    }
    Ok((admitted, terminal))
}
