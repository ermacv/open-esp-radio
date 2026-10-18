//! The W1 receive sink: count every staged unit, then release it.
//!
//! [`CountingSink`] is the publisher of the receive runner while no upper
//! layer consumes frames: it counts the unit and its bytes and drops the
//! staged lease at once, which marks the original DMA buffer released for
//! the ring owner to rearm. [`CountingAdmission`] admits everything as
//! [`AdmitAll`](super::transaction::AdmitAll) does and counts every drop by
//! its reason.

use oer_esp32s31_ieee80211_mac::rx::pool::NetworkRxFrame;

use super::{
    counters::WifiRxCounters,
    transaction::{
        Admission, CompletedUnit, IngressClass, IngressRoute, Observation, Preview, Publisher,
    },
};

/// A publisher that counts each unit and releases its lease.
pub struct CountingSink<'a> {
    counters: &'a WifiRxCounters,
}

impl<'a> CountingSink<'a> {
    pub const fn new(counters: &'a WifiRxCounters) -> Self {
        Self { counters }
    }
}

impl<'pool, const CAPACITY: usize, const SLOTS: usize> Publisher<'pool, CAPACITY, SLOTS>
    for CountingSink<'_>
{
    const DEPTH: usize = SLOTS;

    /// Every lease is released inside `try_send`, so the sink never fills.
    fn free_capacity(&self) -> usize {
        SLOTS
    }

    fn preview(&self, unit: CompletedUnit, bytes: [u8; 24]) -> Preview {
        let [low, high, ..] = bytes;
        let frame_control = Some(u16::from_le_bytes([low, high]));
        Preview {
            unit,
            frame_control,
            class: IngressClass::of(frame_control),
            route: IngressRoute::Standalone,
        }
    }

    fn unclassified_preview(&self, unit: CompletedUnit) -> Preview {
        Preview {
            unit,
            frame_control: None,
            class: IngressClass::Unclassified,
            route: IngressRoute::Malformed,
        }
    }

    fn try_send(
        &self,
        frame: NetworkRxFrame<'pool, SLOTS, CAPACITY>,
    ) -> Result<(), NetworkRxFrame<'pool, SLOTS, CAPACITY>> {
        self.counters.received(frame.length());
        drop(frame);
        Ok(())
    }
}

/// Admit everything; count every drop and every blocked admission.
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
                self.counters
                    .discarded(super::transaction::Discard::OverloadBulk);
            }
            Observation::BulkAdmissionBlocked(_) | Observation::CriticalAdmissionBlocked(_) => {
                self.counters.admission_blocked();
            }
            Observation::Staged(_) | Observation::CriticalReserveAdmitted(_) => {}
        }
    }
}

#[cfg(test)]
mod tests;
