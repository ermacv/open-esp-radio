//! Backend-independent TX ownership scenarios. The model runs these;
//! each backend supplies a fixture for this shared contract suite.
//! Harnesses submit the operations in `OPERATIONS`, retain the original
//! identities in opaque contexts, and report identities of actual returned
//! resources. No observation may be synthesized from the expected identity.
//! `check_barriers` uses the same captured publication/event trace for held
//! ACK, retry, timeout and key-removal cases. A Sent repeat differs only in
//! Retry, so the shared comparison covers the original SN, PN and payload.
//! Changed MPDU bytes report `MpduChanged`, separately from `Owners` for
//! a context, ticket or buffer identity mismatch.

/// GTK installation, replacement and selection RX boundaries.
pub mod group_keys;
/// Configuration permissions and opaque contexts through FIFO and disable.
pub mod interface_commands;

use alloc::vec::Vec;
use oer_ieee80211_lower_mac::{
    IndeterminateCause, SingleEnd, SingleOutcome, TxOutcome, WithdrawCause,
};
use oer_ieee80211_mac::qos::WmmAccessCategory;

/// Result supplied to a publication already held when its barrier arrives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BarrierCase {
    GateAck,
    GateRetry,
    GateTimeout,
    RemoveKey,
}

/// Counts sampled from the real backend while its publication result is held.
#[derive(Clone, Copy, Debug)]
pub struct HeldSnapshot {
    pub publications: usize,
    pub events: usize,
}

/// The actual state of a returned single MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MpduState {
    Fresh,
    Sent,
}

/// Actual terminal/barrier observations, in delivery order, of a backend
/// with the ACK evidence `A` and lost-air evidence `D`.
pub enum BarrierEvent<A, D> {
    Tx {
        ticket: u8,
        state: MpduState,
        outcome: SingleOutcome<A, D>,
    },
    GateClosed,
    KeyRemoved,
}

/// One published head and one queued successor on the same AC. Fixtures
/// capture counts and frame bytes from their actual publication observer;
/// terminals come through the port, never from an expected identity.
pub struct BarrierTrace<A, D> {
    pub tickets: [u8; 2],
    pub before_request: HeldSnapshot,
    pub before_result: HeldSnapshot,
    pub just_before_timeout: Option<HeldSnapshot>,
    pub publications_at_barrier: usize,
    pub events: Vec<BarrierEvent<A, D>>,
    /// Encoded MPDU after numbering, excluding hardware-generated trailers.
    /// Capture it from the actual publication's owner, before hardware owns it.
    pub first_frame: Vec<u8>,
    /// The same encoded extent for each real Sent republication.
    pub retried_frames: Vec<Vec<u8>>,
}

/// The publication/barrier suite shared by model and hardware fixtures.
pub trait BarrierHarness {
    /// The backend's ACK evidence.
    type Ack;
    /// The backend's lost-air evidence.
    type AirDenied;
    fn exercise_barrier(
        &mut self,
        case: BarrierCase,
    ) -> impl core::future::Future<Output = BarrierTrace<Self::Ack, Self::AirDenied>>;
}

pub async fn check_barriers(
    harness: &mut impl BarrierHarness,
) -> Result<(), (BarrierCase, Violation)> {
    for case in [
        BarrierCase::GateAck,
        BarrierCase::GateRetry,
        BarrierCase::GateTimeout,
        BarrierCase::RemoveKey,
    ] {
        verify_barrier(case, harness.exercise_barrier(case).await).map_err(|why| (case, why))?;
    }
    Ok(())
}

/// Verify real observations of a held publication, its barrier and a Sent retry.
pub fn verify_barrier<A, D>(case: BarrierCase, trace: BarrierTrace<A, D>) -> Result<(), Violation> {
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
    ] = trace.events.as_slice()
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
        BarrierCase::GateRetry if trace.retried_frames.is_empty() => {
            return Err(Violation::Publication);
        }
        BarrierCase::GateAck | BarrierCase::GateTimeout | BarrierCase::RemoveKey
            if !trace.retried_frames.is_empty() =>
        {
            return Err(Violation::Publication);
        }
        _ => {}
    }
    for retry in trace.retried_frames {
        let Some(([f0, f1], original)) = trace.first_frame.split_first_chunk::<2>() else {
            return Err(Violation::Publication);
        };
        let Some(([r0, r1], repeated)) = retry.split_first_chunk::<2>() else {
            return Err(Violation::Publication);
        };
        // Everything after Frame Control includes the original SN, CCMP PN
        // and payload. Attempt policy may change; MPDU contents may not.
        if f0 != r0
            || f1 & 0x08 != 0
            || r1 & 0x08 == 0
            || f1 & !0x08 != r1 & !0x08
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
    Small,
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

/// Alternating classes, same-AC order, and other ACs completing first.
pub const OPERATIONS: [Operation; 4] = [
    Operation {
        id: 0,
        class: Class::Small,
        category: WmmAccessCategory::BestEffort,
    },
    Operation {
        id: 1,
        class: Class::Large,
        category: WmmAccessCategory::BestEffort,
    },
    Operation {
        id: 2,
        class: Class::Small,
        category: WmmAccessCategory::Voice,
    },
    Operation {
        id: 3,
        class: Class::Large,
        category: WmmAccessCategory::Video,
    },
];

/// How the harness terminates its operations. BestEffort is held while
/// other ACs complete; then it is released, withdrawn, or ended by teardown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scenario {
    Complete,
    CloseGate,
    Disable,
    Refuse,
    Poison,
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
    Completed,
    Withdrawn,
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

/// Evidence after the install has ended.
pub struct Trace {
    /// Actual returns in observation order.
    pub returned: Vec<Returned>,
    /// Context destructors run. Quarantine retains unreturned contexts even
    /// after its receipt has been dropped.
    pub contexts_dropped: usize,
    /// Destructors observed after disable, before final result delivery.
    /// Poison must still own every unreturned context here.
    pub before_teardown_dropped: usize,
    /// Publications observed by the backend fixture.
    pub publications: usize,
}

/// Backend-specific execution, shared scenarios and checks.
pub trait Harness {
    /// Exercise the prescribed operations through the real backend API.
    fn exercise(&mut self, scenario: Scenario) -> impl core::future::Future<Output = Trace>;
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

/// Run the same ownership suite for any backend fixture. Refusal creates
/// no publication or terminal; fail-stop ends all owners with the install.
pub async fn check(harness: &mut impl Harness) -> Result<(), (Scenario, Violation)> {
    for scenario in [
        Scenario::Complete,
        Scenario::CloseGate,
        Scenario::Disable,
        Scenario::Refuse,
        Scenario::Poison,
    ] {
        verify(scenario, harness.exercise(scenario).await).map_err(|why| (scenario, why))?;
    }
    Ok(())
}

fn verify(scenario: Scenario, trace: Trace) -> Result<(), Violation> {
    let expected_drops = if scenario == Scenario::Poison {
        2
    } else {
        OPERATIONS.len()
    };
    if trace.contexts_dropped != expected_drops {
        return Err(Violation::Lifetime);
    }
    let returned_before_teardown = if scenario == Scenario::Poison { 2 } else { 4 };
    if trace.before_teardown_dropped != returned_before_teardown {
        return Err(Violation::Lifetime);
    }
    let expected_publications = match scenario {
        Scenario::Complete => 4,
        Scenario::Refuse => 0,
        _ => 2,
    };
    if trace.publications != expected_publications {
        return Err(Violation::Publication);
    }
    let mut remaining = OPERATIONS.map(Some);
    for returned in trace.returned {
        let Some(slot) = remaining
            .iter_mut()
            .find(|op| op.as_ref() == Some(&returned.operation))
        else {
            return Err(Violation::Duplicate);
        };
        *slot = None;
        if returned.expected != returned.actual || returned.actual.class != returned.operation.class
        {
            return Err(Violation::Owners);
        }
        if remaining.iter().flatten().any(|earlier| {
            earlier.category == returned.operation.category && earlier.id < returned.operation.id
        }) {
            return Err(Violation::Order);
        }
        let expected_end = match (scenario, returned.operation.category) {
            (Scenario::Refuse, _) => End::Refused,
            (Scenario::CloseGate | Scenario::Disable, WmmAccessCategory::BestEffort) => {
                End::Withdrawn
            }
            _ => End::Completed,
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
    Ok(())
}

#[cfg(test)]
mod tests;
