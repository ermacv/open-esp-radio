//! Small Wi-Fi driver observation values; images count their kinds.
use oer_trace::{Channel, Domain, Event, Kind};
mod edca;
pub use edca::{EdcaDraw, EdcaRecord, EdcaTable};

/// A successful publication of one admitted MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Publication {
    pub ticket: u8,
    pub buffer: u8,
}
impl Event for Publication {
    const KIND: Kind = Kind::new(Domain::Ieee80211, 100);
    const CHANNEL: Channel = const {
        match Channel::new(Domain::Ieee80211, 19) {
            Ok(channel) => channel,
            Err(_) => panic!("invalid trace channel"),
        }
    };
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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

#[cfg(test)]
mod tests {
    use super::*;
    use core::{
        cell::Cell,
        sync::atomic::{AtomicU32, Ordering::Relaxed},
    };
    struct ImageObserver(AtomicU32);
    impl oer_trace::Observer for ImageObserver {
        fn event<E: Event + Copy + 'static>(&self, _: E) {
            assert_eq!(E::KIND, Publication::KIND);
            self.0.fetch_add(1, Relaxed);
        }
    }
    #[test]
    fn driver_value_is_counted_once_and_disabled_arguments_are_not_evaluated() {
        let observer = ImageObserver(AtomicU32::new(0));
        let evaluated = Cell::new(0u32);
        oer_trace::observe!(observer, {
            evaluated.set(evaluated.get() + 1);
            Publication {
                ticket: 2,
                buffer: 3,
            }
        });
        assert_eq!(evaluated.get(), u32::from(cfg!(oer_trace)));
        assert_eq!(observer.0.load(Relaxed), u32::from(cfg!(oer_trace)));
    }
}
