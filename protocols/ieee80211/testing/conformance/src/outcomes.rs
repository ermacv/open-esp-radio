//! One-publication single-MPDU outcomes and actual returned-owner identity.
//! ACK uses a reachable peer; NoResponse uses an absent peer. Collisions,
//! arbitration denial and lost hardware completions require model injection.

use super::{Environment, Execution, ModelOnlyReason, MpduState, Report};
use core::num::NonZeroU16;
use oer_protocols_ieee80211_contracts_port::{
    IndeterminateCause, PhyRate, SingleEnd, SingleOutcome, SingleResponse, TxOutcome,
};

/// Requested observation for one publication of an individual MPDU,
/// without a retry plan extension.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Case {
    /// The MPDU receives an ACK from a real peer.
    Ack,
    /// The absent receiver sends no response.
    NoResponse,
    /// The model injects a collision/contention-limit completion.
    Collision,
    /// The model injects arbitration denial before air transmission.
    AirDenied,
    /// The model injects a lost hardware completion.
    HardwareTimeout,
}
impl Case {
    /// Every supported outcome.
    pub const ALL: [Self; 5] = [
        Self::Ack,
        Self::NoResponse,
        Self::Collision,
        Self::AirDenied,
        Self::HardwareTimeout,
    ];
    /// Whether real air can supply this completion deterministically.
    pub const fn execution(self) -> Execution {
        match self {
            Self::Collision => Execution::ModelOnly(ModelOnlyReason::Collision),
            Self::AirDenied => Execution::ModelOnly(ModelOnlyReason::AirDenied),
            Self::HardwareTimeout => Execution::ModelOnly(ModelOnlyReason::HardwareTimeout),
            Self::Ack | Self::NoResponse => Execution::Hardware,
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
/// Actual observations of a single-publication case.
pub struct Trace<A, D> {
    /// Resource identities captured before submission.
    pub admitted: Identity,
    /// Identities read from the returned owners, before renewal.
    pub returned: Identity,
    /// Returned buffer state, read from the actual terminal payload.
    pub state: MpduState,
    /// Rate actually selected for the admitted publication.
    pub rate: PhyRate,
    /// Publications captured by the fixture, excluding denied air access.
    pub publications: usize,
    /// Actual terminal delivered by the port, with the backend's own
    /// evidence types.
    pub outcome: SingleOutcome<A, D>,
}
/// Backend-specific admission, real air/fault stimulus, and owner observation.
pub trait Harness {
    /// Evidence carried by an ACK terminal.
    type Ack;
    /// Evidence carried by an air-denied terminal.
    type AirDenied;
    /// Submit one admitted operation, observe its real terminal and returned owners.
    fn exercise(
        &mut self,
        case: Case,
    ) -> impl core::future::Future<Output = Trace<Self::Ack, Self::AirDenied>>;
}
/// A single-publication contract violation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Violation {
    /// The terminal returned different ticket/buffer owners.
    Owners,
    /// The returned payload was not the encoded Sent MPDU.
    State,
    /// The outcome differs from the requested observation.
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
pub fn verify<A, D>(case: Case, trace: Trace<A, D>) -> Result<(), Violation> {
    if trace.admitted != trace.returned {
        return Err(Violation::Owners);
    }
    if trace.state != MpduState::Sent {
        return Err(Violation::State);
    }
    if trace.publications != usize::from(!matches!(case, Case::AirDenied)) {
        return Err(Violation::Publication);
    }
    let correct_air = |air: oer_protocols_ieee80211_contracts_port::Air| {
        air.transmissions == NonZeroU16::MIN && air.rate == trace.rate
    };
    let correct = match (case, trace.outcome) {
        (
            Case::Ack,
            TxOutcome::Ended(SingleEnd::Delivered {
                air,
                response: SingleResponse::Ack(_),
            }),
        )
        | (Case::NoResponse, TxOutcome::Ended(SingleEnd::AckTimeout(air))) => correct_air(air),
        (Case::Collision, TxOutcome::Ended(SingleEnd::ContentionLimit(None)))
        | (Case::AirDenied, TxOutcome::Ended(SingleEnd::AirDenied { air: None, .. }))
        | (Case::HardwareTimeout, TxOutcome::Indeterminate(IndeterminateCause::HardwareTimeout)) => {
            true
        }
        _ => false,
    };
    if correct {
        Ok(())
    } else {
        Err(Violation::Outcome)
    }
}
