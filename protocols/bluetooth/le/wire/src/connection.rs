//! Legacy CONNECT_IND fields and validation (Core Vol 6 Part B 2.3.3.1).

use crate::advertising_header::{ADVERTISING_HEADER_LENGTH, AdvertisingHeader, AdvertisingPduType};
use crate::air_time::{LE_1M_MICROS_PER_OCTET, LE_1M_PACKET_OVERHEAD_OCTETS};
use crate::{LeDeviceAddress, LeDeviceAddressKind};
use core::num::{NonZeroU8, NonZeroU64};
use oer_wire::Reader;

pub const LEGACY_CONNECT_IND_PDU_BYTES: usize =
    ADVERTISING_HEADER_LENGTH + LEGACY_CONNECT_IND_PAYLOAD_BYTES;
pub const LEGACY_CONNECT_IND_PAYLOAD_BYTES: usize = 34;
/// Complete on-air duration of a legacy `CONNECT_IND` on LE 1M.
///
/// This includes the one-byte preamble, four-byte Access Address, complete
/// Link Layer PDU and three-byte CRC. The chip backend adds this duration to
/// the observed packet start before applying the transmit-window offset.
pub const LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS: u32 =
    (LE_1M_PACKET_OVERHEAD_OCTETS + LEGACY_CONNECT_IND_PDU_BYTES as u32) * LE_1M_MICROS_PER_OCTET;

const ADVERTISING_ACCESS_ADDRESS: u32 = 0x8e89_bed6;

/// Intrinsically valid Access Address for one LE ACL connection on an uncoded PHY.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeUncodedAccessAddress(u32);

impl LeUncodedAccessAddress {
    /// Validate the bit-pattern requirements that do not depend on other live
    /// connections. Cross-connection uniqueness remains a Controller policy.
    pub const fn new(value: u32) -> Result<Self, LeUncodedAccessAddressError> {
        if (value ^ ADVERTISING_ACCESS_ADDRESS).count_ones() <= 1 {
            return Err(LeUncodedAccessAddressError::AdvertisingAddressOrOneBitAway);
        }
        let bytes = value.to_le_bytes();
        if bytes[0] == bytes[1] && bytes[1] == bytes[2] && bytes[2] == bytes[3] {
            return Err(LeUncodedAccessAddressError::AllOctetsEqual);
        }
        if longest_run(value) > 6 {
            return Err(LeUncodedAccessAddressError::RunTooLong);
        }
        if transition_count(value) > 24 {
            return Err(LeUncodedAccessAddressError::TooManyTransitions);
        }
        if most_significant_six_transition_count(value) < 2 {
            return Err(LeUncodedAccessAddressError::TooFewMostSignificantTransitions);
        }
        Ok(Self(value))
    }

    pub const fn value(self) -> u32 {
        self.0
    }

    /// Fixed channel identifier used by Channel Selection Algorithm #2.
    pub const fn channel_identifier(self) -> u16 {
        (self.0 >> 16) as u16 ^ self.0 as u16
    }
}

const fn longest_run(value: u32) -> u8 {
    let mut longest = 1;
    // At most 32 bits are visited; neither u8 run length nor u32 cursor can
    // reach saturation for any Access Address.
    let mut current = 1_u8;
    let mut previous = value & 1;
    let mut bit = 1_u32;
    while bit < 32 {
        let next = (value >> bit) & 1;
        if next == previous {
            current = current.saturating_add(1);
            if current > longest {
                longest = current;
            }
        } else {
            current = 1;
            previous = next;
        }
        bit = bit.saturating_add(1);
    }
    longest
}

const fn transition_count(value: u32) -> u32 {
    ((value ^ (value >> 1)) & 0x7fff_ffff).count_ones()
}

const fn most_significant_six_transition_count(value: u32) -> u8 {
    let six = value >> 26;
    (((six ^ (six >> 1)) & 0x1f).count_ones()) as u8
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeUncodedAccessAddressError {
    AdvertisingAddressOrOneBitAway,
    AllOctetsEqual,
    RunTooLong,
    TooManyTransitions,
    TooFewMostSignificantTransitions,
}

/// Three-octet CRC initialization value carried by `CONNECT_IND`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeCrcInitialization([u8; 3]);

impl LeCrcInitialization {
    pub const fn from_wire_bytes(bytes: [u8; 3]) -> Self {
        Self(bytes)
    }

    pub const fn wire_bytes(self) -> [u8; 3] {
        self.0
    }

    pub const fn value(self) -> u32 {
        self.0[0] as u32 | (self.0[1] as u32) << 8 | (self.0[2] as u32) << 16
    }
}

/// One of the 37 general-purpose LE data channel indices.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct LeDataChannelIndex(u8);

impl LeDataChannelIndex {
    pub const fn new(index: u8) -> Option<Self> {
        if index < 37 { Some(Self(index)) } else { None }
    }

    /// Reduce a channel-selection result to the 37-channel domain.
    pub const fn wrapping(index: u8) -> Self {
        Self(index % 37)
    }

    pub const fn get(self) -> u8 {
        self.0
    }
}

/// Validated map containing at least two used data channels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeDataChannelMap {
    mask: NonZeroU64,
    used: NonZeroU8,
}

impl LeDataChannelMap {
    pub const fn new(bytes: [u8; 5]) -> Result<Self, LeDataChannelMapError> {
        let [a, b, c, d, e] = bytes;
        if e & 0xe0 != 0 {
            return Err(LeDataChannelMapError::ReservedBitsSet);
        }
        let mask = u64::from_le_bytes([a, b, c, d, e, 0, 0, 0]);
        if mask.count_ones() < 2 {
            return Err(LeDataChannelMapError::FewerThanTwoUsedChannels);
        }
        let (Some(mask), Some(used)) = (
            NonZeroU64::new(mask),
            NonZeroU8::new(mask.count_ones() as u8),
        ) else {
            return Err(LeDataChannelMapError::FewerThanTwoUsedChannels);
        };
        Ok(Self { mask, used })
    }

    pub const fn all() -> Self {
        Self {
            mask: const { NonZeroU64::new((1 << 37) - 1).unwrap() },
            used: const { NonZeroU8::new(37).unwrap() },
        }
    }

    pub const fn wire_bytes(self) -> [u8; 5] {
        let [a, b, c, d, e, _, _, _] = self.mask.get().to_le_bytes();
        [a, b, c, d, e]
    }

    pub const fn used_channel_count(self) -> u8 {
        self.used.get()
    }

    pub const fn contains(self, channel: LeDataChannelIndex) -> bool {
        self.mask.get() & (1 << channel.get()) != 0
    }

    /// Select a rank in the cyclic sequence of used channels. The nonempty
    /// mask makes each step total, including the last-to-first transition.
    pub const fn remap(self, index: u8) -> LeDataChannelIndex {
        let mut channels = self.mask;
        let mut remaining = index;
        while remaining != 0 {
            let rest = channels.get() & channels.get().wrapping_sub(1);
            channels = match NonZeroU64::new(rest) {
                Some(rest) => rest,
                None => self.mask,
            };
            remaining = remaining.saturating_sub(1);
        }
        LeDataChannelIndex(channels.trailing_zeros() as u8)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeDataChannelMapError {
    ReservedBitsSet,
    FewerThanTwoUsedChannels,
}

/// Channel selection negotiated by the advertising header.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeChannelSelectionAlgorithm {
    AlgorithmOne,
    AlgorithmTwo,
}

/// Worst-case sleep-clock accuracy class supplied by the Central.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeSleepClockAccuracy {
    Ppm500,
    Ppm250,
    Ppm150,
    Ppm100,
    Ppm75,
    Ppm50,
    Ppm30,
    Ppm20,
}

impl LeSleepClockAccuracy {
    const fn from_three_bits(bits: u8) -> Self {
        match bits & 7 {
            0 => Self::Ppm500,
            1 => Self::Ppm250,
            2 => Self::Ppm150,
            3 => Self::Ppm100,
            4 => Self::Ppm75,
            5 => Self::Ppm50,
            6 => Self::Ppm30,
            _ => Self::Ppm20,
        }
    }
    pub const fn encoded(self) -> u8 {
        match self {
            Self::Ppm500 => 0,
            Self::Ppm250 => 1,
            Self::Ppm150 => 2,
            Self::Ppm100 => 3,
            Self::Ppm75 => 4,
            Self::Ppm50 => 5,
            Self::Ppm30 => 6,
            Self::Ppm20 => 7,
        }
    }
    /// Worst-case Central sleep-clock error represented by this SCA class.
    pub const fn worst_case_ppm(self) -> u16 {
        match self {
            Self::Ppm500 => 500,
            Self::Ppm250 => 250,
            Self::Ppm150 => 150,
            Self::Ppm100 => 100,
            Self::Ppm75 => 75,
            Self::Ppm50 => 50,
            Self::Ppm30 => 30,
            Self::Ppm20 => 20,
        }
    }
}

/// Validated first transmit window and recurring connection timing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeConnectionTiming {
    window_size: u8,
    window_offset: u16,
    interval: u16,
    peripheral_latency: u16,
    supervision_timeout: u16,
}

impl LeConnectionTiming {
    pub const fn new(
        window_size: u8,
        window_offset: u16,
        interval: u16,
        peripheral_latency: u16,
        supervision_timeout: u16,
    ) -> Result<Self, LeConnectionTimingError> {
        if interval < 6 || interval > 3200 {
            return Err(LeConnectionTimingError::IntervalOutsideRange);
        }
        if window_size == 0 || window_size > 8 || window_size as u16 >= interval {
            return Err(LeConnectionTimingError::WindowSizeOutsideRange);
        }
        if window_offset > interval {
            return Err(LeConnectionTimingError::WindowOffsetOutsideRange);
        }
        if peripheral_latency > 499 {
            return Err(LeConnectionTimingError::PeripheralLatencyOutsideRange);
        }
        if supervision_timeout < 10 || supervision_timeout > 3200 {
            return Err(LeConnectionTimingError::SupervisionTimeoutOutsideRange);
        }
        // The admitted ranges bound the products to 12,800 and 1,600,000.
        // Saturating operations are exact here; invalid inputs returned above.
        if (supervision_timeout as u32).saturating_mul(4)
            <= (interval as u32).saturating_mul((peripheral_latency as u32).saturating_add(1))
        {
            return Err(LeConnectionTimingError::SupervisionTimeoutTooShort);
        }
        Ok(Self {
            window_size,
            window_offset,
            interval,
            peripheral_latency,
            supervision_timeout,
        })
    }

    pub const fn window_size_units(self) -> u8 {
        self.window_size
    }

    pub const fn window_offset_units(self) -> u16 {
        self.window_offset
    }

    pub const fn interval_units(self) -> u16 {
        self.interval
    }

    // Validated protocol bounds keep these widened calculations exact.
    pub const fn interval_micros(self) -> u32 {
        (self.interval as u32).saturating_mul(1_250)
    }

    pub const fn peripheral_latency(self) -> u16 {
        self.peripheral_latency
    }

    pub const fn supervision_timeout_units(self) -> u16 {
        self.supervision_timeout
    }

    pub const fn supervision_timeout_micros(self) -> u32 {
        (self.supervision_timeout as u32).saturating_mul(10_000)
    }

    /// Earliest first anchor relative to the end of the `CONNECT_IND` packet.
    pub const fn first_window_start_micros(self) -> u32 {
        1_250_u32.saturating_add((self.window_offset as u32).saturating_mul(1_250))
    }

    /// Exclusive upper edge of the first transmit window relative to the end
    /// of the `CONNECT_IND` packet.
    pub const fn first_window_end_micros(self) -> u32 {
        self.first_window_start_micros()
            .saturating_add((self.window_size as u32).saturating_mul(1_250))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeConnectionTimingError {
    IntervalOutsideRange,
    WindowSizeOutsideRange,
    WindowOffsetOutsideRange,
    PeripheralLatencyOutsideRange,
    SupervisionTimeoutOutsideRange,
    SupervisionTimeoutTooShort,
}

/// Complete semantic value carried by one legacy `CONNECT_IND` PDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeLegacyConnectionRequest {
    initiator: LeDeviceAddress,
    advertiser: LeDeviceAddress,
    access_address: LeUncodedAccessAddress,
    crc_initialization: LeCrcInitialization,
    timing: LeConnectionTiming,
    channel_map: LeDataChannelMap,
    hop_increment: u8,
    sleep_clock_accuracy: LeSleepClockAccuracy,
    channel_selection: LeChannelSelectionAlgorithm,
}

impl LeLegacyConnectionRequest {
    /// Decode and validate exactly one complete legacy `CONNECT_IND` PDU.
    pub const fn decode(pdu: &[u8]) -> Result<Self, LeLegacyConnectionRequestError> {
        let mut reader = Reader::new(pdu);
        let header = match reader.read::<ADVERTISING_HEADER_LENGTH>() {
            Ok(bytes) => AdvertisingHeader::from_bytes(bytes),
            Err(_) => return Err(LeLegacyConnectionRequestError::HeaderTruncated),
        };
        if !matches!(header.pdu_type(), AdvertisingPduType::ConnectInd) {
            return Err(LeLegacyConnectionRequestError::UnexpectedPduType);
        }
        if header.reserved() || header.legacy_length_reserved() != 0 {
            return Err(LeLegacyConnectionRequestError::ReservedHeaderBitsSet);
        }
        let declared = header.legacy_length() as usize;
        if declared != LEGACY_CONNECT_IND_PAYLOAD_BYTES || pdu.len() != LEGACY_CONNECT_IND_PDU_BYTES
        {
            return Err(LeLegacyConnectionRequestError::LengthMismatch);
        }

        let Some((pdu, _)) = pdu.split_first_chunk::<LEGACY_CONNECT_IND_PDU_BYTES>() else {
            return Err(LeLegacyConnectionRequestError::LengthMismatch);
        };

        let initiator = LeDeviceAddress::from_wire_bytes(
            [pdu[2], pdu[3], pdu[4], pdu[5], pdu[6], pdu[7]],
            if !header.tx_add_random() {
                LeDeviceAddressKind::Public
            } else {
                LeDeviceAddressKind::Random
            },
        );
        let advertiser = LeDeviceAddress::from_wire_bytes(
            [pdu[8], pdu[9], pdu[10], pdu[11], pdu[12], pdu[13]],
            if !header.rx_add_random() {
                LeDeviceAddressKind::Public
            } else {
                LeDeviceAddressKind::Random
            },
        );
        let access_address = match LeUncodedAccessAddress::new(u32::from_le_bytes([
            pdu[14], pdu[15], pdu[16], pdu[17],
        ])) {
            Ok(value) => value,
            Err(error) => return Err(LeLegacyConnectionRequestError::AccessAddress(error)),
        };
        let crc_initialization = LeCrcInitialization::from_wire_bytes([pdu[18], pdu[19], pdu[20]]);
        let timing = match LeConnectionTiming::new(
            pdu[21],
            u16::from_le_bytes([pdu[22], pdu[23]]),
            u16::from_le_bytes([pdu[24], pdu[25]]),
            u16::from_le_bytes([pdu[26], pdu[27]]),
            u16::from_le_bytes([pdu[28], pdu[29]]),
        ) {
            Ok(value) => value,
            Err(error) => return Err(LeLegacyConnectionRequestError::Timing(error)),
        };
        let channel_map = match LeDataChannelMap::new([pdu[30], pdu[31], pdu[32], pdu[33], pdu[34]])
        {
            Ok(value) => value,
            Err(error) => return Err(LeLegacyConnectionRequestError::ChannelMap(error)),
        };
        let hop_increment = pdu[35] & 0x1f;
        if hop_increment < 5 || hop_increment > 16 {
            return Err(LeLegacyConnectionRequestError::HopIncrementOutsideRange);
        }
        Ok(Self {
            initiator,
            advertiser,
            access_address,
            crc_initialization,
            timing,
            channel_map,
            hop_increment,
            sleep_clock_accuracy: LeSleepClockAccuracy::from_three_bits(pdu[35] >> 5),
            channel_selection: if !header.channel_selection_two() {
                LeChannelSelectionAlgorithm::AlgorithmOne
            } else {
                LeChannelSelectionAlgorithm::AlgorithmTwo
            },
        })
    }

    pub const fn initiator(self) -> LeDeviceAddress {
        self.initiator
    }

    pub const fn advertiser(self) -> LeDeviceAddress {
        self.advertiser
    }

    pub fn is_addressed_to(self, address: LeDeviceAddress) -> bool {
        self.advertiser == address
    }

    pub const fn access_address(self) -> LeUncodedAccessAddress {
        self.access_address
    }

    pub const fn crc_initialization(self) -> LeCrcInitialization {
        self.crc_initialization
    }

    pub const fn timing(self) -> LeConnectionTiming {
        self.timing
    }

    pub const fn channel_map(self) -> LeDataChannelMap {
        self.channel_map
    }

    pub const fn hop_increment(self) -> u8 {
        self.hop_increment
    }

    pub const fn sleep_clock_accuracy(self) -> LeSleepClockAccuracy {
        self.sleep_clock_accuracy
    }

    /// Algorithm indicated by the initiator's ChSel bit, before negotiation
    /// with the advertiser's ChSel bit.
    pub const fn channel_selection(self) -> LeChannelSelectionAlgorithm {
        self.channel_selection
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeLegacyConnectionRequestError {
    HeaderTruncated,
    UnexpectedPduType,
    ReservedHeaderBitsSet,
    LengthMismatch,
    AccessAddress(LeUncodedAccessAddressError),
    Timing(LeConnectionTimingError),
    ChannelMap(LeDataChannelMapError),
    HopIncrementOutsideRange,
}
impl LeLegacyConnectionRequest {
    /// The same wire fields with the admitted channel map update applied.
    pub const fn with_channel_map(mut self, channel_map: LeDataChannelMap) -> Self {
        self.channel_map = channel_map;
        self
    }
    /// The same wire fields with the admitted timing update applied.
    pub const fn with_timing(mut self, timing: LeConnectionTiming) -> Self {
        self.timing = timing;
        self
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
mod tests;
