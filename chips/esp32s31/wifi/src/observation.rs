//! Value-only station observations; collectors and counters belong to images.
use oer_trace::{Channel, Domain, Event, Kind};
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Observation {
    Authenticated,
    Beacon(u32),
    Associated,
    Indeterminate,
    HardwareTimeout,
    QueueCollision,
    ControlDelivered,
}
impl Event for Observation {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 240);
    const CHANNEL: Channel = oer_ieee80211_instrumentation::SERVICE;
    fn encode(&self) -> [u32; 2] {
        match self {
            Self::Authenticated => [0, 0],
            Self::Beacon(t) => [1, *t],
            Self::Associated => [2, 0],
            Self::Indeterminate => [3, 0],
            Self::HardwareTimeout => [4, 0],
            Self::QueueCollision => [5, 0],
            Self::ControlDelivered => [6, 0],
        }
    }
    fn decode(w: [u32; 2]) -> Option<Self> {
        match w {
            [0, 0] => Some(Self::Authenticated),
            [1, t] => Some(Self::Beacon(t)),
            [2, 0] => Some(Self::Associated),
            [3, 0] => Some(Self::Indeterminate),
            [4, 0] => Some(Self::HardwareTimeout),
            [5, 0] => Some(Self::QueueCollision),
            [6, 0] => Some(Self::ControlDelivered),
            _ => None,
        }
    }
}
impl Observation {
    pub fn of<E: Event>(event: &E) -> Option<Self> {
        (E::KIND == Self::KIND)
            .then(|| Self::decode(event.encode()))
            .flatten()
    }
}
