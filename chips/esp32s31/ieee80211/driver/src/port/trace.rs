//! P2 publication evidence for the image declaring trace and debug-control.
use oer_trace::{Channel, Domain, Event, Kind};
mod edca;
pub(crate) use edca::contention;
pub use edca::{EdcaDraw, EdcaRecord, EdcaTable};

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

/// Internal rollback and the actual retained command owners before fail-stop.
pub struct GroupRollback {
    pub owners: [u8; 2],
    pub previous_key_id: u8,
    pub attempted_key_id: u8,
    pub restored: bool,
}
impl Event for GroupRollback {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 106);
    const CHANNEL: Channel = Publication::CHANNEL;
    fn encode(&self) -> [u32; 2] {
        [
            u32::from_le_bytes([
                self.owners[0],
                self.owners[1],
                self.previous_key_id,
                self.attempted_key_id,
            ]),
            u32::from(self.restored),
        ]
    }
    fn decode([owners, restored]: [u32; 2]) -> Option<Self> {
        let [slot, permission, previous_key_id, attempted_key_id] = owners.to_le_bytes();
        Some(Self {
            owners: [slot, permission],
            previous_key_id,
            attempted_key_id,
            restored: match restored {
                0 => false,
                1 => true,
                _ => return None,
            },
        })
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
