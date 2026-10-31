#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::panic,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::unreachable
)]
//! The shared conformance suite of the IEEE 802.11 lower-MAC port.
//!
//! Backend-independent TX ownership scenarios. The model runs these;
//! each backend supplies a fixture for this shared contract suite.
//! Harnesses submit `Classes::operations` of their `Harness::CLASSES` (one
//! buffer class carries every operation of a one-class backend), retain the original
//! identities in opaque contexts, and report identities of actual returned
//! resources. No observation may be synthesized from the expected identity.
//! A fixture holds BestEffort in a `Held` mode, unpublished or with its head
//! published and the result withheld; the trace's ordered publications,
//! request, returns and gate confirmation prove the mode's precondition.
//! `check_barriers` uses the same captured publication/event trace for held
//! ACK, retry, timeout and key-removal cases. A Sent repeat differs only in
//! Retry, so the shared comparison covers the original SN, PN and payload.
//! Changed MPDU bytes report `MpduChanged`, separately from `Owners` for
//! a context, ticket or buffer identity mismatch.

/// Allocation lifetime identities used by backend fixtures.
pub mod allocations;
pub mod catalog;
pub mod edca;
/// GTK installation, replacement and selection RX boundaries.
pub mod group_keys;
/// Configuration permissions and opaque contexts through FIFO and disable.
pub mod interface_commands;
pub mod msdu_age;

#[cfg(test)]
extern crate alloc;
#[cfg(test)]
extern crate std;

use oer_ieee80211_contracts_port::{
    IndeterminateCause, SingleEnd, SingleOutcome, TxOutcome, WithdrawCause,
};
use oer_ieee80211_wire::qos::WmmAccessCategory;

/// Where a fixture obtains the requested outcomes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Environment {
    /// A host model can inject every declared fault and ingress stage.
    Model,
    /// A physical backend uses real air and hardware observations.
    Hardware,
}

/// Why a case requires the host model rather than a physical air exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelOnlyReason {
    /// A collision cannot be produced deterministically on air.
    Collision,
    /// Air arbitration denial requires an injected result.
    AirDenied,
    /// A missing hardware completion requires an injected hardware failure.
    HardwareTimeout,
    /// Fail-stop/quarantine is deliberately injected into the backend.
    PoisonedBackend,
    /// DMA, interrupt and software ingress stages are injected independently.
    DelayedIngress,
    /// A group-key hardware change is stalled through its command deadline.
    CommandStall,
    /// A queue or its completion is held independently of real air.
    HeldContention,
    /// Explicit age boundaries and held publications require virtual time.
    MsduAge,
}

/// A case's physical execution requirements, independent of its fixture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Execution {
    /// A physical fixture can use real frames and cooperative backend progress.
    Hardware,
    /// The case requires an explicitly injected model condition.
    ModelOnly(ModelOnlyReason),
}

/// Explicit accounting of executed cases and model-only exclusions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Report {
    /// Cases whose observations passed their shared verifier.
    pub passed: u16,
    /// Cases explicitly excluded from a physical-backend run.
    pub model_only: u16,
}
impl Report {
    pub(crate) fn includes(&mut self, environment: Environment, execution: Execution) -> bool {
        if environment == Environment::Hardware && matches!(execution, Execution::ModelOnly(_)) {
            self.model_only = self.model_only.saturating_add(1);
            false
        } else {
            true
        }
    }
    pub(crate) fn passed(&mut self) {
        self.passed = self.passed.saturating_add(1);
    }
}

/// Single and aggregate attempt outcomes shared by backend fixtures.
pub mod outcomes;

/// Result supplied to a publication already held when its barrier arrives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BarrierCase {
    /// Close the gate while a held publication later receives its ACK.
    GateAck,
    /// Close the gate after a held publication, then repeat the returned Sent MPDU.
    GateRetry,
    /// Hold the publication result until its hardware deadline.
    GateTimeout,
    /// Remove a key while one of its publications remains held.
    RemoveKey,
}

impl BarrierCase {
    /// Every publication/barrier case, in shared-suite order.
    pub const ALL: [Self; 4] = [
        Self::GateAck,
        Self::GateRetry,
        Self::GateTimeout,
        Self::RemoveKey,
    ];
    /// A held result may be observed cooperatively; a lost hardware completion
    /// itself cannot be produced as a controlled air response.
    pub const fn execution(self) -> Execution {
        match self {
            Self::GateTimeout => Execution::ModelOnly(ModelOnlyReason::HardwareTimeout),
            _ => Execution::Hardware,
        }
    }
}

/// Counts sampled from the real backend while its publication result is held.
#[derive(Clone, Copy, Debug)]
pub struct HeldSnapshot {
    /// Publications observed at the sampled boundary.
    pub publications: usize,
    /// Events captured from the actual backend event stream.
    pub events: usize,
}

/// The actual state of a returned single MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MpduState {
    /// Unpublished memory returned to its caller.
    Fresh,
    /// Encoded MPDU returned with its original sequence and packet number.
    Sent,
}

/// Actual terminal/barrier observations, in delivery order, of a backend
/// with the ACK evidence `A` and lost-air evidence `D`.
pub enum BarrierEvent<A, D> {
    /// A transmit terminal observed through the port.
    Tx {
        /// Actual transmit ticket slot identity.
        ticket: u8,
        /// State of the MPDU returned by this terminal.
        state: MpduState,
        /// The outcome delivered by the real port.
        outcome: SingleOutcome<A, D>,
    },
    /// The gate-closing terminal delivered after pending transmissions.
    GateClosed,
    /// The key-removal terminal delivered after pending uses.
    KeyRemoved,
}

/// One published head and one queued successor on the same AC. Fixtures
/// capture counts and frame bytes from their actual publication observer;
/// terminals come through the port, never from an expected identity.
pub struct BarrierTrace<E, F, R, T> {
    /// Head and successor identities captured before admission.
    pub tickets: [u8; 2],
    /// Sample with the head published, before requesting its barrier.
    pub before_request: HeldSnapshot,
    /// Sample after requesting the barrier, before releasing the result.
    pub before_result: HeldSnapshot,
    /// Sample immediately before an intentionally held publication expires.
    pub just_before_timeout: Option<HeldSnapshot>,
    /// Publication count when the barrier terminal was delivered.
    pub publications_at_barrier: usize,
    /// Events captured from the actual backend event stream.
    pub events: E,
    /// Encoded MPDU after numbering, excluding hardware-generated trailers.
    /// Capture it from the actual publication's owner, before hardware owns it.
    pub first_frame: F,
    /// The same encoded extent for each real Sent republication.
    pub retried_frames: R,
    /// The actual terminal of the post-barrier Sent submission, if any.
    pub retry_terminal: Option<T>,
}

/// The publication/barrier suite shared by model and hardware fixtures.
pub trait BarrierHarness {
    /// The backend's ACK evidence.
    type Ack;
    /// The backend's lost-air evidence.
    type AirDenied;
    /// Captured event storage; may be an array or bounded collection.
    type Events: AsRef<[BarrierEvent<Self::Ack, Self::AirDenied>]>;
    /// Encoded MPDU storage owned by this fixture.
    type Frame: AsRef<[u8]>;
    /// Captured republications, without a prescribed allocator.
    type Retries: AsRef<[Self::Frame]>;
    /// Run the case through the actual backend and capture its owners and bytes.
    fn exercise_barrier(
        &mut self,
        case: BarrierCase,
    ) -> impl core::future::Future<
        Output = BarrierTrace<
            Self::Events,
            Self::Frame,
            Self::Retries,
            BarrierEvent<Self::Ack, Self::AirDenied>,
        >,
    >;
}

/// Run every prescribed publication/barrier case through the harness.
pub async fn check_barriers(
    harness: &mut impl BarrierHarness,
    environment: Environment,
) -> Result<Report, (BarrierCase, Violation)> {
    let mut report = Report::default();
    for case in BarrierCase::ALL {
        if !report.includes(environment, case.execution()) {
            continue;
        }
        verify_barrier(case, harness.exercise_barrier(case).await).map_err(|why| (case, why))?;
        report.passed();
    }
    Ok(report)
}

/// Verify real observations of a held publication, its barrier and a Sent retry.
pub fn verify_barrier<A, D, E: AsRef<[BarrierEvent<A, D>]>, F: AsRef<[u8]>, R: AsRef<[F]>>(
    case: BarrierCase,
    trace: BarrierTrace<E, F, R, BarrierEvent<A, D>>,
) -> Result<(), Violation> {
    for snapshot in [trace.before_request, trace.before_result]
        .into_iter()
        .chain(trace.just_before_timeout)
    {
        if snapshot.publications != 1 || snapshot.events != 0 {
            return Err(Violation::Publication);
        }
    }
    if (case == BarrierCase::GateTimeout) != trace.just_before_timeout.is_some()
        || trace.publications_at_barrier != 1
    {
        return Err(Violation::Publication);
    }
    let [head, tail] = trace.tickets;
    let [
        BarrierEvent::Tx {
            ticket: returned_head,
            state: MpduState::Sent,
            outcome,
        },
        BarrierEvent::Tx {
            ticket: returned_tail,
            state: MpduState::Fresh,
            outcome: tail_outcome,
        },
        barrier,
    ] = trace.events.as_ref()
    else {
        return Err(Violation::Order);
    };
    if head == tail || *returned_head != head || *returned_tail != tail {
        return Err(Violation::Owners);
    }
    let cause = match case {
        BarrierCase::RemoveKey => WithdrawCause::KeyRemoved,
        _ => WithdrawCause::GateClosed,
    };
    if !matches!(tail_outcome, TxOutcome::Withdrawn(withdrawn) if *withdrawn == cause)
        || !matches!(
            (case, barrier),
            (BarrierCase::RemoveKey, BarrierEvent::KeyRemoved)
                | (
                    BarrierCase::GateAck | BarrierCase::GateRetry | BarrierCase::GateTimeout,
                    BarrierEvent::GateClosed
                )
        )
    {
        return Err(Violation::TerminalKind);
    }
    let correct_head = match case {
        BarrierCase::GateAck => matches!(
            outcome,
            TxOutcome::Ended(SingleEnd::Delivered { air, .. })
                if air.transmissions == core::num::NonZeroU16::MIN
        ),
        BarrierCase::GateRetry | BarrierCase::RemoveKey => {
            matches!(outcome, TxOutcome::Withdrawn(withdrawn) if *withdrawn == cause)
        }
        BarrierCase::GateTimeout => matches!(
            outcome,
            TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout)
        ),
    };
    if !correct_head {
        return Err(Violation::TerminalKind);
    }
    match case {
        BarrierCase::GateRetry if trace.retried_frames.as_ref().is_empty() => {
            return Err(Violation::Publication);
        }
        BarrierCase::GateAck | BarrierCase::GateTimeout | BarrierCase::RemoveKey
            if !trace.retried_frames.as_ref().is_empty() =>
        {
            return Err(Violation::Publication);
        }
        _ => {}
    }
    for retry in trace.retried_frames.as_ref() {
        let Some((first_control, original)) = trace.first_frame.as_ref().split_first_chunk::<2>()
        else {
            return Err(Violation::Publication);
        };
        let Some((retry_control, repeated)) = retry.as_ref().split_first_chunk::<2>() else {
            return Err(Violation::Publication);
        };
        // Everything after Frame Control includes the original SN, CCMP PN
        // and payload. Attempt policy may change; MPDU contents may not.
        let first_control =
            oer_ieee80211_wire::frame::FrameControlFields::from_bytes(*first_control);
        let retry_control =
            oer_ieee80211_wire::frame::FrameControlFields::from_bytes(*retry_control);
        if first_control.retry()
            || !retry_control.retry()
            || first_control.to_bytes() != retry_control.with_retry(false).to_bytes()
            || original != repeated
        {
            return Err(Violation::MpduChanged);
        }
    }
    Ok(())
}

/// Resource classes deliberately sharing an AC queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Class {
    /// Small MPDU buffer class.
    Small,
    /// Large MPDU buffer class.
    Large,
}

/// One common scenario operation, in submission order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Operation {
    /// Context identity, independent of frame contents.
    pub id: u8,
    /// Buffer pool.
    pub class: Class,
    /// Hardware/software queue.
    pub category: WmmAccessCategory,
}

/// The transmit buffer classes a backend offers to the ownership suite.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Classes {
    /// Separate small and large pools.
    Two,
    /// One pool carries every operation.
    One,
}
impl Classes {
    /// Alternating classes, same-AC order, and other ACs completing first.
    /// With one class every operation takes it; order and AC are unchanged.
    pub const fn operations(self) -> [Operation; 4] {
        let large = match self {
            Self::Two => Class::Large,
            Self::One => Class::Small,
        };
        [
            Operation {
                id: 0,
                class: Class::Small,
                category: WmmAccessCategory::BestEffort,
            },
            Operation {
                id: 1,
                class: large,
                category: WmmAccessCategory::BestEffort,
            },
            Operation {
                id: 2,
                class: Class::Small,
                category: WmmAccessCategory::Voice,
            },
            Operation {
                id: 3,
                class: large,
                category: WmmAccessCategory::Video,
            },
        ]
    }
}

/// How the harness terminates its operations. BestEffort is held while
/// other ACs complete; then it is released, withdrawn, or ended by teardown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scenario {
    /// Release the held access category and complete every operation.
    Complete,
    /// Withdraw the held access category by closing the gate.
    CloseGate,
    /// Drain accepted operations through disable.
    Disable,
    /// Refuse invalid MPDU layouts before backend submission.
    Refuse,
    /// Fail-stop with accepted owners still retained.
    Poison,
}

impl Scenario {
    /// Every ownership case, in shared-suite order.
    pub const ALL: [Self; 5] = [
        Self::Complete,
        Self::CloseGate,
        Self::Disable,
        Self::Refuse,
        Self::Poison,
    ];
    /// Fail-stop is an injected model fault; other cases use real backend
    /// API admission, completion and lifecycle boundaries.
    pub const fn execution(self) -> Execution {
        match self {
            Self::Poison => Execution::ModelOnly(ModelOnlyReason::PoisonedBackend),
            _ => Execution::Hardware,
        }
    }
}

/// Stable identities, observed from the resources themselves.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Resources {
    /// Ticket slot.
    pub ticket: u8,
    /// Buffer slot (class remains part of identity).
    pub buffer: u8,
    /// Pool class.
    pub class: Class,
    /// External body allocation identity, including after Sent renewal.
    pub body: usize,
}

/// Distinguishes admission refusal from an accepted terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum End {
    /// The accepted operation returned through its terminal.
    Ended,
    /// The accepted operation returned through a lifecycle withdrawal.
    Withdrawn,
    /// The operation was refused before submission, without a terminal.
    Refused,
}

/// An observation of an actual return, including the original opaque context.
pub struct Returned {
    /// Original operation from the returned context.
    pub operation: Operation,
    /// Resource identity recorded before transfer, carried by that context.
    pub expected: Resources,
    /// Identity read from the returned owners.
    pub actual: Resources,
    /// Actual API path taken by the return.
    pub end: End,
}

/// Compact fixture evidence, sharing the former pointer-check byte. These
/// ownership fixtures admit at most two transmissions per operation.
#[derive(Clone, Copy)]
pub struct ReturnEvidence(u8);
impl ReturnEvidence {
    /// Refusal has no terminal or air count.
    pub const fn refused(pointer_matches: bool) -> Self {
        Self(pointer_matches as u8)
    }
    /// Preserve the actual outcome and its air count, without synthesizing a
    /// count for a terminal whose outcome carries no air evidence.
    pub fn terminal<A, D>(
        pointer_matches: bool,
        outcome: &oer_ieee80211_contracts_port::SingleOutcome<A, D>,
    ) -> Option<Self> {
        use oer_ieee80211_contracts_port::{SingleEnd, TxOutcome};
        let (kind, air) = match outcome {
            TxOutcome::Ended(SingleEnd::Delivered { air, .. }) => (1, Some(air)),
            TxOutcome::Ended(SingleEnd::AckTimeout(air)) => (2, Some(air)),
            TxOutcome::Ended(SingleEnd::CtsTimeout(air)) => (3, air.as_ref()),
            TxOutcome::Ended(SingleEnd::ContentionLimit(air)) => (4, air.as_ref()),
            TxOutcome::Ended(SingleEnd::AirDenied { air, .. }) => (5, air.as_ref()),
            TxOutcome::Withdrawn(_) => (6, None),
            _ => (7, None),
        };
        let count = u8::try_from(air.map_or(0, |air| air.transmissions.get()))
            .ok()
            .filter(|n| *n <= 15)?;
        Some(Self(u8::from(pointer_matches) | (kind << 1) | (count << 4)))
    }
    /// Raw pointer comparison remains independent of outcome and counting.
    pub const fn pointer_matches(self) -> bool {
        self.0 & 1 != 0
    }
    /// Zero means the public terminal carried no air count.
    pub const fn transmissions(self) -> Option<u16> {
        let count = self.0 >> 4;
        if count == 0 { None } else { Some(count as u16) }
    }
    /// Refused, delivered, ACK timeout, CTS timeout, contention limit,
    /// air denied, withdrawn, or another terminal, respectively.
    pub const fn outcome(self) -> u8 {
        (self.0 >> 1) & 7
    }
    /// Attach the independent pointer check after recovering the body owner.
    pub const fn with_pointer(self, matches: bool) -> Self {
        Self((self.0 & !1) | matches as u8)
    }
}

/// Publication, request, return and barrier order observed by a fixture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublicationEvent {
    /// A real publication of this operation ticket.
    Tx(u8),
    /// The fixture issued the scenario's request: Complete releases the
    /// held access category, CloseGate closes the gate, Disable begins
    /// disabling and Poison injects the fault.
    Requested,
    /// The accepted operation of this ticket returned through the port.
    Returned(u8),
    /// The gate-closing terminal was delivered.
    GateClosed,
}

/// The fixture's observation order: `markers`, each taken when the given
/// number of publications had been observed and recorded in that order, merged
/// into the publication tickets `published`. A marker precedes every
/// publication observed after it.
pub fn observed_order(
    published: impl IntoIterator<Item = u8>,
    markers: impl IntoIterator<Item = (usize, PublicationEvent)>,
) -> impl Iterator<Item = PublicationEvent> {
    let mut markers = markers.into_iter().peekable();
    let mut published = published.into_iter().enumerate().peekable();
    core::iter::from_fn(move || {
        let next = published.peek().map(|(index, _)| *index);
        if let Some((_, marker)) = markers.next_if(|(at, _)| next.is_none_or(|next| *at <= next)) {
            return Some(marker);
        }
        published
            .next()
            .map(|(_, ticket)| PublicationEvent::Tx(ticket))
    })
}

/// How a fixture holds BestEffort while Voice and Video complete. The
/// trace's events prove the mode's precondition at the request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Held {
    /// Neither BestEffort operation is published at the request.
    Unpublished,
    /// The BestEffort head is published with its result withheld and its
    /// tail is unpublished at the request. CloseGate and Disable return the
    /// head through its own terminal once the result is released and
    /// withdraw the tail; Poison keeps both quarantined.
    PublishedHead,
}

impl Held {
    /// Both fixture modes.
    pub const ALL: [Self; 2] = [Self::Unpublished, Self::PublishedHead];
}

/// Compare the contract's per-AC publication sequences. Ticket identities and
/// gate confirmation positions stay exact; independent ACs may interleave.
/// An unclassified ticket is invalid rather than silently omitted.
pub fn same_publications_by_ac<L, R>(
    left: L,
    right: R,
    category: impl Fn(u8) -> Option<WmmAccessCategory>,
) -> bool
where
    L: Iterator<Item = PublicationEvent> + Clone,
    R: Iterator<Item = PublicationEvent> + Clone,
{
    // Requests and returns are fixture-local positions; the comparison
    // covers what reached the air and the gate confirmation.
    let published = |event: &PublicationEvent| {
        matches!(
            event,
            PublicationEvent::Tx(_) | PublicationEvent::GateClosed
        )
    };
    let (left, right) = (left.filter(published), right.filter(published));
    if left
        .clone()
        .chain(right.clone())
        .any(|event| matches!(event, PublicationEvent::Tx(ticket) if category(ticket).is_none()))
    {
        return false;
    }
    let gate = |(index, event)| (event == PublicationEvent::GateClosed).then_some(index);
    if !left
        .clone()
        .enumerate()
        .filter_map(gate)
        .eq(right.clone().enumerate().filter_map(gate))
    {
        return false;
    }
    WmmAccessCategory::ALL.into_iter().all(|ac| {
        let belongs = |event: &PublicationEvent| match event {
            PublicationEvent::Tx(ticket) => category(*ticket) == Some(ac),
            _ => true,
        };
        left.clone()
            .filter(belongs)
            .eq(right.clone().filter(belongs))
    })
}

/// Evidence after the install has ended. Terminal evidence follows the
/// returned owners; publication events preserve their observed order.
pub struct Trace<R, E, P> {
    /// Actual returns in observation order.
    pub returned: R,
    /// Context destructors run. Quarantine retains unreturned contexts even
    /// after its receipt has been dropped.
    pub contexts_dropped: usize,
    /// Destructors observed after disable, before final result delivery.
    /// Poison must still own every unreturned context here.
    pub before_teardown_dropped: usize,
    /// Publications observed by the backend fixture.
    pub publications: usize,
    /// Actual terminal outcomes/counts, aligned with `returned`.
    pub terminal_evidence: E,
    /// Actual publications, the request, accepted returns and the gate
    /// confirmation, in observation order.
    pub publication_events: P,
    /// Ticket identities captured before submission, in operation order.
    pub tickets: [u8; 4],
}

/// Backend-specific execution, shared scenarios and checks.
pub trait Harness {
    /// The buffer classes the backend's operations use.
    const CLASSES: Classes;
    /// Storage for actual resource-return observations.
    type Returns: AsRef<[Returned]>;
    /// Actual terminal evidence storage.
    type Evidence: AsRef<[ReturnEvidence]>;
    /// Actual publication/barrier event storage.
    type Publications: AsRef<[PublicationEvent]>;
    /// Exercise the prescribed operations through the real backend API,
    /// holding BestEffort as `held` names.
    fn exercise(
        &mut self,
        scenario: Scenario,
        held: Held,
    ) -> impl core::future::Future<Output = Trace<Self::Returns, Self::Evidence, Self::Publications>>;
}

/// The common suite found a contract violation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Violation {
    /// A terminal names a different context, ticket, buffer or class.
    Owners,
    /// A Sent repeat changed its MPDU beyond setting the Retry bit.
    MpduChanged,
    /// An operation produced more than one return.
    Duplicate,
    /// Returns or barriers violated their prescribed order.
    Order,
    /// A required terminal or retained owner is absent.
    Missing,
    /// The terminal's outcome or shape differs from the operation's.
    TerminalKind,
    /// A context did not live until its terminal or install teardown.
    Lifetime,
    /// Publication counts or captured frame extents are invalid.
    Publication,
}

/// Run the same ownership suite for any backend fixture in the fixture's
/// `held` mode. Refusal creates no publication or terminal; fail-stop ends
/// all owners with the install.
pub async fn check<H: Harness>(
    harness: &mut H,
    environment: Environment,
    held: Held,
) -> Result<Report, (Scenario, Violation)> {
    let mut report = Report::default();
    for scenario in Scenario::ALL {
        if !report.includes(environment, scenario.execution()) {
            continue;
        }
        let trace = harness.exercise(scenario, held).await;
        verify(scenario, H::CLASSES, held, &trace).map_err(|why| (scenario, why))?;
        report.passed();
    }
    Ok(report)
}

/// Check captured ownership, publication, lifetime and FIFO observations
/// of a fixture in the `held` mode.
pub fn verify<R, E, P>(
    scenario: Scenario,
    classes: Classes,
    held: Held,
    trace: &Trace<R, E, P>,
) -> Result<(), Violation>
where
    R: AsRef<[Returned]>,
    E: AsRef<[ReturnEvidence]>,
    P: AsRef<[PublicationEvent]>,
{
    let operations = classes.operations();
    let expected_drops = if scenario == Scenario::Poison {
        2
    } else {
        operations.len()
    };
    if trace.contexts_dropped != expected_drops {
        return Err(Violation::Lifetime);
    }
    let returned_before_teardown = if scenario == Scenario::Poison { 2 } else { 4 };
    if trace.before_teardown_dropped != returned_before_teardown {
        return Err(Violation::Lifetime);
    }
    let head_published = held == Held::PublishedHead;
    let expected_publications = match scenario {
        Scenario::Complete => 4,
        Scenario::Refuse => 0,
        _ => 2 + usize::from(head_published),
    };
    // A closed gate's fixture may retry until exhaustion; its count is the
    // per-ticket publication events, each matching its return's air count.
    if scenario != Scenario::CloseGate && trace.publications != expected_publications {
        return Err(Violation::Publication);
    }
    let mut remaining = operations.map(Some);
    for returned in trace.returned.as_ref() {
        let Some(slot) = remaining
            .iter_mut()
            .find(|op| op.as_ref() == Some(&returned.operation))
        else {
            return Err(Violation::Duplicate);
        };
        *slot = None;
        if returned.expected != returned.actual
            || returned.actual.class != returned.operation.class
            || trace.tickets.get(usize::from(returned.operation.id))
                != Some(&returned.expected.ticket)
        {
            return Err(Violation::Owners);
        }
        if remaining.iter().flatten().any(|earlier| {
            earlier.category == returned.operation.category && earlier.id < returned.operation.id
        }) {
            return Err(Violation::Order);
        }
        let head = returned.operation.id == best_effort(operations)[0].id;
        let expected_end = match (scenario, returned.operation.category) {
            (Scenario::Refuse, _) => End::Refused,
            (Scenario::CloseGate | Scenario::Disable, WmmAccessCategory::BestEffort)
                if !(head && head_published) =>
            {
                End::Withdrawn
            }
            _ => End::Ended,
        };
        if returned.end != expected_end {
            return Err(Violation::TerminalKind);
        }
    }
    if scenario == Scenario::Poison {
        if remaining.iter().flatten().count() != 2
            || remaining
                .iter()
                .flatten()
                .any(|op| op.category != WmmAccessCategory::BestEffort)
        {
            return Err(Violation::Missing);
        }
    } else if remaining.iter().any(Option::is_some) {
        return Err(Violation::Missing);
    }
    verify_events(scenario, held, operations, trace)
}

/// The two BestEffort operations, head first: `Classes::operations`
/// submits them before Voice and Video.
const fn best_effort(operations: [Operation; 4]) -> [Operation; 2] {
    let [head, tail, _, _] = operations;
    [head, tail]
}

/// The observed order of publications, the request, returns and the gate
/// confirmation: the mode's precondition at the request, no BestEffort
/// publication after a withdrawing or poisoning request, FIFO returns and
/// a gate confirmed after both BestEffort returns with nothing published
/// after it. Each return's air count matches its ticket's publications.
fn verify_events<R, E, P>(
    scenario: Scenario,
    held: Held,
    operations: [Operation; 4],
    trace: &Trace<R, E, P>,
) -> Result<(), Violation>
where
    R: AsRef<[Returned]>,
    E: AsRef<[ReturnEvidence]>,
    P: AsRef<[PublicationEvent]>,
{
    use PublicationEvent::{GateClosed, Requested, Returned as Back, Tx};
    let rows = trace.returned.as_ref();
    let evidence = trace.terminal_evidence.as_ref();
    let events = trace.publication_events.as_ref();
    let tickets = trace.tickets;
    if rows.len() != evidence.len() {
        return Err(Violation::Missing);
    }
    let ticket = |operation: Operation| tickets[usize::from(operation.id)];
    let position = |wanted: PublicationEvent| events.iter().position(|event| *event == wanted);
    let count = |wanted: PublicationEvent, within: &[PublicationEvent]| {
        within.iter().filter(|event| **event == wanted).count()
    };
    for event in events {
        if let Tx(t) | Back(t) = event
            && !tickets.contains(t)
        {
            return Err(Violation::Owners);
        }
    }
    if events.iter().filter(|event| matches!(event, Tx(_))).count() != trace.publications {
        return Err(Violation::Publication);
    }
    // One return per accepted operation, and none for a refusal.
    for operation in operations {
        let returned = count(Back(ticket(operation)), events);
        let accepted = rows
            .iter()
            .any(|row| row.operation == operation && row.end != End::Refused);
        if returned > 1 {
            return Err(Violation::Duplicate);
        }
        if (returned == 1) != accepted {
            return Err(Violation::Missing);
        }
    }
    let requests = count(Requested, events);
    let gates = count(GateClosed, events);
    if requests != usize::from(scenario != Scenario::Refuse)
        || gates != usize::from(scenario == Scenario::CloseGate)
    {
        return Err(Violation::Order);
    }
    if let Some(at) = position(Requested) {
        let (before, after) = events.split_at(at);
        let [head, tail] = best_effort(operations);
        // Voice and Video complete while BestEffort is held.
        for other in operations
            .into_iter()
            .filter(|operation| operation.category != WmmAccessCategory::BestEffort)
        {
            if count(Back(ticket(other)), before) != 1 {
                return Err(Violation::Order);
            }
        }
        let head_before = count(Tx(ticket(head)), before);
        let precondition = match held {
            Held::Unpublished => head_before == 0,
            Held::PublishedHead => head_before == 1,
        };
        if !precondition || count(Tx(ticket(tail)), before) != 0 {
            return Err(Violation::Publication);
        }
        if count(Back(ticket(head)), before) + count(Back(ticket(tail)), before) != 0 {
            return Err(Violation::Order);
        }
        if scenario != Scenario::Complete
            && count(Tx(ticket(head)), after) + count(Tx(ticket(tail)), after) != 0
        {
            return Err(Violation::Publication);
        }
        if let (Some(first), Some(second)) =
            (position(Back(ticket(head))), position(Back(ticket(tail))))
            && first > second
        {
            return Err(Violation::Order);
        }
        if let Some(gate) = position(GateClosed) {
            let returned = |operation| position(Back(ticket(operation)));
            if events[gate..].iter().any(|event| matches!(event, Tx(_)))
                || returned(head).is_none_or(|at| at > gate)
                || returned(tail).is_none_or(|at| at > gate)
            {
                return Err(Violation::Order);
            }
        }
    }
    for (row, terminal) in rows.iter().zip(evidence) {
        let published = count(Tx(row.actual.ticket), events);
        match row.end {
            End::Ended if terminal.transmissions().map(usize::from) == Some(published) => {}
            End::Withdrawn | End::Refused
                if published == 0 && terminal.transmissions().is_none() => {}
            _ => return Err(Violation::Publication),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
