//! Read-only demand for the physical radio owner.
//!
//! Observing a due request neither advances the source scheduler's timestamps
//! nor acquires hardware. The owner may defer it without reporting tracking
//! success. Re-observe the current client set after a wake and before invoking
//! the consuming tracking transition; a copied demand is not an epoch token.

use super::parameters::PhyParamTrackRequest;
use crate::state::client::{PhyClientSnapshot, PhyPllTrackClass, PhyTrackTimeError, RadioClient};

/// Scheduling observation, independent of executor and hardware admission.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Schedule {
    /// No radio client requires tracking; no timer is needed.
    Inactive,
    /// Earliest absolute microsecond at which the current client set is due.
    At(u64),
    /// Tracking is due, but exclusive hardware access has not been granted.
    Due(Demand),
}

/// Value-only demand, never a hardware grant or a completed calibration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Demand {
    due_since_micros: u64,
    request: PhyParamTrackRequest,
}

impl Demand {
    /// First due instant under the source interval. This is not a measured
    /// maximum tolerable delay or a protocol event deadline.
    pub const fn due_since_micros(self) -> u64 {
        self.due_since_micros
    }

    /// Active tracking classes observed together. BT and IEEE 802.15.4 share
    /// one calibration class; this does not imply shared protocol ownership.
    pub const fn request(self) -> PhyParamTrackRequest {
        self.request
    }
}

impl PhyClientSnapshot {
    /// Inspect demand with one monotonic sample, without changing the owner.
    ///
    /// Once either active class is due, the existing outer scheduler requests
    /// all active classes. Clock reversal is rejected even before the next
    /// deadline. Inactive classes do not constrain the clock or arm timers.
    /// This observation does not replace the source-exact sample order in
    /// `evaluate_immediate_tracking` or its consuming ownership transition.
    pub fn tracking_schedule_at(self, now_micros: u64) -> Result<Schedule, PhyTrackTimeError> {
        let wifi = self.contains(RadioClient::Wifi);
        let shared =
            self.contains(RadioClient::Bluetooth) || self.contains(RadioClient::Ieee802154);
        for (active, class) in [
            (wifi, PhyPllTrackClass::Wifi),
            (shared, PhyPllTrackClass::BluetoothIeee802154),
        ] {
            let previous_micros = self.previous_micros(class);
            if active && now_micros < previous_micros {
                return Err(PhyTrackTimeError::TimeReversed {
                    class,
                    previous_micros,
                    now_micros,
                });
            }
        }
        Ok(match self.next_tracking_deadline_micros()? {
            None => Schedule::Inactive,
            Some(at) if now_micros < at => Schedule::At(at),
            Some(at) => Schedule::Due(Demand {
                due_since_micros: at,
                request: PhyParamTrackRequest::new(wifi, shared),
            }),
        })
    }
}
