//! Portable LE peripheral-connection admission and event-channel planning.
//!
//! This module owns Bluetooth air-interface semantics only. It parses one
//! legacy `CONNECT_IND`, validates the complete first-event timing profile and
//! advances Channel Selection Algorithm #1 or #2 without MMIO, controller SRAM,
//! HCI, an executor or an allocator. A chip backend must retain the affine
//! event owner with its own hardware ticket before reporting completion.
//!
//! [`acknowledgement`] owns the data PDU header's SN, NESN and MD fields and
//! the acknowledgement and flow control of a backend that runs them in
//! software; [`maintenance`] reads received headers through it.

use core::num::NonZeroU16;

use crate::{LeDeviceAddress, LeDeviceAddressKind};

pub const LEGACY_CONNECT_IND_PDU_BYTES: usize = 36;
pub const LEGACY_CONNECT_IND_PAYLOAD_BYTES: usize = 34;
/// Complete on-air duration of a legacy `CONNECT_IND` on LE 1M.
///
/// This includes the one-byte preamble, four-byte Access Address, complete
/// Link Layer PDU and three-byte CRC. The chip backend adds this duration to
/// the observed packet start before applying the transmit-window offset.
pub const LEGACY_CONNECT_IND_LE_1M_AIRTIME_MICROS: u32 =
    (1 + 4 + LEGACY_CONNECT_IND_PDU_BYTES as u32 + 3) * 8;

const CONNECT_IND_TYPE: u8 = 0b0101;
const PDU_TYPE_MASK: u8 = 0x0f;
const HEADER_RESERVED: u8 = 1 << 4;
const CHANNEL_SELECTION_TWO: u8 = 1 << 5;
const TX_ADD_RANDOM: u8 = 1 << 6;
const RX_ADD_RANDOM: u8 = 1 << 7;
const PAYLOAD_LENGTH_MASK: u8 = 0x3f;
const PAYLOAD_LENGTH_RESERVED: u8 = 0xc0;
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
    let mut current = 1;
    let mut previous = value & 1;
    let mut bit = 1;
    while bit < 32 {
        let next = (value >> bit) & 1;
        if next == previous {
            current += 1;
            if current > longest {
                longest = current;
            }
        } else {
            current = 1;
            previous = next;
        }
        bit += 1;
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

    pub const fn get(self) -> u8 {
        self.0
    }
}

/// Validated map containing at least two used data channels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeDataChannelMap {
    bytes: [u8; 5],
    used: u8,
}

impl LeDataChannelMap {
    pub const fn new(bytes: [u8; 5]) -> Result<Self, LeDataChannelMapError> {
        if bytes[4] & 0xe0 != 0 {
            return Err(LeDataChannelMapError::ReservedBitsSet);
        }
        let used = bytes[0].count_ones()
            + bytes[1].count_ones()
            + bytes[2].count_ones()
            + bytes[3].count_ones()
            + bytes[4].count_ones();
        if used < 2 {
            return Err(LeDataChannelMapError::FewerThanTwoUsedChannels);
        }
        Ok(Self {
            bytes,
            used: used as u8,
        })
    }

    pub const fn all() -> Self {
        Self {
            bytes: [0xff, 0xff, 0xff, 0xff, 0x1f],
            used: 37,
        }
    }

    pub const fn wire_bytes(self) -> [u8; 5] {
        self.bytes
    }

    pub const fn used_channel_count(self) -> u8 {
        self.used
    }

    pub const fn contains(self, channel: LeDataChannelIndex) -> bool {
        let index = channel.get() as usize;
        self.bytes[index / 8] & (1 << (index % 8)) != 0
    }

    const fn remap(self, index: u8) -> LeDataChannelIndex {
        let mut remaining = index;
        let mut channel = 0;
        while channel < 37 {
            let candidate = LeDataChannelIndex(channel);
            if self.contains(candidate) {
                if remaining == 0 {
                    return candidate;
                }
                remaining -= 1;
            }
            channel += 1;
        }
        unreachable!()
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
pub struct LeSleepClockAccuracy(u8);

impl LeSleepClockAccuracy {
    pub const fn encoded(self) -> u8 {
        self.0
    }

    /// Worst-case Central sleep-clock error represented by this SCA class.
    pub const fn worst_case_ppm(self) -> u16 {
        match self.0 {
            0 => 500,
            1 => 250,
            2 => 150,
            3 => 100,
            4 => 75,
            5 => 50,
            6 => 30,
            7 => 20,
            _ => unreachable!(),
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
        if supervision_timeout as u32 * 4 <= interval as u32 * (peripheral_latency as u32 + 1) {
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

    pub const fn interval_micros(self) -> u32 {
        self.interval as u32 * 1_250
    }

    pub const fn peripheral_latency(self) -> u16 {
        self.peripheral_latency
    }

    pub const fn supervision_timeout_units(self) -> u16 {
        self.supervision_timeout
    }

    pub const fn supervision_timeout_micros(self) -> u32 {
        self.supervision_timeout as u32 * 10_000
    }

    /// Earliest first anchor relative to the end of the `CONNECT_IND` packet.
    pub const fn first_window_start_micros(self) -> u32 {
        1_250 + self.window_offset as u32 * 1_250
    }

    /// Exclusive upper edge of the first transmit window relative to the end
    /// of the `CONNECT_IND` packet.
    pub const fn first_window_end_micros(self) -> u32 {
        self.first_window_start_micros() + self.window_size as u32 * 1_250
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

/// Timing change applied by a Central Connection Update at one exact instant.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeConnectionTimingTransition {
    previous: LeConnectionTiming,
    updated: LeConnectionTiming,
    instant: u16,
}

impl LeConnectionTimingTransition {
    pub const fn previous(self) -> LeConnectionTiming {
        self.previous
    }

    pub const fn updated(self) -> LeConnectionTiming {
        self.updated
    }

    pub const fn instant(self) -> u16 {
        self.instant
    }

    /// Whether the three Host-visible connection parameters changed.
    pub const fn host_parameters_changed(self) -> bool {
        self.previous.interval != self.updated.interval
            || self.previous.peripheral_latency != self.updated.peripheral_latency
            || self.previous.supervision_timeout != self.updated.supervision_timeout
    }
}

/// How the event one interval after the last one follows it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeFollowingEvent {
    /// One interval of the timing in force.
    Unchanged(LeConnectionTiming),
    /// The event is a Connection Update's instant: one interval of the
    /// previous timing, then the updated window offset, listening across the
    /// updated transmit window.
    Instant {
        /// The timing before the instant.
        previous: LeConnectionTiming,
        /// The timing from the instant on.
        updated: LeConnectionTiming,
    },
}

/// How the intervals of one recurring step divide around a connection
/// update's instant: `before` intervals of the previous timing reach the
/// instant, then `after` intervals of the updated timing follow, together
/// the step's event distance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeConnectionTimingIntervals {
    /// Intervals of the previous timing, the last one ending at the instant.
    pub before: u16,
    /// Intervals of the updated timing after the instant.
    pub after: u16,
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
        if pdu.len() < 2 {
            return Err(LeLegacyConnectionRequestError::HeaderTruncated);
        }
        if pdu[0] & PDU_TYPE_MASK != CONNECT_IND_TYPE {
            return Err(LeLegacyConnectionRequestError::UnexpectedPduType);
        }
        if pdu[0] & HEADER_RESERVED != 0 || pdu[1] & PAYLOAD_LENGTH_RESERVED != 0 {
            return Err(LeLegacyConnectionRequestError::ReservedHeaderBitsSet);
        }
        let declared = (pdu[1] & PAYLOAD_LENGTH_MASK) as usize;
        if declared != LEGACY_CONNECT_IND_PAYLOAD_BYTES || pdu.len() != LEGACY_CONNECT_IND_PDU_BYTES
        {
            return Err(LeLegacyConnectionRequestError::LengthMismatch);
        }

        let initiator = LeDeviceAddress::from_wire_bytes(
            copy_six(pdu, 2),
            if pdu[0] & TX_ADD_RANDOM == 0 {
                LeDeviceAddressKind::Public
            } else {
                LeDeviceAddressKind::Random
            },
        );
        let advertiser = LeDeviceAddress::from_wire_bytes(
            copy_six(pdu, 8),
            if pdu[0] & RX_ADD_RANDOM == 0 {
                LeDeviceAddressKind::Public
            } else {
                LeDeviceAddressKind::Random
            },
        );
        let access_address =
            match LeUncodedAccessAddress::new(u32::from_le_bytes(copy_four(pdu, 14))) {
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
            sleep_clock_accuracy: LeSleepClockAccuracy(pdu[35] >> 5),
            channel_selection: if pdu[0] & CHANNEL_SELECTION_TWO == 0 {
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

const fn copy_four(source: &[u8], start: usize) -> [u8; 4] {
    [
        source[start],
        source[start + 1],
        source[start + 2],
        source[start + 3],
    ]
}

const fn copy_six(source: &[u8], start: usize) -> [u8; 6] {
    [
        source[start],
        source[start + 1],
        source[start + 2],
        source[start + 3],
        source[start + 4],
        source[start + 5],
    ]
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

/// Pure Channel Selection Algorithm #2 event selector.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeChannelSelectionAlgorithmTwo {
    channel_identifier: u16,
    channel_map: LeDataChannelMap,
}

impl LeChannelSelectionAlgorithmTwo {
    pub const fn new(
        access_address: LeUncodedAccessAddress,
        channel_map: LeDataChannelMap,
    ) -> Self {
        Self {
            channel_identifier: access_address.channel_identifier(),
            channel_map,
        }
    }

    pub const fn select(self, event_counter: u16) -> LeDataChannelIndex {
        let mut pseudo_random = event_counter ^ self.channel_identifier;
        let mut round = 0;
        while round < 3 {
            pseudo_random = permute(pseudo_random);
            pseudo_random = pseudo_random
                .wrapping_mul(17)
                .wrapping_add(self.channel_identifier);
            round += 1;
        }
        pseudo_random ^= self.channel_identifier;
        let unmapped = LeDataChannelIndex((pseudo_random % 37) as u8);
        if self.channel_map.contains(unmapped) {
            unmapped
        } else {
            let remapping_index =
                ((self.channel_map.used_channel_count() as u32 * pseudo_random as u32) >> 16) as u8;
            self.channel_map.remap(remapping_index)
        }
    }

    const fn with_channel_map(self, channel_map: LeDataChannelMap) -> Self {
        Self {
            channel_identifier: self.channel_identifier,
            channel_map,
        }
    }
}

const fn permute(value: u16) -> u16 {
    let bytes = value.to_le_bytes();
    u16::from_le_bytes([bytes[0].reverse_bits(), bytes[1].reverse_bits()])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ConnectionChannelSelector {
    One {
        channel_map: LeDataChannelMap,
        hop_increment: u8,
        last_unmapped: u8,
    },
    Two(LeChannelSelectionAlgorithmTwo),
}

impl ConnectionChannelSelector {
    const fn with_channel_map(self, channel_map: LeDataChannelMap) -> Self {
        match self {
            Self::One {
                hop_increment,
                last_unmapped,
                ..
            } => Self::One {
                channel_map,
                hop_increment,
                last_unmapped,
            },
            Self::Two(selector) => Self::Two(selector.with_channel_map(channel_map)),
        }
    }

    const fn preview(self, event_counter: u16) -> (LeDataChannelIndex, u8) {
        match self {
            Self::One {
                channel_map,
                hop_increment,
                last_unmapped,
            } => {
                let unmapped = (last_unmapped + hop_increment) % 37;
                let candidate = LeDataChannelIndex(unmapped);
                let selected = if channel_map.contains(candidate) {
                    candidate
                } else {
                    channel_map.remap(unmapped % channel_map.used_channel_count())
                };
                (selected, unmapped)
            }
            Self::Two(selector) => (selector.select(event_counter), 0),
        }
    }

    const fn complete(self, unmapped: u8) -> Self {
        match self {
            Self::One {
                channel_map,
                hop_increment,
                ..
            } => Self::One {
                channel_map,
                hop_increment,
                last_unmapped: unmapped,
            },
            Self::Two(selector) => Self::Two(selector),
        }
    }

    /// Advance past events which will not be submitted to the radio.
    const fn skip(self, event_count: u16) -> Self {
        match self {
            Self::One {
                channel_map,
                hop_increment,
                last_unmapped,
            } => {
                let hop_distance = (hop_increment as u16 * (event_count % 37)) % 37;
                Self::One {
                    channel_map,
                    hop_increment,
                    last_unmapped: ((last_unmapped as u16 + hop_distance) % 37) as u8,
                }
            }
            Self::Two(selector) => Self::Two(selector),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingChannelMapUpdate {
    channel_map: LeDataChannelMap,
    instant: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingConnectionUpdate {
    timing: LeConnectionTiming,
    instant: u16,
}

/// Why a peer Channel Map Update cannot enter the pending instant owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LePeripheralChannelMapUpdateError {
    InstantPassed,
    ProcedureAlreadyPending,
    IncompatibleProcedurePending,
}

/// Why a peer Connection Update cannot enter the pending instant owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LePeripheralConnectionUpdateError {
    InstantPassed,
    ProcedureAlreadyPending,
    IncompatibleProcedurePending,
}

/// Portable connection-establishment state retained between radio events.
///
/// Creation begins when the validated `CONNECT_IND` is accepted. The
/// connection becomes established only after one event observes peer
/// activity. The first such event remains distinct from the later activity
/// anchor used by supervision policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LePeripheralConnectionState {
    Created,
    Established {
        establishment_event_counter: u16,
        supervision_anchor_event_counter: u16,
    },
}

impl LePeripheralConnectionState {
    pub const fn establishment_event_counter(self) -> Option<u16> {
        match self {
            Self::Created => None,
            Self::Established {
                establishment_event_counter,
                ..
            } => Some(establishment_event_counter),
        }
    }

    pub const fn supervision_anchor_event_counter(self) -> Option<u16> {
        match self {
            Self::Created => None,
            Self::Established {
                supervision_anchor_event_counter,
                ..
            } => Some(supervision_anchor_event_counter),
        }
    }
}

/// Whether the completed connection event observed activity from the peer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LePeripheralConnectionEventPeerActivity {
    Observed,
    Missed,
}

/// Nonzero distance from one completed event to a recurring event.
///
/// A value of one means the immediately following event. Constructing the
/// distance from a skipped-event count makes the scheduler relation explicit:
/// `delta = skipped + 1`.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct LePeripheralConnectionEventDelta(NonZeroU16);

impl LePeripheralConnectionEventDelta {
    pub const fn new(delta: u16) -> Option<Self> {
        match NonZeroU16::new(delta) {
            Some(delta) => Some(Self(delta)),
            None => None,
        }
    }

    /// Form an event distance from the number of intervening events skipped.
    ///
    /// Returns `None` when `skipped + 1` is not representable as a `u16`.
    pub const fn from_skipped(skipped: u16) -> Option<Self> {
        match skipped.checked_add(1) {
            Some(delta) => Self::new(delta),
            None => None,
        }
    }

    pub const fn get(self) -> u16 {
        self.0.get()
    }

    pub const fn skipped(self) -> u16 {
        self.get() - 1
    }
}

/// Portable peripheral connection between hardware events.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "prepare the next event or retain the connection"]
pub struct LePeripheralConnection {
    request: LeLegacyConnectionRequest,
    event_counter: u16,
    initial_transmit_window_pending: bool,
    selector: ConnectionChannelSelector,
    state: LePeripheralConnectionState,
    pending_channel_map: Option<PendingChannelMapUpdate>,
    pending_connection_update: Option<PendingConnectionUpdate>,
    maintenance: maintenance::State,
}

impl LePeripheralConnection {
    /// Negotiate the legacy connection using both devices' ChSel fields.
    /// `advertised_algorithm` describes the exact ADV_IND/ADV_DIRECT_IND sent.
    /// Core Vol 6, Part B, 4.5.8 requires algorithm one if either bit was zero.
    pub const fn from_request(
        request: LeLegacyConnectionRequest,
        advertised_algorithm: LeChannelSelectionAlgorithm,
    ) -> Self {
        let algorithm = match (advertised_algorithm, request.channel_selection) {
            (
                LeChannelSelectionAlgorithm::AlgorithmTwo,
                LeChannelSelectionAlgorithm::AlgorithmTwo,
            ) => LeChannelSelectionAlgorithm::AlgorithmTwo,
            _ => LeChannelSelectionAlgorithm::AlgorithmOne,
        };
        let selector = match algorithm {
            LeChannelSelectionAlgorithm::AlgorithmOne => ConnectionChannelSelector::One {
                channel_map: request.channel_map,
                hop_increment: request.hop_increment,
                last_unmapped: 0,
            },
            LeChannelSelectionAlgorithm::AlgorithmTwo => ConnectionChannelSelector::Two(
                LeChannelSelectionAlgorithmTwo::new(request.access_address, request.channel_map),
            ),
        };
        Self {
            request,
            event_counter: 0,
            initial_transmit_window_pending: true,
            selector,
            state: LePeripheralConnectionState::Created,
            pending_channel_map: None,
            pending_connection_update: None,
            maintenance: maintenance::State::new(),
        }
    }

    pub const fn request(&self) -> LeLegacyConnectionRequest {
        self.request
    }

    /// Effective algorithm selected from the advertising/request exchange.
    pub const fn channel_selection(&self) -> LeChannelSelectionAlgorithm {
        match self.selector {
            ConnectionChannelSelector::One { .. } => LeChannelSelectionAlgorithm::AlgorithmOne,
            ConnectionChannelSelector::Two(_) => LeChannelSelectionAlgorithm::AlgorithmTwo,
        }
    }

    pub const fn event_counter(&self) -> u16 {
        self.event_counter
    }

    pub const fn state(&self) -> LePeripheralConnectionState {
        self.state
    }

    /// Wrapping event distance from establishment to the next event.
    pub const fn next_event_distance_from_establishment(&self) -> Option<u16> {
        match self.state.establishment_event_counter() {
            Some(anchor) => Some(self.event_counter.wrapping_sub(anchor)),
            None => None,
        }
    }

    /// Wrapping event distance from the latest peer activity to the next event.
    ///
    /// This is exact protocol bookkeeping only. A later policy may combine it
    /// with connection timing and a hardware time observation when deciding a
    /// supervision timeout.
    pub const fn next_event_distance_from_supervision_anchor(&self) -> Option<u16> {
        match self.state.supervision_anchor_event_counter() {
            Some(anchor) => Some(self.event_counter.wrapping_sub(anchor)),
            None => None,
        }
    }

    /// Prepare one exact event without advancing channel or counter state.
    pub const fn prepare_event(self) -> LePeripheralConnectionEventPrepared {
        let mut connection = self;
        let channel_map_updated =
            connection.apply_channel_map_for_target(connection.event_counter, 0);
        let connection_timing_transition =
            connection.apply_connection_update_for_target(connection.event_counter, 0);
        let (channel, unmapped) = connection.selector.preview(connection.event_counter);
        LePeripheralConnectionEventPrepared {
            connection,
            channel,
            unmapped,
            channel_map_updated,
            connection_timing_transition,
        }
    }

    const fn channel_map_update_for_target(
        &self,
        target: u16,
        skipped: u16,
    ) -> Option<PendingChannelMapUpdate> {
        let update = match self.pending_channel_map {
            Some(update) => update,
            None => return None,
        };
        let source = target.wrapping_sub(skipped);
        if update.instant.wrapping_sub(source) <= skipped {
            Some(update)
        } else {
            None
        }
    }

    const fn apply_channel_map_for_target(&mut self, target: u16, skipped: u16) -> bool {
        if let Some(update) = self.channel_map_update_for_target(target, skipped) {
            self.selector = self.selector.with_channel_map(update.channel_map);
            self.request.channel_map = update.channel_map;
            self.pending_channel_map = None;
            true
        } else {
            false
        }
    }

    /// The pending update whose instant lies among the `skipped` events
    /// before `target` or at it, with the instant's offset from the first of
    /// them, at most `skipped`.
    const fn connection_update_for_target(
        &self,
        target: u16,
        skipped: u16,
    ) -> Option<(PendingConnectionUpdate, u16)> {
        let update = match self.pending_connection_update {
            Some(update) => update,
            None => return None,
        };
        let source = target.wrapping_sub(skipped);
        let offset = update.instant.wrapping_sub(source);
        if offset <= skipped {
            Some((update, offset))
        } else {
            None
        }
    }

    /// The timing of the event `target`, one interval after the last one.
    const fn following(&self, target: u16) -> LeFollowingEvent {
        match self.connection_update_for_target(target, 0) {
            Some((update, _)) => LeFollowingEvent::Instant {
                previous: self.request.timing,
                updated: update.timing,
            },
            None => LeFollowingEvent::Unchanged(self.request.timing),
        }
    }

    const fn apply_connection_update_for_target(
        &mut self,
        target: u16,
        skipped: u16,
    ) -> Option<LeConnectionTimingTransition> {
        let update = match self.connection_update_for_target(target, skipped) {
            Some((update, _)) => update,
            None => return None,
        };
        let transition = LeConnectionTimingTransition {
            previous: self.request.timing,
            updated: update.timing,
            instant: update.instant,
        };
        self.request.timing = update.timing;
        self.pending_connection_update = None;
        Some(transition)
    }
}

/// Protocol event not yet accepted by a hardware backend.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "submit, cancel, or retain the prepared connection event"]
pub struct LePeripheralConnectionEventPrepared {
    connection: LePeripheralConnection,
    channel: LeDataChannelIndex,
    unmapped: u8,
    channel_map_updated: bool,
    connection_timing_transition: Option<LeConnectionTimingTransition>,
}

impl LePeripheralConnectionEventPrepared {
    /// Immutable connection parameters retained by this exact event.
    pub const fn request(&self) -> LeLegacyConnectionRequest {
        self.connection.request
    }

    pub const fn event_counter(&self) -> u16 {
        self.connection.event_counter
    }

    /// Establishment and supervision state before this event.
    pub const fn state(&self) -> LePeripheralConnectionState {
        self.connection.state
    }

    pub const fn channel(&self) -> LeDataChannelIndex {
        self.channel
    }

    pub const fn timing(&self) -> LeConnectionTiming {
        self.connection.request.timing
    }

    pub const fn first_transmit_window_micros(&self) -> Option<(u32, u32)> {
        if self.connection.initial_transmit_window_pending {
            Some((
                self.timing().first_window_start_micros(),
                self.timing().first_window_end_micros(),
            ))
        } else {
            None
        }
    }

    /// Reject lower admission without advancing protocol time or hopping state.
    pub const fn cancel(self) -> LePeripheralConnection {
        self.connection
    }

    /// Mark the point where a lower backend accepted this exact event.
    pub const fn into_submitted(self) -> LePeripheralConnectionEventInFlight {
        LePeripheralConnectionEventInFlight { prepared: self }
    }
}

/// Portable continuation retained alongside one lower hardware transaction.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "complete the exact backend event or retain its connection owner"]
pub struct LePeripheralConnectionEventInFlight {
    prepared: LePeripheralConnectionEventPrepared,
}

impl LePeripheralConnectionEventInFlight {
    pub const fn event_counter(&self) -> u16 {
        self.prepared.event_counter()
    }

    pub const fn channel(&self) -> LeDataChannelIndex {
        self.prepared.channel()
    }

    /// How the event after this one follows it, without completing it.
    pub const fn following_event(&self) -> LeFollowingEvent {
        let connection = &self.prepared.connection;
        connection.following(connection.event_counter.wrapping_add(1))
    }

    /// Advance exactly once after the paired lower owner proves the event closed.
    ///
    /// Both an observed peer packet and a missed event commit the same channel
    /// and event-counter progression. Peer activity additionally establishes
    /// the connection or refreshes its supervision anchor.
    pub const fn complete(
        self,
        peer_activity: LePeripheralConnectionEventPeerActivity,
    ) -> LePeripheralConnectionEventCompleted {
        let LePeripheralConnectionEventPrepared {
            mut connection,
            channel,
            unmapped,
            channel_map_updated,
            connection_timing_transition,
        } = self.prepared;
        let event_counter = connection.event_counter;
        connection.selector = connection.selector.complete(unmapped);
        connection.event_counter = connection.event_counter.wrapping_add(1);
        connection.initial_transmit_window_pending = false;
        connection.state = match (connection.state, peer_activity) {
            (
                LePeripheralConnectionState::Created,
                LePeripheralConnectionEventPeerActivity::Observed,
            ) => LePeripheralConnectionState::Established {
                establishment_event_counter: event_counter,
                supervision_anchor_event_counter: event_counter,
            },
            (
                LePeripheralConnectionState::Established {
                    establishment_event_counter,
                    ..
                },
                LePeripheralConnectionEventPeerActivity::Observed,
            ) => LePeripheralConnectionState::Established {
                establishment_event_counter,
                supervision_anchor_event_counter: event_counter,
            },
            (state, LePeripheralConnectionEventPeerActivity::Missed) => state,
        };
        LePeripheralConnectionEventCompleted {
            connection,
            event_counter,
            channel,
            peer_activity,
            channel_map_updated,
            connection_timing_transition,
        }
    }
}

/// One closed radio event paired with its uniquely advanced LL successor.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "inspect completion or retain its advanced connection successor"]
pub struct LePeripheralConnectionEventCompleted {
    connection: LePeripheralConnection,
    event_counter: u16,
    channel: LeDataChannelIndex,
    peer_activity: LePeripheralConnectionEventPeerActivity,
    channel_map_updated: bool,
    connection_timing_transition: Option<LeConnectionTimingTransition>,
}

impl LePeripheralConnectionEventCompleted {
    /// Exact accepted CONNECT_IND request retained by the advanced connection.
    pub const fn request(&self) -> LeLegacyConnectionRequest {
        self.connection.request
    }

    pub const fn timing(&self) -> LeConnectionTiming {
        self.connection.request.timing()
    }

    /// How the next event follows the closed one, without preparing it:
    /// [`Self::prepare_recurring_event`] of one interval previews the same.
    pub const fn following_event(&self) -> LeFollowingEvent {
        // The successor already holds the next event's counter.
        self.connection.following(self.connection.event_counter)
    }

    pub const fn event_counter(&self) -> u16 {
        self.event_counter
    }

    pub const fn channel(&self) -> LeDataChannelIndex {
        self.channel
    }

    pub const fn peer_activity(&self) -> LePeripheralConnectionEventPeerActivity {
        self.peer_activity
    }

    pub const fn connection_state(&self) -> LePeripheralConnectionState {
        self.connection.state
    }

    /// Connection parameters applied by the event which just closed.
    pub const fn connection_timing_transition(&self) -> Option<LeConnectionTimingTransition> {
        self.connection_timing_transition
    }

    /// Whether the Channel Map Update instant was applied by this event.
    pub const fn channel_map_updated(&self) -> bool {
        self.channel_map_updated
    }

    /// Whether six initial connection events have closed without a peer packet.
    ///
    /// Core Vol 6, Part B, 4.5.2 requires failed establishment after six events.
    /// The backend must check this before scheduling another event; this is
    /// distinct from the elapsed-time supervision of an established connection.
    pub const fn establishment_failed(&self) -> bool {
        matches!(self.connection.state, LePeripheralConnectionState::Created)
            && self.event_counter >= 5
    }

    /// Stage a valid Central Channel Map Update until its connection instant.
    pub fn schedule_channel_map_update(
        &mut self,
        channel_map: LeDataChannelMap,
        instant: u16,
    ) -> Result<(), LePeripheralChannelMapUpdateError> {
        if self.connection.pending_channel_map.is_some() {
            return Err(LePeripheralChannelMapUpdateError::ProcedureAlreadyPending);
        }
        if self.channel_map_updated {
            return Err(LePeripheralChannelMapUpdateError::ProcedureAlreadyPending);
        }
        if self.connection.pending_connection_update.is_some()
            || self.connection_timing_transition.is_some()
        {
            return Err(LePeripheralChannelMapUpdateError::IncompatibleProcedurePending);
        }
        let distance = instant.wrapping_sub(self.event_counter);
        if distance >= 0x7fff {
            return Err(LePeripheralChannelMapUpdateError::InstantPassed);
        }
        if distance == 0 {
            self.connection.selector = self.connection.selector.with_channel_map(channel_map);
            self.connection.request.channel_map = channel_map;
            self.channel_map_updated = true;
            return Ok(());
        }
        self.connection.pending_channel_map = Some(PendingChannelMapUpdate {
            channel_map,
            instant,
        });
        self.connection.maintenance.instant_received();
        Ok(())
    }

    /// Stage a valid Central Connection Update until its connection instant.
    pub fn schedule_connection_update(
        &mut self,
        timing: LeConnectionTiming,
        instant: u16,
    ) -> Result<(), LePeripheralConnectionUpdateError> {
        if self.connection.pending_connection_update.is_some() {
            return Err(LePeripheralConnectionUpdateError::ProcedureAlreadyPending);
        }
        if self.connection_timing_transition.is_some() {
            return Err(LePeripheralConnectionUpdateError::ProcedureAlreadyPending);
        }
        if self.connection.pending_channel_map.is_some() || self.channel_map_updated {
            return Err(LePeripheralConnectionUpdateError::IncompatibleProcedurePending);
        }
        let distance = instant.wrapping_sub(self.event_counter);
        if distance >= 0x7fff {
            return Err(LePeripheralConnectionUpdateError::InstantPassed);
        }
        if distance == 0 {
            self.connection_timing_transition = Some(LeConnectionTimingTransition {
                previous: self.connection.request.timing,
                updated: timing,
                instant,
            });
            self.connection.request.timing = timing;
            return Ok(());
        }
        self.connection.pending_connection_update =
            Some(PendingConnectionUpdate { timing, instant });
        self.connection.maintenance.instant_received();
        Ok(())
    }

    /// Preview a recurring event without advancing the retained LL owner.
    ///
    /// The completed event already owns the successor at `n + 1`, so only the
    /// `delta - 1` intervening events are skipped before previewing `n + delta`.
    /// A lower scheduler can inspect this candidate and either cancel it or
    /// accept it before the LL successor is committed.
    pub const fn prepare_recurring_event(
        self,
        delta: LePeripheralConnectionEventDelta,
    ) -> LePeripheralConnectionRecurringEventProvisional {
        let skipped = delta.skipped();
        let event_counter = self.connection.event_counter.wrapping_add(skipped);
        let mut selector = self.connection.selector.skip(skipped);
        let channel_map_update = self
            .connection
            .channel_map_update_for_target(event_counter, skipped);
        if let Some(update) = channel_map_update {
            selector = selector.with_channel_map(update.channel_map);
        }
        let (channel, _) = selector.preview(event_counter);
        let (connection_timing_transition, connection_timing_intervals) = match self
            .connection
            .connection_update_for_target(event_counter, skipped)
        {
            Some((update, offset)) => (
                Some(LeConnectionTimingTransition {
                    previous: self.connection.request.timing,
                    updated: update.timing,
                    instant: update.instant,
                }),
                // The instant is `offset + 1` events after the completed one,
                // within the `skipped + 1` of the step: `offset <= skipped`.
                Some(LeConnectionTimingIntervals {
                    before: offset + 1,
                    after: skipped - offset,
                }),
            ),
            None => (None, None),
        };
        LePeripheralConnectionRecurringEventProvisional {
            completed: self,
            delta,
            event_counter,
            channel,
            channel_map_update_instant: match channel_map_update {
                Some(update) => Some(update.instant),
                None => None,
            },
            connection_timing_transition,
            connection_timing_intervals,
            maintenance_skip: false,
        }
    }

    /// Consume the completion record into the already advanced successor.
    pub fn into_connection(self) -> LePeripheralConnection {
        self.connection
    }
}

/// Recurring-event candidate whose advanced successor is not yet committed.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "inspect and commit the recurring event, or recover its completion owner"]
pub struct LePeripheralConnectionRecurringEventProvisional {
    completed: LePeripheralConnectionEventCompleted,
    delta: LePeripheralConnectionEventDelta,
    event_counter: u16,
    channel: LeDataChannelIndex,
    channel_map_update_instant: Option<u16>,
    connection_timing_transition: Option<LeConnectionTimingTransition>,
    connection_timing_intervals: Option<LeConnectionTimingIntervals>,
    maintenance_skip: bool,
}

impl LePeripheralConnectionRecurringEventProvisional {
    pub const fn request(&self) -> LeLegacyConnectionRequest {
        self.completed.connection.request
    }

    pub const fn delta(&self) -> LePeripheralConnectionEventDelta {
        self.delta
    }

    pub const fn event_counter(&self) -> u16 {
        self.event_counter
    }

    pub const fn channel(&self) -> LeDataChannelIndex {
        self.channel
    }

    /// Channel Map Update instant crossed by this provisional event.
    pub const fn channel_map_update_instant(&self) -> Option<u16> {
        self.channel_map_update_instant
    }

    pub const fn timing(&self) -> LeConnectionTiming {
        match self.connection_timing_transition {
            Some(transition) => transition.updated,
            None => self.completed.connection.request.timing,
        }
    }

    pub const fn connection_state(&self) -> LePeripheralConnectionState {
        self.completed.connection_state()
    }

    /// Timing transition which must shape this instant event's scheduler window.
    pub const fn connection_timing_transition(&self) -> Option<LeConnectionTimingTransition> {
        self.connection_timing_transition
    }

    /// How the intervals of this step divide around the transition's instant.
    pub const fn connection_timing_intervals(&self) -> Option<LeConnectionTimingIntervals> {
        self.connection_timing_intervals
    }

    /// Reject lower admission and recover the exact completed-event owner.
    pub const fn cancel(self) -> LePeripheralConnectionEventCompleted {
        self.completed
    }

    /// Commit the preview after lower admission and prepare the exact event.
    pub const fn commit(self) -> LePeripheralConnectionEventPrepared {
        let Self {
            completed,
            delta,
            event_counter,
            channel,
            channel_map_update_instant: _,
            connection_timing_transition: _,
            connection_timing_intervals: _,
            maintenance_skip,
        } = self;
        let skipped = delta.skipped();
        let mut connection = completed.connection;
        if maintenance_skip {
            connection.maintenance.commit_skip();
        }
        connection.event_counter = event_counter;
        connection.selector = connection.selector.skip(skipped);
        let channel_map_updated = connection.apply_channel_map_for_target(event_counter, skipped);
        let connection_timing_transition =
            connection.apply_connection_update_for_target(event_counter, skipped);
        let (_, unmapped) = connection.selector.preview(event_counter);
        LePeripheralConnectionEventPrepared {
            connection,
            channel,
            unmapped,
            channel_map_updated,
            connection_timing_transition,
        }
    }
}

#[cfg(test)]
mod tests;

pub mod acknowledgement;
pub mod maintenance;
