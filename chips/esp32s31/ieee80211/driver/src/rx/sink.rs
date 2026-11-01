//! The W1 ingress policy: preview by frame control, admit everything,
//! count every drop.
//!
//! [`frame_control_preview`] classifies a completed unit by the frame
//! control it begins with, so management, control and unprotected data stay
//! [`IngressClass::Critical`]; [`unclassified_preview`] is the preview of a
//! unit whose header could not be read. Every W1 publisher previews through
//! them. [`CountingAdmission`] admits everything as
//! [`AdmitAll`](super::transaction::AdmitAll) does and counts every drop by
//! its reason.

use super::{
    counters::WifiRxCounters,
    transaction::{
        Admission, CompletedUnit, Discard, IngressClass, IngressRoute, Observation, Preview,
    },
};

/// The preview of `unit` whose MPDU begins with `bytes`: classified by its
/// frame control.
pub fn frame_control_preview(unit: CompletedUnit, bytes: [u8; 24]) -> Preview {
    let [low, high, ..] = bytes;
    let frame_control = Some(u16::from_le_bytes([low, high]));
    Preview {
        unit,
        frame_control,
        class: IngressClass::of(frame_control),
        route: IngressRoute::Standalone,
    }
}

/// The preview of `unit` whose header could not be read.
pub fn unclassified_preview(unit: CompletedUnit) -> Preview {
    Preview {
        unit,
        frame_control: None,
        class: IngressClass::Unclassified,
        route: IngressRoute::Malformed,
    }
}

/// Admit everything; count every staged unit, every drop and every blocked
/// admission.
pub struct CountingAdmission<'a> {
    counters: &'a WifiRxCounters,
}

impl<'a> CountingAdmission<'a> {
    pub const fn new(counters: &'a WifiRxCounters) -> Self {
        Self { counters }
    }
}

impl Admission for CountingAdmission<'_> {
    fn observe(&self, observation: Observation) {
        match observation {
            Observation::DiscardRetained { reason, .. } => self.counters.discarded(reason),
            Observation::OverloadDiscardedAndRecycled(_) => {
                self.counters.discarded(Discard::OverloadBulk);
            }
            Observation::BulkAdmissionBlocked(_) | Observation::CriticalAdmissionBlocked(_) => {
                self.counters.admission_blocked();
            }
            // Every staged unit, whichever sink takes it, counts here once.
            Observation::Staged(unit) => self.counters.received(unit.payload_length),
            Observation::CriticalReserveAdmitted(_) => {}
        }
    }
}

#[cfg(test)]
mod tests;
