//! Event identity: which subsystem defines an event, which one it is, and
//! which host-controlled channel gates it.

/// The subsystem that defines an event's id and encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Domain {
    Platform = 1,
    Phy = 2,
    Ieee80211 = 3,
    Bluetooth = 4,
    Ieee802154 = 5,
}

impl Domain {
    /// The first mask bit and number of channels this domain owns.
    pub const fn channels(self) -> (u8, u8) {
        match self {
            Self::Platform => (0, 8),
            Self::Phy => (8, 12),
            Self::Ieee80211 => (20, 20),
            Self::Bluetooth => (40, 12),
            Self::Ieee802154 => (52, 12),
        }
    }

    pub const fn from_raw(raw: u8) -> Option<Self> {
        Some(match raw {
            1 => Self::Platform,
            2 => Self::Phy,
            3 => Self::Ieee80211,
            4 => Self::Bluetooth,
            5 => Self::Ieee802154,
            _ => return None,
        })
    }
}

/// One event type: its domain and the domain's own id for it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Kind {
    pub domain: Domain,
    pub event: u8,
}

impl Kind {
    pub const fn new(domain: Domain, event: u8) -> Self {
        Self { domain, event }
    }

    pub const fn raw(self) -> u16 {
        ((self.domain as u16) << 8) | self.event as u16
    }

    pub const fn from_raw(raw: u16) -> Option<Self> {
        match Domain::from_raw((raw >> 8) as u8) {
            Some(domain) => Some(Self::new(domain, raw as u8)),
            None => None,
        }
    }
}

/// A host-controlled enable bit. The 64 bits of the mask are split into
/// fixed per-domain ranges, so domains allocate channels independently.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Channel(u8);

impl Channel {
    /// The `index`th channel of `domain`; fails to compile in a constant
    /// when the domain's range has no such channel.
    pub const fn new(domain: Domain, index: u8) -> Self {
        let (first, count) = domain.channels();
        assert!(index < count, "the domain has no such trace channel");
        Self(first + index)
    }

    /// The mask bit of this channel.
    pub const fn bit_index(self) -> u8 {
        self.0
    }

    pub const fn mask(self) -> u64 {
        1 << self.0
    }

    pub(crate) const fn word(self) -> usize {
        (self.0 / 32) as usize
    }

    pub(crate) const fn bit(self) -> u32 {
        1 << (self.0 % 32)
    }
}

/// A trace point. Its encoding is the contract between the target that emits
/// it and the host that decodes it, so both link the same type.
pub trait Event: Sized {
    const KIND: Kind;
    const CHANNEL: Channel;

    fn encode(&self) -> [u32; 2];

    /// `None` for words this type never encodes.
    fn decode(words: [u32; 2]) -> Option<Self>;
}
