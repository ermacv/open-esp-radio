use oer_ieee80211_port::{
    conformance::{self, MpduState, catalog, outcomes},
    *,
};
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};

/// Match an AP without putting its address or SSID in the HIL transcript.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Prepare {
    pub channel: u8,
    pub country: [u8; 2],
    pub peer_sha256: [u8; 32],
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct Prepared {
    /// The diagnostic composition's optional aggregate admission capability.
    pub ampdu: bool,
    /// Basic-rate elements captured from the matched AP's beacon.
    pub basic_rates: [u8; 16],
    pub basic_rates_len: u8,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct RunCase(pub u8);
impl RunCase {
    pub fn case(self) -> Option<catalog::Case> {
        catalog::Case::all().nth(usize::from(self.0))
    }
}
/// Observable terminal classification, without physical signal measurements.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum End {
    Ack,
    NoResponse,
    Collision,
    AirDenied,
    HardwareTimeout,
    Withdrawn,
    Expired,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct OutcomeTrace {
    pub admitted: [u8; 2],
    pub returned: [u8; 2],
    pub sent: bool,
    pub rate_kbps: u32,
    /// Last on-air rate reported by the terminal, independently of admission.
    pub observed_rate_kbps: Option<u32>,
    pub sequence: u16,
    pub publications: u16,
    pub transmissions: u16,
    pub end: End,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CaseTrace {
    pub case: RunCase,
    pub trace: Trace,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum Trace {
    Outcome(OutcomeTrace),
    Ownership(OwnershipTrace),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct OwnerResources {
    pub ticket: u8,
    pub buffer: u8,
    pub class: u8,
    /// Allocation ordinal, derived from the original owned bodies. None
    /// preserves an unknown/changed body instead of inventing its identity.
    pub body: Option<u8>,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct OwnerReturn {
    pub operation: u8,
    pub category: u8,
    pub class: u8,
    pub expected: OwnerResources,
    pub actual: OwnerResources,
    pub end: u8,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct OwnershipTrace {
    pub returned: [Option<OwnerReturn>; 8],
    pub contexts_dropped: u16,
    pub before_teardown_dropped: u16,
    pub publications: u16,
}
/// Pointer coordinates differ across machines. Preserve allocation identity
/// by looking it up in the original contexts, including a failed lookup.
pub fn ownership_trace<R: AsRef<[conformance::Returned]>>(
    trace: &conformance::Trace<R>,
) -> Option<OwnershipTrace> {
    use conformance::{Class, End, Resources};
    fn class(value: Class) -> u8 {
        match value {
            Class::Small => 0,
            Class::Large => 1,
        }
    }
    let rows = trace.returned.as_ref();
    let resource = |value: Resources| OwnerResources {
        ticket: value.ticket,
        buffer: value.buffer,
        class: class(value.class),
        body: rows
            .iter()
            .find(|row| row.expected.body == value.body)
            .map(|row| row.operation.id),
    };
    let mut returned = [None; 8];
    for (index, row) in rows.iter().enumerate() {
        *returned.get_mut(index)? = Some(OwnerReturn {
            operation: row.operation.id,
            category: row.operation.category as u8,
            class: class(row.operation.class),
            expected: resource(row.expected),
            actual: resource(row.actual),
            end: match row.end {
                End::Ended => 0,
                End::Withdrawn => 1,
                End::Refused => 2,
            },
        });
    }
    Some(OwnershipTrace {
        returned,
        contexts_dropped: trace.contexts_dropped.try_into().ok()?,
        before_teardown_dropped: trace.before_teardown_dropped.try_into().ok()?,
        publications: trace.publications.try_into().ok()?,
    })
}
/// Project actual resource and terminal observations onto the port contract.
/// Signal measurements and clock coordinates are physical evidence, not
/// model-comparable values. Every contract field of the outcome is retained.
pub fn outcome_trace<A, B, D>(trace: &outcomes::Trace<A, B, D>) -> Option<OutcomeTrace> {
    let outcomes::Outcome::Single(outcome) = &trace.outcome else {
        return None;
    };
    let (end, air) = match outcome {
        TxOutcome::Ended(SingleEnd::Delivered {
            air,
            response: SingleResponse::Ack(_),
        }) => (End::Ack, Some(*air)),
        TxOutcome::Ended(SingleEnd::Delivered { .. }) => return None,
        TxOutcome::Ended(SingleEnd::AckTimeout(air)) => (End::NoResponse, Some(*air)),
        TxOutcome::Ended(SingleEnd::ContentionLimit(air)) => (End::Collision, *air),
        TxOutcome::Ended(SingleEnd::AirDenied { air, .. }) => (End::AirDenied, *air),
        TxOutcome::Indeterminate(_) => (End::HardwareTimeout, None),
        TxOutcome::Withdrawn(_) => (End::Withdrawn, None),
        TxOutcome::Expired { .. } => (End::Expired, None),
        _ => return None,
    };
    let PhyRate::Legacy(rate) = trace.rate else {
        return None;
    };
    Some(OutcomeTrace {
        admitted: [trace.admitted.ticket, trace.admitted.buffer],
        returned: [trace.returned.ticket, trace.returned.buffer],
        sent: trace.state == MpduState::Sent,
        rate_kbps: rate.kbps(),
        observed_rate_kbps: air.and_then(|air| match air.rate {
            PhyRate::Legacy(rate) => Some(rate.kbps()),
            _ => None,
        }),
        sequence: trace.sequence.get(),
        publications: u16::try_from(trace.publications).ok()?,
        transmissions: air.map_or(0, |air| air.transmissions.get()),
        end,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::num::NonZeroU16;
    use oer_ieee80211_port::{Air, PhyRate};
    use oer_ieee80211_wire::{phy::LegacyRate, sequence::SequenceNumber};

    fn trace(observed: LegacyRate, state: MpduState) -> outcomes::Trace<(), (), ()> {
        outcomes::Trace {
            admitted: outcomes::Identity {
                ticket: 0,
                buffer: 1,
            },
            returned: outcomes::Identity {
                ticket: 0,
                buffer: 1,
            },
            state,
            rate: PhyRate::Legacy(LegacyRate::Ofdm6M),
            sequence: SequenceNumber::from_sequence_control(0x1230),
            publications: 1,
            outcome: outcomes::Outcome::Single(TxOutcome::Ended(SingleEnd::AckTimeout(Air {
                transmissions: NonZeroU16::MIN,
                rate: PhyRate::Legacy(observed),
            }))),
        }
    }
    #[test]
    fn projection_does_not_hide_a_different_terminal_rate_or_owner_state() {
        let expected = outcome_trace(&trace(LegacyRate::Ofdm6M, MpduState::Sent)).unwrap();
        let changed = outcome_trace(&trace(LegacyRate::Ofdm12M, MpduState::Sent)).unwrap();
        assert_eq!(expected.rate_kbps, changed.rate_kbps);
        assert_ne!(expected, changed);
        let fresh = outcome_trace(&trace(LegacyRate::Ofdm6M, MpduState::Fresh)).unwrap();
        assert!(!fresh.sent);
        assert_ne!(expected, fresh);
    }

    #[test]
    fn ownership_projection_keeps_unknown_and_exchanged_body_allocations() {
        use conformance::{End, OPERATIONS, Resources, Returned};
        let first = Resources {
            ticket: 0,
            buffer: 0,
            class: OPERATIONS[0].class,
            body: 11,
        };
        let second = Resources {
            ticket: 1,
            buffer: 0,
            class: OPERATIONS[1].class,
            body: 22,
        };
        let mut raw = conformance::Trace {
            returned: [
                Returned {
                    operation: OPERATIONS[0],
                    expected: first,
                    actual: first,
                    end: End::Ended,
                },
                Returned {
                    operation: OPERATIONS[1],
                    expected: second,
                    actual: second,
                    end: End::Ended,
                },
            ],
            contexts_dropped: 2,
            before_teardown_dropped: 2,
            publications: 2,
        };
        let original = ownership_trace(&raw).unwrap();
        raw.returned[0].actual.body = second.body;
        let exchanged = ownership_trace(&raw).unwrap();
        assert_eq!(exchanged.returned[0].unwrap().actual.body, Some(1));
        assert_ne!(original, exchanged);
        raw.returned[0].actual.body = 33;
        let unknown = ownership_trace(&raw).unwrap();
        assert_eq!(unknown.returned[0].unwrap().actual.body, None);
        assert_ne!(original, unknown);
    }
}
