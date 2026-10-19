//! Portable, non-retaining receive-monitor contract.
//!
//! A monitor sink observes a borrowed frame synchronously. It may copy the
//! frame into its own bounded storage, but it cannot retain the radio/DMA
//! borrow. Queue saturation is an ordinary observation loss and must never
//! backpressure the primary radio owner.

use core::{
    fmt,
    num::{NonZeroU16, NonZeroU32},
};

use oer_ieee80211_mac::channel::WifiChannel;

use crate::{
    MacRxEvidence, MacRxMetadata,
    interface::{ChannelContextId, MonitorTapPoint},
};

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
        let mut stored = [first; MONITOR_CHANNEL_SEQUENCE_CAPACITY];
        let mut length = 0_usize;
        for &channel in channels {
            if stored[..length].contains(&channel) {
                return Err(MonitorChannelSequenceError::Duplicate(channel));
            }
            stored[length] = channel;
            length += 1;
        }
        Ok(Self {
            channels: stored,
            length: length as u8,
            dwell_millis,
        })
    }

    pub fn channels(&self) -> &[WifiChannel] {
        &self.channels[..usize::from(self.length)]
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

/// Exact physical-channel dwell which may admit one monitor injection.
///
/// `generation` is the surrounding radio-service generation and
/// `channel_epoch` is the monotonically increasing dwell identity published
/// by the monitor capture owner.  Carrying both values prevents a retained
/// capture from authorizing transmission after either a hop or a complete
/// monitor restart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorInjectionChannelBinding {
    generation: u32,
    channel_epoch: NonZeroU32,
    channel: WifiChannel,
}

impl MonitorInjectionChannelBinding {
    /// Describe the dwell expected by an application request.
    ///
    /// Constructing this copyable value grants no authority. Admission
    /// compares it with the independently published binding owned by the live
    /// monitor task/sink.
    pub const fn new(
        generation: u32,
        channel_epoch: u32,
        channel: WifiChannel,
    ) -> Result<Self, MonitorInjectionBindingError> {
        match NonZeroU32::new(channel_epoch) {
            Some(channel_epoch) => Ok(Self {
                generation,
                channel_epoch,
                channel,
            }),
            None => Err(MonitorInjectionBindingError::ZeroChannelEpoch),
        }
    }

    pub const fn generation(self) -> u32 {
        self.generation
    }

    pub const fn channel_epoch(self) -> u32 {
        self.channel_epoch.get()
    }

    pub const fn channel(self) -> WifiChannel {
        self.channel
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorInjectionBindingError {
    ZeroChannelEpoch,
}

impl fmt::Display for MonitorInjectionBindingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a monitor injection channel epoch must be nonzero")
    }
}

impl core::error::Error for MonitorInjectionBindingError {}

/// Standard legacy rate accepted by the portable monitor-injection boundary.
///
/// There is deliberately no raw rate code, HT/HE or long-range variant.  A
/// backend must add a separately reviewed typed profile before any of those
/// PHY geometries can reach its queue-vector or PLCP owner.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MonitorInjectionRate {
    #[default]
    Dsss1MLong,
    Dsss2MLong,
    Cck5M5Long,
    Cck11MLong,
    Dsss2MShort,
    Cck5M5Short,
    Cck11MShort,
    Ofdm6M,
    Ofdm9M,
    Ofdm12M,
    Ofdm18M,
    Ofdm24M,
    Ofdm36M,
    Ofdm48M,
    Ofdm54M,
}

/// Supported header geometry of one source-supplied monitor MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorInjectionFrameType {
    Management,
    Data,
    NullData,
}

/// A header-geometry-checked, unprotected IEEE 802.11 MPDU without an FCS.
///
/// The constructor name makes FCS ownership explicit: the caller supplies
/// bytes ending at the MPDU body, while a concrete backend must append or ask
/// hardware to append the FCS exactly once.  The source cannot infer whether
/// four arbitrary trailing bytes were intended as an FCS, so that part of the
/// contract is represented by this type rather than guessed from contents.
/// Management bodies remain opaque raw-policy bytes; this type does not claim
/// that subtype-specific information elements or action contents are valid.
///
/// The initial implementation intentionally admits only ordinary 24-byte
/// management headers, non-QoS three-address data and exact Null Data.  It
/// rejects protected, fragmented, ordered, four-address, QoS, control and
/// extension frames before a sequence allocator, key owner or DMA buffer can
/// be borrowed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorInjectionMpdu<'frame> {
    bytes: &'frame [u8],
    frame_type: MonitorInjectionFrameType,
}

impl<'frame> MonitorInjectionMpdu<'frame> {
    pub fn from_no_fcs(bytes: &'frame [u8]) -> Result<Self, MonitorInjectionFrameError> {
        const BASE_HEADER_LENGTH: usize = 24;
        const PROTOCOL_VERSION_MASK: u16 = 0x0003;
        const FRAME_TYPE_MASK: u16 = 0x000c;
        const MANAGEMENT_FRAME: u16 = 0x0000;
        const DATA_FRAME: u16 = 0x0008;
        const TO_DS: u16 = 0x0100;
        const FROM_DS: u16 = 0x0200;
        const MORE_FRAGMENTS: u16 = 0x0400;
        const PROTECTED: u16 = 0x4000;
        const ORDERED: u16 = 0x8000;

        if bytes.len() < BASE_HEADER_LENGTH {
            return Err(MonitorInjectionFrameError::TooShort {
                required: BASE_HEADER_LENGTH,
                available: bytes.len(),
            });
        }
        let frame_control = u16::from_le_bytes([bytes[0], bytes[1]]);
        if frame_control & PROTOCOL_VERSION_MASK != 0 {
            return Err(MonitorInjectionFrameError::UnsupportedProtocolVersion(
                (frame_control & PROTOCOL_VERSION_MASK) as u8,
            ));
        }
        if frame_control & PROTECTED != 0 {
            return Err(MonitorInjectionFrameError::ProtectedFrame);
        }
        if frame_control & MORE_FRAGMENTS != 0
            || u16::from_le_bytes([bytes[22], bytes[23]]) & 0x000f != 0
        {
            return Err(MonitorInjectionFrameError::FragmentedFrame);
        }
        if frame_control & ORDERED != 0 {
            return Err(MonitorInjectionFrameError::OrderedFrame);
        }

        let frame_type = match frame_control & FRAME_TYPE_MASK {
            MANAGEMENT_FRAME => {
                if frame_control & (TO_DS | FROM_DS) != 0 {
                    return Err(MonitorInjectionFrameError::UnsupportedManagementFlags);
                }
                MonitorInjectionFrameType::Management
            }
            DATA_FRAME => {
                if frame_control & (TO_DS | FROM_DS) == (TO_DS | FROM_DS) {
                    return Err(MonitorInjectionFrameError::FourAddressData);
                }
                let subtype = ((frame_control >> 4) & 0x0f) as u8;
                match subtype {
                    0 => MonitorInjectionFrameType::Data,
                    4 if bytes.len() == BASE_HEADER_LENGTH => MonitorInjectionFrameType::NullData,
                    4 => {
                        return Err(MonitorInjectionFrameError::InvalidNullDataLength {
                            expected: BASE_HEADER_LENGTH,
                            actual: bytes.len(),
                        });
                    }
                    8..=15 => return Err(MonitorInjectionFrameError::QosData),
                    _ => return Err(MonitorInjectionFrameError::UnsupportedDataSubtype(subtype)),
                }
            }
            raw => {
                return Err(MonitorInjectionFrameError::UnsupportedFrameType(
                    ((raw >> 2) & 0x03) as u8,
                ));
            }
        };
        Ok(Self { bytes, frame_type })
    }

    pub const fn bytes(self) -> &'frame [u8] {
        self.bytes
    }

    pub const fn len(self) -> usize {
        self.bytes.len()
    }

    pub const fn is_empty(self) -> bool {
        self.bytes.is_empty()
    }

    pub const fn frame_type(self) -> MonitorInjectionFrameType {
        self.frame_type
    }

    pub const fn is_group_addressed(self) -> bool {
        self.bytes[4] & 1 != 0
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorInjectionFrameError {
    TooShort { required: usize, available: usize },
    UnsupportedProtocolVersion(u8),
    ProtectedFrame,
    FragmentedFrame,
    OrderedFrame,
    UnsupportedManagementFlags,
    FourAddressData,
    QosData,
    UnsupportedDataSubtype(u8),
    InvalidNullDataLength { expected: usize, actual: usize },
    UnsupportedFrameType(u8),
}

impl fmt::Display for MonitorInjectionFrameError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooShort {
                required,
                available,
            } => write!(
                formatter,
                "monitor injection requires at least {required} no-FCS MPDU bytes, got {available}"
            ),
            Self::UnsupportedProtocolVersion(version) => write!(
                formatter,
                "monitor injection does not support 802.11 protocol version {version}"
            ),
            Self::ProtectedFrame => {
                formatter.write_str("monitor injection does not own a protected/keyed TX path")
            }
            Self::FragmentedFrame => {
                formatter.write_str("monitor injection does not own fragment sequencing")
            }
            Self::OrderedFrame => {
                formatter.write_str("monitor injection does not own ordered/HT-Control geometry")
            }
            Self::UnsupportedManagementFlags => formatter
                .write_str("monitor management injection requires both DS direction bits clear"),
            Self::FourAddressData => {
                formatter.write_str("monitor injection does not own four-address data geometry")
            }
            Self::QosData => {
                formatter.write_str("monitor injection does not own QoS data geometry")
            }
            Self::UnsupportedDataSubtype(subtype) => write!(
                formatter,
                "monitor injection does not support data subtype {subtype}"
            ),
            Self::InvalidNullDataLength { expected, actual } => write!(
                formatter,
                "monitor Null Data injection requires exactly {expected} bytes, got {actual}"
            ),
            Self::UnsupportedFrameType(frame_type) => write!(
                formatter,
                "monitor injection does not support frame type {frame_type}"
            ),
        }
    }
}

impl core::error::Error for MonitorInjectionFrameError {}

/// Complete borrowed request for one exact standalone-monitor dwell.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorInjectionRequest<'frame> {
    binding: MonitorInjectionChannelBinding,
    rate: MonitorInjectionRate,
    mpdu: MonitorInjectionMpdu<'frame>,
}

impl<'frame> MonitorInjectionRequest<'frame> {
    pub const fn new(
        binding: MonitorInjectionChannelBinding,
        rate: MonitorInjectionRate,
        mpdu: MonitorInjectionMpdu<'frame>,
    ) -> Self {
        Self {
            binding,
            rate,
            mpdu,
        }
    }

    pub const fn binding(self) -> MonitorInjectionChannelBinding {
        self.binding
    }

    pub const fn rate(self) -> MonitorInjectionRate {
        self.rate
    }

    pub const fn mpdu(self) -> MonitorInjectionMpdu<'frame> {
        self.mpdu
    }
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
        let Some(frame_type) = frame_type(frame.bytes) else {
            return false;
        };
        if !self.frame_types.contains(frame_type) {
            return false;
        }
        if let Some(minimum) = self.minimum_rssi_dbm {
            let observed = match frame.metadata.rssi_dbm {
                MacRxEvidence::HardwareObserved(value)
                | MacRxEvidence::ProtocolValidated(value) => value,
                MacRxEvidence::Unavailable => return false,
            };
            if observed < minimum {
                return false;
            }
        }
        self.any_address
            .is_none_or(|address| frame_contains_address(frame.bytes, frame_type, address))
    }
}

impl Default for MonitorFilter {
    fn default() -> Self {
        Self::all()
    }
}

fn frame_type(bytes: &[u8]) -> Option<MonitorFrameType> {
    let frame_control = u16::from_le_bytes([*bytes.first()?, *bytes.get(1)?]);
    Some(match (frame_control >> 2) & 0x03 {
        0 => MonitorFrameType::Management,
        1 => MonitorFrameType::Control,
        2 => MonitorFrameType::Data,
        _ => MonitorFrameType::Extension,
    })
}

fn frame_contains_address(bytes: &[u8], frame_type: MonitorFrameType, address: [u8; 6]) -> bool {
    let address_at = |offset: usize| {
        bytes
            .get(offset..offset + 6)
            .is_some_and(|candidate| candidate == address)
    };
    match frame_type {
        MonitorFrameType::Management => {
            bytes.len() >= 24 && (address_at(4) || address_at(10) || address_at(16))
        }
        MonitorFrameType::Control => {
            let subtype = bytes.first().map_or(0, |value| value >> 4);
            let has_transmitter = matches!(subtype, 8 | 9 | 10 | 11 | 14 | 15);
            address_at(4) || (has_transmitter && address_at(10))
        }
        MonitorFrameType::Data => {
            if bytes.len() < 24 {
                return false;
            }
            let to_ds_and_from_ds = bytes.get(1).is_some_and(|flags| flags & 0x03 == 0x03);
            address_at(4)
                || address_at(10)
                || address_at(16)
                || (to_ds_and_from_ds && address_at(24))
        }
        MonitorFrameType::Extension => address_at(4),
    }
}

/// One frame observed at an explicitly identified receive-pipeline boundary.
#[derive(Clone, Copy, Debug)]
pub struct MonitorFrame<'frame, Rate> {
    pub tap: MonitorTapPoint,
    pub channel_context: ChannelContextId,
    /// Bytes visible at this tap. They never include a hardware-stripped FCS.
    pub bytes: &'frame [u8],
    pub metadata: MacRxMetadata<Rate>,
    /// Complete logical MPDU length inferred from the receive status.
    ///
    /// `bytes` may be shorter when hardware consumed an authenticated cipher
    /// trailer before publishing the DMA view. The distinction prevents a
    /// capture consumer from treating such a frame as ordinary queue loss.
    pub logical_length: usize,
}

impl<Rate> MonitorFrame<'_, Rate> {
    pub const fn is_complete(&self) -> bool {
        self.bytes.len() == self.logical_length
    }
}

/// Why a best-effort monitor sink did not retain an observation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorDropReason {
    /// Every caller-owned capture slot was occupied.
    Full,
    /// The frame exceeds the sink's configured capture capacity.
    TooLong,
    /// Caller policy intentionally excluded this observation.
    Filtered,
}

/// Immediate result of a non-blocking monitor publication attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorPublishOutcome {
    Published,
    Dropped(MonitorDropReason),
}

/// Best-effort observation endpoint for one receive tap.
///
/// Implementations must return without waiting for capacity. An async adapter
/// can wake a consumer after copying into bounded storage, but the radio owner
/// never awaits that consumer and continues recycling its RX descriptor.
pub trait MonitorSink<Rate> {
    /// Start a fresh physical-channel capture epoch.
    ///
    /// The default preserves source-only sinks which do not retain channel
    /// metadata. Queue-backed product sinks override this edge and stamp the
    /// exact channel into every independently retained frame.
    fn begin_channel_epoch(&mut self, _channel: WifiChannel) {}

    /// Revoke dwell-bound injection authority before stop or retune begins.
    fn end_channel_epoch(&mut self) {}

    /// Exact authority for injection during the currently live dwell.
    ///
    /// Legacy and observation-only sinks return `None`. A production sink
    /// overrides this only after its task owner has bound the supervisor
    /// generation, tuned channel and monotonically increasing capture epoch.
    fn injection_channel_binding(&self) -> Option<MonitorInjectionChannelBinding> {
        None
    }

    fn try_publish(&mut self, frame: MonitorFrame<'_, Rate>) -> MonitorPublishOutcome;
}

#[cfg(test)]
mod tests;
