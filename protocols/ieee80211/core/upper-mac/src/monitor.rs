//! Monitor admission and channel policy above the lower-MAC monitor tap.
//!
//! The tap publishes every frame at its point of the receive path; these
//! values select which frames a monitor keeps and which channels it visits.

use core::{fmt, num::NonZeroU16};

use oer_ieee80211_contracts_port::{
    MonitorFrame, MonitorInjectionChannelBinding, MonitorInjectionRate, MonitorInjectionRequest,
    RxEvidence,
};
pub use oer_ieee80211_wire::monitor_injection::MonitorInjectionFrameError;
use oer_ieee80211_wire::{
    channel::WifiChannel,
    frame::{AddressFields, FrameControlFields, FrameType},
    monitor_injection::MonitorInjectionMpdu,
};

/// The injection request of retained no-FCS MPDU bytes, validated again as
/// an injected MPDU before the port sees them.
pub fn stored_injection_request(
    binding: MonitorInjectionChannelBinding,
    rate: MonitorInjectionRate,
    mpdu: &[u8],
) -> Result<MonitorInjectionRequest<'_>, MonitorInjectionFrameError> {
    MonitorInjectionMpdu::from_no_fcs(mpdu)
        .map(|mpdu| MonitorInjectionRequest::new(binding, rate, mpdu))
}

/// Maximum number of ordered 2.4-GHz channels in one monitor hopping cycle.
///
/// This is a bounded product policy, not a claim that fourteen entries exhaust
/// every valid 20/40-MHz channel geometry. Each retained entry carries the
/// complete validated [`WifiChannel`], including its width relationship.
pub const MONITOR_CHANNEL_SEQUENCE_CAPACITY: usize = 14;

/// Allocation-free ordered channel cycle with one nonzero dwell per channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorChannelSequence {
    channels: [WifiChannel; MONITOR_CHANNEL_SEQUENCE_CAPACITY],
    length: u8,
    dwell_millis: NonZeroU16,
}

impl MonitorChannelSequence {
    #[deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )]
    pub fn new(
        channels: &[WifiChannel],
        dwell_millis: NonZeroU16,
    ) -> Result<Self, MonitorChannelSequenceError> {
        let Some(&first) = channels.first() else {
            return Err(MonitorChannelSequenceError::Empty);
        };
        if channels.len() > MONITOR_CHANNEL_SEQUENCE_CAPACITY {
            return Err(MonitorChannelSequenceError::TooMany {
                requested: channels.len(),
                capacity: MONITOR_CHANNEL_SEQUENCE_CAPACITY,
            });
        }
        for (index, &channel) in channels.iter().enumerate() {
            if channels
                .iter()
                .take(index)
                .any(|earlier| *earlier == channel)
            {
                return Err(MonitorChannelSequenceError::Duplicate(channel));
            }
        }
        let mut stored = [first; MONITOR_CHANNEL_SEQUENCE_CAPACITY];
        for (slot, &channel) in stored.iter_mut().zip(channels) {
            *slot = channel;
        }
        Ok(Self {
            channels: stored,
            length: channels.len() as u8,
            dwell_millis,
        })
    }

    #[deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )]
    pub fn channels(&self) -> &[WifiChannel] {
        self.channels
            .get(..usize::from(self.length))
            .unwrap_or_default()
    }

    pub const fn len(self) -> u8 {
        self.length
    }

    pub const fn is_empty(self) -> bool {
        self.length == 0
    }

    pub const fn first(self) -> WifiChannel {
        self.channels[0]
    }

    pub const fn dwell(self) -> NonZeroU16 {
        self.dwell_millis
    }

    pub const fn dwell_millis(self) -> u16 {
        self.dwell_millis.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorChannelSequenceError {
    Empty,
    TooMany { requested: usize, capacity: usize },
    Duplicate(WifiChannel),
}

impl fmt::Display for MonitorChannelSequenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("a monitor hopping cycle cannot be empty"),
            Self::TooMany {
                requested,
                capacity,
            } => write!(
                formatter,
                "monitor hopping requested {requested} channels but capacity is {capacity}"
            ),
            Self::Duplicate(channel) => {
                write!(formatter, "monitor hopping repeats channel {channel:?}")
            }
        }
    }
}

impl core::error::Error for MonitorChannelSequenceError {}

/// Physical-channel policy for one exclusive standalone monitor role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorChannelPolicy {
    /// Preserve the historical behavior: capture on one channel until stop.
    Fixed(WifiChannel),
    /// Repeatedly traverse one bounded ordered cycle.
    Hopping(MonitorChannelSequence),
}

impl MonitorChannelPolicy {
    pub const fn fixed(channel: WifiChannel) -> Self {
        Self::Fixed(channel)
    }

    pub const fn hopping(sequence: MonitorChannelSequence) -> Self {
        Self::Hopping(sequence)
    }

    pub const fn initial_channel(self) -> WifiChannel {
        match self {
            Self::Fixed(channel) => channel,
            Self::Hopping(sequence) => sequence.first(),
        }
    }

    pub const fn hopping_sequence(self) -> Option<MonitorChannelSequence> {
        match self {
            Self::Fixed(_) => None,
            Self::Hopping(sequence) => Some(sequence),
        }
    }
}

/// IEEE 802.11 frame class used by the bounded monitor filter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum MonitorFrameType {
    Management = 0,
    Control = 1,
    Data = 2,
    Extension = 3,
}

/// Fixed-size set of IEEE 802.11 frame classes.
///
/// This is deliberately not an allocated predicate tree. Filtering remains a
/// bounded amount of work in the RX bottom half, before capture-pool copying.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorFrameTypeMask(u8);

impl MonitorFrameTypeMask {
    pub const NONE: Self = Self(0);
    pub const MANAGEMENT: Self = Self(1 << MonitorFrameType::Management as u8);
    pub const CONTROL: Self = Self(1 << MonitorFrameType::Control as u8);
    pub const DATA: Self = Self(1 << MonitorFrameType::Data as u8);
    pub const EXTENSION: Self = Self(1 << MonitorFrameType::Extension as u8);
    pub const ALL: Self = Self(0x0f);

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    pub const fn contains(self, frame_type: MonitorFrameType) -> bool {
        self.0 & (1 << frame_type as u8) != 0
    }
}

/// Allocation-free monitor admission policy.
///
/// All predicates are conjunctive. An RSSI predicate rejects frames whose
/// selected tap has no RSSI evidence. Address matching examines only address
/// fields defined by the decoded IEEE 802.11 frame class, never payload bytes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorFilter {
    frame_types: MonitorFrameTypeMask,
    minimum_rssi_dbm: Option<i8>,
    any_address: Option<[u8; 6]>,
}

impl MonitorFilter {
    pub const fn all() -> Self {
        Self {
            frame_types: MonitorFrameTypeMask::ALL,
            minimum_rssi_dbm: None,
            any_address: None,
        }
    }

    pub const fn frame_types(mut self, frame_types: MonitorFrameTypeMask) -> Self {
        self.frame_types = frame_types;
        self
    }

    pub const fn minimum_rssi_dbm(mut self, minimum_rssi_dbm: i8) -> Self {
        self.minimum_rssi_dbm = Some(minimum_rssi_dbm);
        self
    }

    pub const fn any_address(mut self, address: [u8; 6]) -> Self {
        self.any_address = Some(address);
        self
    }

    pub const fn selected_frame_types(self) -> MonitorFrameTypeMask {
        self.frame_types
    }

    pub const fn selected_minimum_rssi_dbm(self) -> Option<i8> {
        self.minimum_rssi_dbm
    }

    pub const fn selected_any_address(self) -> Option<[u8; 6]> {
        self.any_address
    }

    pub fn accepts<Rate>(self, frame: &MonitorFrame<'_, Rate>) -> bool {
        let Some(control) = frame_control(frame.bytes) else {
            return false;
        };
        let frame_type = frame_type(control);
        if !self.frame_types.contains(frame_type) {
            return false;
        }
        if let Some(minimum) = self.minimum_rssi_dbm {
            let observed = match frame.metadata.rssi_dbm {
                RxEvidence::HardwareObserved(value) | RxEvidence::ProtocolValidated(value) => value,
                RxEvidence::Unavailable => return false,
            };
            if observed < minimum {
                return false;
            }
        }
        self.any_address
            .is_none_or(|address| frame_contains_address(frame.bytes, address))
    }
}

impl Default for MonitorFilter {
    fn default() -> Self {
        Self::all()
    }
}

fn frame_control(bytes: &[u8]) -> Option<FrameControlFields> {
    bytes
        .first_chunk::<2>()
        .copied()
        .map(FrameControlFields::from_bytes)
}

fn frame_type(control: FrameControlFields) -> MonitorFrameType {
    match control.kind() {
        FrameType::Management => MonitorFrameType::Management,
        FrameType::Control => MonitorFrameType::Control,
        FrameType::Data => MonitorFrameType::Data,
        FrameType::Extension | FrameType::Reserved(_) => MonitorFrameType::Extension,
    }
}

/// Whether `address` is one of the addresses the frame holds; a frame whose
/// address fields cannot be read holds none.
fn frame_contains_address(bytes: &[u8], address: [u8; 6]) -> bool {
    AddressFields::parse(bytes).is_ok_and(|fields| fields.addresses().any(|held| held == address))
}

#[cfg(test)]
mod tests;
