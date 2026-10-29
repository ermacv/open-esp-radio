//! MPDU containment is evidence, never a physical aggregate/member count.
use crate::event::coded;
use oer_trace::{Channel, Domain, Event, Kind};

pub const AGGREGATION: Channel = const {
    match Channel::new(Domain::Ieee80211, 4) {
        Ok(channel) => channel,
        Err(_) => panic!("aggregation channel is in the IEEE 802.11 range"),
    }
};
coded! { pub enum Evidence {
    Unavailable = 0, HardwareYes = 1, HardwareNo = 2, ProtocolYes = 3, ProtocolNo = 4,
} }
coded! { pub enum ReorderCause {
    Receive = 0, Timeout = 1, Bar = 2, Stop = 3, Clear = 4, Duplicate = 5, Rejected = 6,
} }
coded! { pub enum BaStop {
    Peer = 0, Replaced = 1, Timeout = 2, InvalidGap = 3, Clear = 4,
} }
macro_rules! event {
    ($name:ident, $id:literal, |$value:ident| $encode:expr, |$words:pat_param| $decode:expr) => {
        impl Event for $name {
            const KIND: Kind = Kind::new(Domain::Ieee80211, $id);
            const CHANNEL: Channel = AGGREGATION;
            fn encode(&$value) -> [u32; 2] { $encode }
            fn decode($words: [u32; 2]) -> Option<Self> { $decode }
        }
        impl core::fmt::Display for $name {
            fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                write!(f, "{} {:?}", stringify!($name), self)
            }
        }
    };
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxContainment {
    pub ampdu: Evidence,
    pub s_mpdu: Evidence,
    /// Normalized port MPDU bytes, excluding any trailers the port removed.
    /// None explicitly reports an unrepresentable length.
    pub bytes: Option<u32>,
}
event!(
    RxContainment,
    36,
    |self| [
        self.ampdu.code() | (self.s_mpdu.code() << 8) | (u32::from(self.bytes.is_some()) << 16),
        self.bytes.unwrap_or(0)
    ],
    |[flags, bytes]| {
        if flags >> 17 != 0 || (flags & (1 << 16) == 0 && bytes != 0) {
            return None;
        }
        Some(Self {
            ampdu: Evidence::of_code(flags & 0xff)?,
            s_mpdu: Evidence::of_code((flags >> 8) & 0xff)?,
            bytes: (flags & (1 << 16) != 0).then_some(bytes),
        })
    }
);
/// One decision's counts; Clear's released owners are recycled, not delivered.
/// Timeout's missing count denotes absent sequences, not dropped buffered MPDUs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxReorder {
    pub tid: u8,
    pub cause: ReorderCause,
    pub held: u8,
    pub released: u8,
    pub missing: u16,
}
event!(
    RxReorder,
    37,
    |self| [
        u32::from_le_bytes([self.tid, self.cause.code() as u8, self.held, self.released]),
        u32::from(self.missing)
    ],
    |[flags, missing]| {
        let [tid, cause, held, released] = flags.to_le_bytes();
        Some(Self {
            tid,
            cause: ReorderCause::of_code(u32::from(cause))?,
            held,
            released,
            missing: u16::try_from(missing).ok()?,
        })
    }
);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBaStarted {
    pub tid: u8,
    pub window: u16,
}
event!(
    RxBaStarted,
    38,
    |self| [u32::from(self.tid), u32::from(self.window)],
    |[tid, window]| {
        Some(Self {
            tid: u8::try_from(tid).ok()?,
            window: u16::try_from(window).ok()?,
        })
    }
);
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RxBaStopped {
    pub tid: u8,
    pub reason: BaStop,
}
event!(
    RxBaStopped,
    39,
    |self| [u32::from(self.tid), self.reason.code()],
    |[tid, reason]| {
        Some(Self {
            tid: u8::try_from(tid).ok()?,
            reason: BaStop::of_code(reason)?,
        })
    }
);
/// First rate of the data retry plan after association or a learned terminal.
/// None means that the selection has no admissible data plan.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TxRateSelected {
    pub kbps: Option<u32>,
}
event!(
    TxRateSelected,
    40,
    |self| [self.kbps.unwrap_or(0), u32::from(self.kbps.is_some())],
    |[kbps, known]| {
        match known {
            0 if kbps == 0 => Some(Self { kbps: None }),
            1 => Some(Self { kbps: Some(kbps) }),
            _ => None,
        }
    }
);
