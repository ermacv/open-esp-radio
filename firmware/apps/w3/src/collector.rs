//! The station service's observations the W3 report counts, collected only
//! in the image's `trace` instrumentation class: the service keeps no
//! counters, so the report's pre-bridge receive row comes from its events.

use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

use oer_ieee80211_instrumentation::{Observation, RxDrop, RxMsduRefusal};
use oer_trace::{Event, Observer};

/// The reason bit of a security drop in the report's reason set; frame
/// drops take the bits of their codes below it.
const SECURITY_REASON: u32 = 9;
/// The reason bit of a received MSDU too long for the network stack.
const TOO_LONG_REASON: u32 = 10;

/// Counts of the service's receive drops and lost port events, saturating
/// like the report's other counters. The image places one in a static and
/// gives the station service a reference as its observer. Only the service
/// task observes, so each count is a plain load and store.
pub struct ServiceTally {
    queue_full: AtomicU32,
    rejected: AtomicU32,
    reasons: AtomicU32,
    events_lost: AtomicU32,
}

impl ServiceTally {
    pub const fn new() -> Self {
        Self {
            queue_full: AtomicU32::new(0),
            rejected: AtomicU32::new(0),
            reasons: AtomicU32::new(0),
            events_lost: AtomicU32::new(0),
        }
    }

    fn count(count: &AtomicU32) {
        count.store(count.load(Relaxed).saturating_add(1), Relaxed);
    }

    fn reason(&self, bit: u32) {
        let mask = 1_u32.checked_shl(bit).unwrap_or(0);
        self.reasons
            .store(self.reasons.load(Relaxed) | mask, Relaxed);
    }

    /// Count one observation; the report keeps no others.
    pub fn observe(&self, observation: Observation) {
        match observation {
            Observation::RxFrameDropped(dropped) => {
                Self::count(&self.rejected);
                match dropped.reason {
                    RxDrop::Frame(reason) => self.reason(reason.code()),
                    RxDrop::Security(_) => self.reason(SECURITY_REASON),
                    RxDrop::Draining => {}
                }
            }
            Observation::RxMsduRefused(refused) => match refused.reason {
                RxMsduRefusal::TooLong => {
                    Self::count(&self.rejected);
                    self.reason(TOO_LONG_REASON);
                }
                RxMsduRefusal::QueueFull => Self::count(&self.queue_full),
            },
            Observation::PortEventsLost(_) => Self::count(&self.events_lost),
            _ => {}
        }
    }

    /// Set the report's service receive row from the counts.
    pub fn report(&self, report: &mut crate::report::Report) {
        report.service_rx_queue_full = self.queue_full.load(Relaxed);
        report.service_rx_rejected = self.rejected.load(Relaxed);
        report.service_rx_reasons = u16::try_from(self.reasons.load(Relaxed)).unwrap_or(u16::MAX);
        report.port_events_lost = self.events_lost.load(Relaxed);
    }
}

impl Default for ServiceTally {
    fn default() -> Self {
        Self::new()
    }
}

impl Observer for ServiceTally {
    fn event<E: Event + Copy + 'static>(&self, event: E) {
        if let Some(observation) = Observation::of(&event) {
            self.observe(observation);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_ieee80211_instrumentation::{
        FrameDrop, PortEventsLost, RxFrameDropped, RxMsduRefused, SecurityDrop, TokenLost,
    };

    #[test]
    fn the_report_row_counts_the_service_receive_events() {
        let tally = ServiceTally::new();
        let observer = &tally;
        for event in [
            RxFrameDropped {
                reason: RxDrop::Frame(FrameDrop::Replay),
            },
            RxFrameDropped {
                reason: RxDrop::Security(SecurityDrop::InvalidMic),
            },
            RxFrameDropped {
                reason: RxDrop::Draining,
            },
        ] {
            oer_trace::observe!(observer, event);
        }
        oer_trace::observe!(
            observer,
            RxMsduRefused {
                reason: RxMsduRefusal::TooLong
            }
        );
        oer_trace::observe!(
            observer,
            RxMsduRefused {
                reason: RxMsduRefusal::QueueFull
            }
        );
        oer_trace::observe!(observer, PortEventsLost);
        oer_trace::observe!(observer, TokenLost);
        let mut report = crate::report::Report::new(1, [0; 6], [0; 6]);
        tally.report(&mut report);
        assert_eq!(report.service_rx_rejected, 4);
        assert_eq!(report.service_rx_queue_full, 1);
        assert_eq!(report.port_events_lost, 1);
        assert_eq!(report.service_rx_reasons, (1 << 6) | (1 << 9) | (1 << 10));
    }

    #[test]
    fn counts_saturate() {
        let tally = ServiceTally::new();
        tally.events_lost.store(u32::MAX, Relaxed);
        tally.observe(Observation::PortEventsLost(PortEventsLost));
        let mut report = crate::report::Report::new(1, [0; 6], [0; 6]);
        tally.report(&mut report);
        assert_eq!(report.port_events_lost, u32::MAX);
    }
}
