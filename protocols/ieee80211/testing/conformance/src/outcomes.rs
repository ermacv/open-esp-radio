//! One-publication single/A-MPDU outcomes and actual returned-owner identity.
//! ACK uses a reachable peer; NoResponse uses an absent peer. Collisions,
//! arbitration denial and lost hardware completions require model injection.

use super::{Environment, Execution, ModelOnlyReason, MpduState, Report};
use core::num::NonZeroU16;
use oer_ieee80211_contracts_port::{
    AggregateEnd, AggregateOutcome, IndeterminateCause, PhyRate, SingleEnd, SingleOutcome,
    SingleResponse, TxOutcome,
};
use oer_ieee80211_wire::sequence::SequenceNumber;

/// Form of the admitted operation and its terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Form {
    /// One MPDU requesting an immediate ACK.
    Single,
    /// One-member aggregate requesting a BlockAck.
    Aggregate,
}

/// Requested observation for one publication, without a retry plan extension.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Case {
    /// An individual MPDU receives an ACK from a real peer.
    Ack,
    /// The one-member aggregate receives a BlockAck for its MPDU.
    BlockAck,
    /// The absent receiver sends no response.
    NoResponse(Form),
    /// The model injects a collision/contention-limit completion.
    Collision(Form),
    /// The model injects arbitration denial before air transmission.
    AirDenied(Form),
    /// The model injects a lost hardware completion.
    HardwareTimeout(Form),
}
impl Case {
    /// Every supported form/outcome pair.
    pub const ALL: [Self; 10] = [
        Self::Ack,
        Self::BlockAck,
        Self::NoResponse(Form::Single),
        Self::NoResponse(Form::Aggregate),
        Self::Collision(Form::Single),
        Self::Collision(Form::Aggregate),
        Self::AirDenied(Form::Single),
        Self::AirDenied(Form::Aggregate),
        Self::HardwareTimeout(Form::Single),
        Self::HardwareTimeout(Form::Aggregate),
    ];
    /// The form the fixture must submit and receive back.
    pub const fn form(self) -> Form {
        match self {
            Self::Ack => Form::Single,
            Self::BlockAck => Form::Aggregate,
            Self::NoResponse(form)
            | Self::Collision(form)
            | Self::AirDenied(form)
            | Self::HardwareTimeout(form) => form,
        }
    }
    /// Whether real air can supply this completion deterministically.
    pub const fn execution(self) -> Execution {
        match self {
            Self::Collision(_) => Execution::ModelOnly(ModelOnlyReason::Collision),
            Self::AirDenied(_) => Execution::ModelOnly(ModelOnlyReason::AirDenied),
            Self::HardwareTimeout(_) => Execution::ModelOnly(ModelOnlyReason::HardwareTimeout),
            _ => Execution::Hardware,
        }
    }
}

/// Identities read from the fixture's ticket and actual buffer owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Identity {
    /// Transmit ticket slot.
    pub ticket: u8,
    /// Buffer slot, within the fixture's single small-buffer pool.
    pub buffer: u8,
}
/// Actual form-specific terminal, retaining the backend's own evidence types.
pub enum Outcome<A, B, D> {
    /// Terminal for the submitted individual MPDU.
    Single(SingleOutcome<A, D>),
    /// Terminal for the submitted one-member aggregate.
    Aggregate(AggregateOutcome<B, D>),
}
/// Actual observations of a single-publication case.
pub struct Trace<A, B, D> {
    /// Resource identities captured before submission.
    pub admitted: Identity,
    /// Identities read from the returned owners, before renewal.
    pub returned: Identity,
    /// Returned buffer state, read from the actual terminal payload.
    pub state: MpduState,
    /// Rate actually selected for the admitted publication.
    pub rate: PhyRate,
    /// Sequence number encoded in the admitted MPDU.
    pub sequence: SequenceNumber,
    /// Publications captured by the fixture, excluding denied air access.
    pub publications: usize,
    /// Actual terminal delivered by the port.
    pub outcome: Outcome<A, B, D>,
}
/// Backend-specific admission, real air/fault stimulus, and owner observation.
pub trait Harness {
    /// Evidence carried by an ACK terminal.
    type Ack;
    /// Evidence carried by a BlockAck terminal.
    type BlockAck;
    /// Evidence carried by an air-denied terminal.
    type AirDenied;
    /// Submit one admitted operation, observe its real terminal and returned owners.
    fn exercise(
        &mut self,
        case: Case,
    ) -> impl core::future::Future<Output = Trace<Self::Ack, Self::BlockAck, Self::AirDenied>>;
}
/// A single-publication contract violation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Violation {
    /// The terminal returned different ticket/buffer owners.
    Owners,
    /// The returned payload was not the encoded Sent MPDU.
    State,
    /// Terminal form or outcome differs from the requested observation.
    Outcome,
    /// Publication count differs from the admitted one-publication case.
    Publication,
}
/// Run selected outcome cases; hardware exclusions remain explicit in the report.
pub async fn check(
    harness: &mut impl Harness,
    environment: Environment,
) -> Result<Report, (Case, Violation)> {
    let mut report = Report::default();
    for case in Case::ALL {
        if !report.includes(environment, case.execution()) {
            continue;
        }
        verify(case, harness.exercise(case).await).map_err(|why| (case, why))?;
        report.passed();
    }
    Ok(report)
}
/// Check an actual terminal's shape, owner identity, rate and publication count.
pub fn verify<A, B, D>(case: Case, trace: Trace<A, B, D>) -> Result<(), Violation> {
    if trace.admitted != trace.returned {
        return Err(Violation::Owners);
    }
    if trace.state != MpduState::Sent {
        return Err(Violation::State);
    }
    if trace.publications != usize::from(!matches!(case, Case::AirDenied(_))) {
        return Err(Violation::Publication);
    }
    let correct_air = |air: oer_ieee80211_contracts_port::Air| {
        air.transmissions == NonZeroU16::MIN && air.rate == trace.rate
    };
    let correct = match (case, trace.outcome) {
        (
            Case::Ack,
            Outcome::Single(TxOutcome::Ended(SingleEnd::Delivered {
                air,
                response: SingleResponse::Ack(_),
            })),
        ) => correct_air(air),
        (
            Case::BlockAck,
            Outcome::Aggregate(TxOutcome::Ended(AggregateEnd::Delivered { air, report, .. })),
        ) => correct_air(air) && report.start_sequence == trace.sequence && report.bitmap == 1,
        (
            Case::NoResponse(Form::Single),
            Outcome::Single(TxOutcome::Ended(SingleEnd::AckTimeout(air))),
        ) => correct_air(air),
        (
            Case::NoResponse(Form::Aggregate),
            Outcome::Aggregate(TxOutcome::Ended(AggregateEnd::AckTimeout(air))),
        ) => correct_air(air),
        (
            Case::Collision(Form::Single),
            Outcome::Single(TxOutcome::Ended(SingleEnd::ContentionLimit(None))),
        ) => true,
        (
            Case::Collision(Form::Aggregate),
            Outcome::Aggregate(TxOutcome::Ended(AggregateEnd::ContentionLimit(None))),
        ) => true,
        (
            Case::AirDenied(Form::Single),
            Outcome::Single(TxOutcome::Ended(SingleEnd::AirDenied { air: None, .. })),
        ) => true,
        (
            Case::AirDenied(Form::Aggregate),
            Outcome::Aggregate(TxOutcome::Ended(AggregateEnd::AirDenied { air: None, .. })),
        ) => true,
        (
            Case::HardwareTimeout(Form::Single),
            Outcome::Single(TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout)),
        ) => true,
        (
            Case::HardwareTimeout(Form::Aggregate),
            Outcome::Aggregate(TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout)),
        ) => true,
        _ => false,
    };
    if correct {
        Ok(())
    } else {
        Err(Violation::Outcome)
    }
}
