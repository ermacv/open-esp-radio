//! The facade's own trace events: what the composed station saw at the port
//! and in its rate selection. They are the facade's vocabulary on the image's
//! trace observer; the protocol's events reach the image's event sink
//! instead ([`crate::Binding::Sink`]).
//!
//! Each kind keeps its id in the IEEE 802.11 trace domain (the S31 driver's
//! lower-MAC trace uses ids from 100) and its channel.

use oer_trace::{Channel, Domain, Event, Kind};

/// The station activity channel.
const SERVICE: Channel = const {
    match Channel::new(Domain::Ieee80211, 1) {
        Ok(channel) => channel,
        Err(_) => panic!("the station activity channel lies in the IEEE 802.11 range"),
    }
};
/// The rate selection channel.
const AGGREGATION: Channel = const {
    match Channel::new(Domain::Ieee80211, 4) {
        Ok(channel) => channel,
        Err(_) => panic!("the rate selection channel lies in the IEEE 802.11 range"),
    }
};

/// An event without fields.
macro_rules! unit_event {
    ($(#[$meta:meta])* $name:ident = $id:literal on $channel:path, $text:literal) => {
        $(#[$meta])*
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        pub struct $name;

        impl Event for $name {
            const KIND: Kind = Kind::new(Domain::Ieee80211, $id);
            const CHANNEL: Channel = $channel;

            fn encode(&self) -> [u32; 2] {
                [0, 0]
            }

            fn decode(words: [u32; 2]) -> Option<Self> {
                (words == [0, 0]).then_some(Self)
            }
        }

        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                f.write_str($text)
            }
        }
    };
}

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
    const KIND: Kind = Kind::new(Domain::Ieee80211, 23);
    const CHANNEL: Channel = SERVICE;

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

/// First rate of the data retry plan after association or a learned
/// terminal. `None` means that the selection has no admissible data plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxRateSelected {
    pub kbps: Option<u32>,
}

impl Event for TxRateSelected {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 40);
    const CHANNEL: Channel = AGGREGATION;

    fn encode(&self) -> [u32; 2] {
        [self.kbps.unwrap_or(0), u32::from(self.kbps.is_some())]
    }

    fn decode([kbps, known]: [u32; 2]) -> Option<Self> {
        match known {
            0 if kbps == 0 => Some(Self { kbps: None }),
            1 => Some(Self { kbps: Some(kbps) }),
            _ => None,
        }
    }
}

impl core::fmt::Display for TxRateSelected {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "TxRateSelected {self:?}")
    }
}

/// The station service's spans, which [`ServiceSpan`] reports.
pub use oer_ieee80211_runtime::station::ServiceSpan as Span;

/// The start or end of a stretch of the station service's work.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ServiceSpan {
    pub span: oer_ieee80211_runtime::station::ServiceSpan,
    pub end: bool,
}

impl Event for ServiceSpan {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 29);
    const CHANNEL: Channel = SERVICE;

    fn encode(&self) -> [u32; 2] {
        [u32::from(self.span as u8), u32::from(self.end)]
    }

    fn decode([span, end]: [u32; 2]) -> Option<Self> {
        Some(Self {
            span: oer_ieee80211_runtime::station::ServiceSpan::from_number(
                u8::try_from(span).ok()?,
            )?,
            end: match end {
                0 => false,
                1 => true,
                _ => return None,
            },
        })
    }
}

impl core::fmt::Display for ServiceSpan {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let edge = if self.end { "end" } else { "start" };
        write!(f, "station.span {:?} {edge}", self.span)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn round_trip<E: Event + PartialEq + core::fmt::Debug>(event: E) {
        assert_eq!(E::decode(event.encode()), Some(event));
    }

    #[test]
    fn every_facade_event_decodes_to_itself() {
        round_trip(StationAuthenticated);
        round_trip(StationBeacon { at_millis: 7 });
        round_trip(StationAssociated);
        round_trip(TxIndeterminate);
        round_trip(TxHardwareTimeout);
        round_trip(TxQueueCollision);
        round_trip(ControlDelivered);
        round_trip(TxRateSelected { kbps: None });
        round_trip(TxRateSelected { kbps: Some(65_000) });
        for span in oer_ieee80211_runtime::station::ServiceSpan::ALL {
            round_trip(ServiceSpan { span, end: false });
            round_trip(ServiceSpan { span, end: true });
        }
        assert_eq!(ServiceSpan::decode([0, 2]), None);
        assert_eq!(ServiceSpan::decode([9, 0]), None);
        assert_eq!(StationBeacon::decode([7, 1]), None);
        assert_eq!(TxRateSelected::decode([1, 0]), None);
    }
}
