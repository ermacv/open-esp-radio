//! P2 publication evidence for the image declaring trace and debug-control.
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

/// A terminal successfully entered the real port event queue.
pub struct TerminalPosted;
impl Event for TerminalPosted {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 102);
    const CHANNEL: Channel = Publication::CHANNEL;
    fn encode(&self) -> [u32; 2] {
        [0, 0]
    }
    fn decode(words: [u32; 2]) -> Option<Self> {
        (words == [0, 0]).then_some(Self)
    }
}
/// Snapshot of the encoded DMA input immediately before reservation.
pub const ENCODED_MPDU: Kind = Kind::new(Domain::Ieee80211, 101);
#[cfg(oer_trace)]
#[inline(always)]
pub(crate) fn encoded_mpdu(bytes: &[u8]) {
    if oer_trace::enabled(Publication::CHANNEL) {
        oer_trace::capture(ENCODED_MPDU, |snapshot| {
            snapshot.push(bytes.len() as u32);
            for chunk in bytes.chunks(4) {
                let mut word = [0; 4];
                for (out, byte) in word.iter_mut().zip(chunk) {
                    *out = *byte;
                }
                snapshot.push(u32::from_le_bytes(word));
            }
        });
    }
}
