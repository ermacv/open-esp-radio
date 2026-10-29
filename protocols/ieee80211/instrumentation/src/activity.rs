//! Independent aggregate kinds for the composed station's observations.

use oer_trace::{Event, Kind};

use crate::{Observation, SERVICE, event::unit_event};

unit_event!(
    /// A successful open authentication response addressed to the local station.
    StationAuthenticated = 22 on SERVICE,
    "station.authenticated"
);

/// A beacon from the configured target, with wrapping monotonic milliseconds.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationBeacon {
    pub at_millis: u32,
}
impl Event for StationBeacon {
    const KIND: Kind = Kind::new(oer_trace::Domain::Ieee80211, 23);
    const CHANNEL: oer_trace::Channel = SERVICE;

    fn encode(&self) -> [u32; 2] {
        [self.at_millis, 0]
    }

    fn decode([at_millis, reserved]: [u32; 2]) -> Option<Self> {
        (reserved == 0).then_some(Self { at_millis })
    }
}
impl core::fmt::Display for StationBeacon {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "station.beacon at_millis={}", self.at_millis)
    }
}

unit_event!(
    /// Rate selection accepted the associated peer's capabilities.
    StationAssociated = 24 on SERVICE,
    "station.associated"
);
unit_event!(
    /// A single-TX terminal had an unknown outcome, of any cause.
    TxIndeterminate = 25 on SERVICE,
    "station.tx-indeterminate"
);
unit_event!(
    /// Rate selection received a single-TX hardware timeout.
    TxHardwareTimeout = 26 on SERVICE,
    "station.tx-hardware-timeout"
);
unit_event!(
    /// One EDCA queue had a sampled collision pending, not a retry count.
    TxQueueCollision = 27 on SERVICE,
    "station.tx-queue-collision"
);
unit_event!(
    /// A control frame completed with delivery confirmed.
    ControlDelivered = 28 on SERVICE,
    "station.control-delivered"
);

/// The station activity decoded from an event, for image-local correlation.
/// Each variant has its own event kind, so the generic capture service keeps
/// independent aggregate counts even when its bounded history is incomplete.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationActivity {
    Authenticated,
    Beacon(u32),
    Associated,
    Indeterminate,
    HardwareTimeout,
    QueueCollision,
    ControlDelivered,
}
impl StationActivity {
    /// Project a received event without depending on the station facade.
    pub fn of<E: Event>(event: &E) -> Option<Self> {
        match Observation::of(event)? {
            Observation::StationAuthenticated(_) => Some(Self::Authenticated),
            Observation::StationBeacon(event) => Some(Self::Beacon(event.at_millis)),
            Observation::StationAssociated(_) => Some(Self::Associated),
            Observation::TxIndeterminate(_) => Some(Self::Indeterminate),
            Observation::TxHardwareTimeout(_) => Some(Self::HardwareTimeout),
            Observation::TxQueueCollision(_) => Some(Self::QueueCollision),
            Observation::ControlDelivered(_) => Some(Self::ControlDelivered),
            _ => None,
        }
    }
}
