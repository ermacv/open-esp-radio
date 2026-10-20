use oer_ieee80211_port::{
    conformance::{MpduState, catalog, outcomes},
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
    pub sequence: u16,
    pub publications: u16,
    pub transmissions: u16,
    pub end: End,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct CaseTrace {
    pub case: RunCase,
    pub outcome: OutcomeTrace,
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
        sequence: trace.sequence.get(),
        publications: u16::try_from(trace.publications).ok()?,
        transmissions: air.map_or(0, |air| air.transmissions.get()),
        end,
    })
}
