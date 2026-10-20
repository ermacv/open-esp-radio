//! Publication evidence from the P2 owner. Compiles out without `oer_trace`.
use oer_trace::{Channel, Domain, Event, Kind};

/// A successful publication of one admitted MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Publication {
    pub ticket: u8,
    pub buffer: u8,
}
impl Event for Publication {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 100);
    const CHANNEL: Channel = Channel::new(Domain::Ieee80211, 19);
    fn encode(&self) -> [u32; 2] {
        [u32::from(self.ticket), u32::from(self.buffer)]
    }
    fn decode([ticket, buffer]: [u32; 2]) -> Option<Self> {
        Some(Self {
            ticket: u8::try_from(ticket).ok()?,
            buffer: u8::try_from(buffer).ok()?,
        })
    }
}
