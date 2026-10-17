//! Allocation-free IEEE 802.11ax Trigger and HE-control field parsing.
//!
//! This module deliberately describes bytes received over the air. It does
//! not describe ESP32-S31 MMIO. The primary implementation oracle is the
//! complete pinned `libpp.a[hal_debug.o]` member:
//!
//! - `dbg_dump_trig_common_info`;
//! - `dbg_dump_trig_user_ru` and `dbg_dump_trig_user_ss`;
//! - `dbg_dump_trig_basic_dependent`, `dbg_dump_trig_bfrp_dependent`,
//!   `dbg_dump_trig_mubar_dependent` and `dbg_dump_trig_nfrp_user`;
//! - `dbg_dump_trs_control` and `dbg_dump_uph_control`.
//!
//! Every extraction below reproduces the loads, shifts and masks in those
//! functions. Derived units such as dBm are exposed separately from their
//! wire encodings.

use crate::sequence::SequenceNumber;

pub const TRIGGER_COMMON_INFO_LEN: usize = 8;
pub const TRIGGER_MAC_HEADER_LEN: usize = 16;
pub const TRIGGER_FRAME_MIN_LEN: usize = TRIGGER_MAC_HEADER_LEN + TRIGGER_COMMON_INFO_LEN;
pub const TRIGGER_USER_INFO_LEN: usize = 5;
pub const TRIGGER_BASIC_DEPENDENT_LEN: usize = 1;
pub const TRIGGER_BASIC_FRAME_LEN: usize =
    TRIGGER_FRAME_MIN_LEN + TRIGGER_USER_INFO_LEN + TRIGGER_BASIC_DEPENDENT_LEN;
pub const TRIGGER_BFRP_DEPENDENT_LEN: usize = 1;
pub const TRIGGER_MU_BAR_DEPENDENT_LEN: usize = 4;
pub const TRIGGER_NFRP_USER_INFO_LEN: usize = 5;
pub const HE_CONTROL_INFO_LEN: usize = 4;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TriggerParseError {
    Truncated { required: usize },
    NotTriggerFrame,
    UnsupportedUserLayout { trigger_type: TriggerType },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TriggerEncodeError {
    OutputTooSmall { required: usize },
    UplinkLengthOutOfRange,
    UplinkBandwidthOutOfRange,
    HeLtfSymbolsOutOfRange,
    ApTxPowerOutOfRange,
    PreFecPaddingOutOfRange,
    HeSigA2ReservedOutOfRange,
    AssociationIdOutOfRange,
    RuAllocationOutOfRange,
    McsOutOfRange,
    StartingSpatialStreamOutOfRange,
    SpatialStreamCountOutOfRange,
    TargetRssiOutOfRange,
    BasicDependentInfoOutOfRange,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TriggerType {
    Basic,
    BeamformingReportPoll,
    MultiUserBlockAckRequest,
    MultiUserRequestToSend,
    BufferStatusReportPoll,
    GroupcastMultiUserBlockAckRequest,
    BandwidthQueryReportPoll,
    NgpaFeedbackReportPoll,
    Reserved(u8),
}

impl TriggerType {
    pub const fn from_encoding(value: u8) -> Self {
        match value & 0x0f {
            0 => Self::Basic,
            1 => Self::BeamformingReportPoll,
            2 => Self::MultiUserBlockAckRequest,
            3 => Self::MultiUserRequestToSend,
            4 => Self::BufferStatusReportPoll,
            5 => Self::GroupcastMultiUserBlockAckRequest,
            6 => Self::BandwidthQueryReportPoll,
            7 => Self::NgpaFeedbackReportPoll,
            value => Self::Reserved(value),
        }
    }

    pub const fn encoding(self) -> u8 {
        match self {
            Self::Basic => 0,
            Self::BeamformingReportPoll => 1,
            Self::MultiUserBlockAckRequest => 2,
            Self::MultiUserRequestToSend => 3,
            Self::BufferStatusReportPoll => 4,
            Self::GroupcastMultiUserBlockAckRequest => 5,
            Self::BandwidthQueryReportPoll => 6,
            Self::NgpaFeedbackReportPoll => 7,
            Self::Reserved(value) => value & 0x0f,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TriggerGiLtf {
    OneLtf1600Ns,
    TwoLtf1600Ns,
    FourLtf3200Ns,
    Reserved,
}

impl TriggerGiLtf {
    const fn from_encoding(value: u8) -> Self {
        match value & 0x03 {
            0 => Self::OneLtf1600Ns,
            1 => Self::TwoLtf1600Ns,
            2 => Self::FourLtf3200Ns,
            _ => Self::Reserved,
        }
    }

    /// Two-bit Trigger Common Info wire encoding.
    ///
    /// This is not the HE-SU HE-SIG-A GI/LTF encoding: Trigger-based uplink
    /// has no 0.8-us value. SOURCE\[BLOB_LIBPP_DBG_TRIGGER_COMMON] proves bits
    /// 21:20 and their named GI/LTF role. IEEE 802.11 Trigger Common Info,
    /// independently implemented by Wireshark
    /// `packet-ieee80211.c::gi_and_ltf_type_subfield_vals`, supplies the exact
    /// `1x/1.6`, `2x/1.6`, `4x/3.2`, reserved mapping.
    pub const fn encoding(self) -> u8 {
        match self {
            Self::OneLtf1600Ns => 0,
            Self::TwoLtf1600Ns => 1,
            Self::FourLtf3200Ns => 2,
            Self::Reserved => 3,
        }
    }
}

/// Fields used to encode one Trigger Common Info value.
///
/// This is deliberately separate from [`TriggerCommonInfo`]: the parsed type
/// also contains derived physical-unit values, while an encoder must have only
/// one authoritative representation for every wire field.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerCommonEncoding {
    pub uplink_length: u16,
    pub more_trigger_frames: bool,
    pub carrier_sense_required: bool,
    pub uplink_bandwidth_encoding: u8,
    pub gi_ltf: TriggerGiLtf,
    pub mu_mimo_ltf_mode: bool,
    pub he_ltf_symbols_and_midamble_periodicity: u8,
    pub uplink_stbc: bool,
    pub ldpc_extra_symbol_segment: bool,
    pub ap_tx_power_encoding: u8,
    pub pre_fec_padding_factor_encoding: u8,
    pub packet_extension_disambiguity: bool,
    pub uplink_spatial_reuse: u16,
    pub doppler: bool,
    pub uplink_he_sig_a2_reserved: u16,
    pub trailing_reserved: bool,
}

impl TriggerCommonEncoding {
    /// Encode Common Info using the inverse of the complete blob decoder.
    ///
    /// SOURCE(esp32s31)\[BLOB_LIBPP_DBG_DUMP_TRIG_COMMON_INFO]: complete
    /// `libpp.a[hal_debug.o]::dbg_dump_trig_common_info`. This method
    /// reverses only its instruction-proven shifts and masks; it does not
    /// choose policy values for reserved or scheduling fields.
    pub fn encode(
        self,
        trigger_type: TriggerType,
    ) -> Result<[u8; TRIGGER_COMMON_INFO_LEN], TriggerEncodeError> {
        if self.uplink_length > 0x0fff {
            return Err(TriggerEncodeError::UplinkLengthOutOfRange);
        }
        if self.uplink_bandwidth_encoding > 0x03 {
            return Err(TriggerEncodeError::UplinkBandwidthOutOfRange);
        }
        if self.he_ltf_symbols_and_midamble_periodicity > 0x07 {
            return Err(TriggerEncodeError::HeLtfSymbolsOutOfRange);
        }
        if self.ap_tx_power_encoding > 0x3f {
            return Err(TriggerEncodeError::ApTxPowerOutOfRange);
        }
        if self.pre_fec_padding_factor_encoding > 0x03 {
            return Err(TriggerEncodeError::PreFecPaddingOutOfRange);
        }
        if self.uplink_he_sig_a2_reserved > 0x01ff {
            return Err(TriggerEncodeError::HeSigA2ReservedOutOfRange);
        }

        let bits = u64::from(trigger_type.encoding())
            | (u64::from(self.uplink_length) << 4)
            | (u64::from(self.more_trigger_frames) << 16)
            | (u64::from(self.carrier_sense_required) << 17)
            | (u64::from(self.uplink_bandwidth_encoding) << 18)
            | (u64::from(self.gi_ltf.encoding()) << 20)
            | (u64::from(self.mu_mimo_ltf_mode) << 22)
            | (u64::from(self.he_ltf_symbols_and_midamble_periodicity) << 23)
            | (u64::from(self.uplink_stbc) << 26)
            | (u64::from(self.ldpc_extra_symbol_segment) << 27)
            | (u64::from(self.ap_tx_power_encoding) << 28)
            | (u64::from(self.pre_fec_padding_factor_encoding) << 34)
            | (u64::from(self.packet_extension_disambiguity) << 36)
            | (u64::from(self.uplink_spatial_reuse) << 37)
            | (u64::from(self.doppler) << 53)
            | (u64::from(self.uplink_he_sig_a2_reserved) << 54)
            | (u64::from(self.trailing_reserved) << 63);
        Ok(bits.to_le_bytes())
    }
}

/// Scheduled spatial-stream fields for one Trigger User Info value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerScheduledUserEncoding {
    pub association_id: u16,
    pub ru_allocation_region: bool,
    pub ru_allocation: u8,
    pub coding_type: bool,
    pub mcs: u8,
    pub dcm: bool,
    pub starting_spatial_stream_encoding: u8,
    pub spatial_stream_count_encoding: u8,
    pub target_rssi_encoding: u8,
    pub reserved: bool,
}

impl TriggerScheduledUserEncoding {
    /// Encode User Info using the inverse of the complete blob decoder.
    ///
    /// SOURCE(esp32s31)\[BLOB_LIBPP_DBG_DUMP_TRIG_USER_SS]: complete
    /// `libpp.a[hal_debug.o]::dbg_dump_trig_user_ss`.
    pub fn encode(self) -> Result<[u8; TRIGGER_USER_INFO_LEN], TriggerEncodeError> {
        if self.association_id == 0 || self.association_id > 0x0fff {
            return Err(TriggerEncodeError::AssociationIdOutOfRange);
        }
        if self.ru_allocation > 0x7f {
            return Err(TriggerEncodeError::RuAllocationOutOfRange);
        }
        if self.mcs > 0x0f {
            return Err(TriggerEncodeError::McsOutOfRange);
        }
        if self.starting_spatial_stream_encoding > 0x07 {
            return Err(TriggerEncodeError::StartingSpatialStreamOutOfRange);
        }
        if self.spatial_stream_count_encoding > 0x07 {
            return Err(TriggerEncodeError::SpatialStreamCountOutOfRange);
        }
        if self.target_rssi_encoding > 0x7f {
            return Err(TriggerEncodeError::TargetRssiOutOfRange);
        }

        Ok([
            self.association_id as u8,
            ((self.association_id >> 8) as u8 & 0x0f)
                | (u8::from(self.ru_allocation_region) << 4)
                | ((self.ru_allocation & 0x07) << 5),
            ((self.ru_allocation >> 3) & 0x0f)
                | (u8::from(self.coding_type) << 4)
                | ((self.mcs & 0x07) << 5),
            ((self.mcs >> 3) & 0x01)
                | (u8::from(self.dcm) << 1)
                | (self.starting_spatial_stream_encoding << 2)
                | (self.spatial_stream_count_encoding << 5),
            self.target_rssi_encoding | (u8::from(self.reserved) << 7),
        ])
    }
}

impl TriggerBasicDependentInfo {
    /// Encode Basic Trigger-dependent User Info.
    ///
    /// SOURCE(esp32s31)\[BLOB_LIBPP_DBG_DUMP_TRIG_BASIC_DEPENDENT]: complete
    /// `libpp.a[hal_debug.o]::dbg_dump_trig_basic_dependent`.
    pub fn encode(self) -> Result<u8, TriggerEncodeError> {
        if self.mpdu_mu_spacing_factor > 0x03
            || self.tid_aggregation_limit > 0x07
            || self.preferred_access_category > 0x03
        {
            return Err(TriggerEncodeError::BasicDependentInfoOutOfRange);
        }
        Ok(self.mpdu_mu_spacing_factor
            | (self.tid_aggregation_limit << 2)
            | (u8::from(self.reserved) << 5)
            | (self.preferred_access_category << 6))
    }
}

/// One Basic Trigger control MPDU with exactly one scheduled user.
///
/// `encode` omits the FCS. A host injection interface or radio DMA owner must
/// add it according to that interface's contract.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BasicTriggerFrameEncoding {
    pub duration: u16,
    pub receiver_address: [u8; 6],
    pub transmitter_address: [u8; 6],
    pub common: TriggerCommonEncoding,
    pub user: TriggerScheduledUserEncoding,
    pub dependent: TriggerBasicDependentInfo,
}

impl BasicTriggerFrameEncoding {
    pub fn encode(self, output: &mut [u8]) -> Result<usize, TriggerEncodeError> {
        if output.len() < TRIGGER_BASIC_FRAME_LEN {
            return Err(TriggerEncodeError::OutputTooSmall {
                required: TRIGGER_BASIC_FRAME_LEN,
            });
        }
        let common = self.common.encode(TriggerType::Basic)?;
        let user = self.user.encode()?;
        let dependent = self.dependent.encode()?;
        output[..TRIGGER_BASIC_FRAME_LEN].fill(0);
        output[0..2].copy_from_slice(&0x0024_u16.to_le_bytes());
        output[2..4].copy_from_slice(&self.duration.to_le_bytes());
        output[4..10].copy_from_slice(&self.receiver_address);
        output[10..16].copy_from_slice(&self.transmitter_address);
        output[16..24].copy_from_slice(&common);
        output[24..29].copy_from_slice(&user);
        output[29] = dependent;
        Ok(TRIGGER_BASIC_FRAME_LEN)
    }
}

pub use crate::he::HeResourceUnit;

/// One raw Trigger RU allocation classified by the complete blob helper.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TriggerRuAllocation {
    /// A supported narrow RU and its one-based index in the printed group.
    Narrow {
        resource_unit: HeResourceUnit,
        one_based_index: u8,
    },
    /// The blob labels every encoding at or above 69 as `484 OFDM`.
    ///
    /// This is retained for diagnostics but lies outside the HE20-specific
    /// narrow-RU representation. Transmit bandwidth support belongs to the backend.
    WiderThan242,
}

impl TriggerRuAllocation {
    /// Classify the seven-bit Trigger RU allocation without guessing gaps.
    ///
    /// SOURCE(esp32s31)\[BLOB_LIBPP_HAL_UTILITIES_RU2STR]: complete
    /// `libpp.a[hal_utilities.o]::ru2str` (size `0x8c`) formats
    /// 0..8 as 26-tone index `raw+1`, 37..40 as 52-tone index `raw-36`,
    /// 53..54 as 106-tone index `raw-52`, 61..62 as 242-tone index
    /// `raw-60`, and values at least 69 as `484 OFDM`. The intervening
    /// encodings do not produce a fresh valid index and remain `None`.
    pub const fn from_encoding(raw: u8) -> Option<Self> {
        match raw {
            0..=8 => Some(Self::Narrow {
                resource_unit: HeResourceUnit::Ru26,
                one_based_index: raw + 1,
            }),
            37..=40 => Some(Self::Narrow {
                resource_unit: HeResourceUnit::Ru52,
                one_based_index: raw - 36,
            }),
            53..=54 => Some(Self::Narrow {
                resource_unit: HeResourceUnit::Ru106,
                one_based_index: raw - 52,
            }),
            61..=62 => Some(Self::Narrow {
                resource_unit: HeResourceUnit::Ru242,
                one_based_index: raw - 60,
            }),
            69..=0x7f => Some(Self::WiderThan242),
            _ => None,
        }
    }

    pub const fn resource_unit(self) -> Option<HeResourceUnit> {
        match self {
            Self::Narrow { resource_unit, .. } => Some(resource_unit),
            Self::WiderThan242 => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TriggerTargetRssi {
    Dbm(i8),
    Reserved,
}

impl TriggerTargetRssi {
    const fn from_user_encoding(value: u8) -> Self {
        if value == 0x7f {
            Self::Reserved
        } else {
            Self::Dbm(value as i8 - 110)
        }
    }

    const fn from_trs_encoding(value: u8) -> Self {
        if value == 0x1f {
            Self::Reserved
        } else {
            Self::Dbm((value as i8 - 45) * 2)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TriggerApTxPower {
    Dbm(i8),
    Reserved,
}

impl TriggerApTxPower {
    const fn from_common_encoding(value: u8) -> Self {
        Self::Dbm(value as i8 - 20)
    }

    const fn from_trs_encoding(value: u8) -> Self {
        if value == 0x1f {
            Self::Reserved
        } else {
            Self::Dbm((value as i8 - 10) * 2)
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerCommonInfo {
    pub trigger_type: TriggerType,
    pub uplink_length: u16,
    pub more_trigger_frames: bool,
    pub carrier_sense_required: bool,
    pub uplink_bandwidth_encoding: u8,
    pub gi_ltf: TriggerGiLtf,
    pub mu_mimo_ltf_mode: bool,
    pub he_ltf_symbols_and_midamble_periodicity: u8,
    pub uplink_stbc: bool,
    pub ldpc_extra_symbol_segment: bool,
    pub ap_tx_power_encoding: u8,
    pub ap_tx_power: TriggerApTxPower,
    pub pre_fec_padding_factor_encoding: u8,
    pub pre_fec_padding_factor: u8,
    pub packet_extension_disambiguity: bool,
    pub uplink_spatial_reuse: u16,
    pub doppler: bool,
    pub uplink_he_sig_a2_reserved: u16,
    pub trailing_reserved: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerFrame<'a> {
    pub frame_control: u16,
    pub duration: u16,
    pub receiver_address: [u8; 6],
    pub transmitter_address: [u8; 6],
    pub common: TriggerCommonInfo,
    /// User Info fields and any trailing padding.
    pub user_info_and_padding: &'a [u8],
}

impl<'a> TriggerFrame<'a> {
    /// Iterate over bounded User Info fields without allocation.
    ///
    /// IEEE Std 802.11ax-2021 9.3.1.22 defines a five-byte User Info field
    /// followed by the type's Trigger Dependent User Info: one byte for
    /// Basic, the four-byte BAR Control and BAR Information for MU-BAR, and
    /// none for BFRP, MU-RTS, BSRP and BQRP; an NFRP Trigger carries one
    /// five-byte User Info. The optional Padding field starts with AID12
    /// `4095` and is all ones, so this iterator stops at a field whose AID12
    /// is `0xfff` and RU allocation is `0x7f`. Complete pinned
    /// `libpp.a[hal_debug.o]::{dbg_dump_trig_basic_dependent,
    /// dbg_dump_trig_mubar_dependent, dbg_dump_trig_nfrp_user}` decode the
    /// same per-type suffixes.
    ///
    /// Groupcast MU-BAR and reserved Trigger types have no layout this
    /// iterator admits; it reports them as an error instead of guessing.
    pub const fn users(&self) -> TriggerUserIterator<'a> {
        TriggerUserIterator::new(self.common.trigger_type, self.user_info_and_padding)
    }
}

/// One borrowed Trigger User Info field and its type-dependent suffix.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerUserField<'a> {
    /// The common five-byte User Info field.
    pub user_info: &'a [u8],
    /// Basic contributes one byte and MU-BAR contributes four; other
    /// instruction-proven layouts expose an empty slice.
    pub dependent_info: &'a [u8],
}

impl TriggerUserField<'_> {
    pub const fn aid12(&self) -> u16 {
        u16::from_le_bytes([self.user_info[0], self.user_info[1] & 0x0f])
    }

    pub const fn ru_allocation(&self) -> u8 {
        (self.user_info[1] >> 5) | ((self.user_info[2] & 0x0f) << 3)
    }

    pub const fn classified_ru_allocation(&self) -> Option<TriggerRuAllocation> {
        TriggerRuAllocation::from_encoding(self.ru_allocation())
    }
}

/// Allocation-free, fail-closed iterator over Trigger User Info fields.
#[derive(Clone, Debug)]
pub struct TriggerUserIterator<'a> {
    trigger_type: TriggerType,
    remaining: &'a [u8],
    padding: &'a [u8],
    finished: bool,
}

impl<'a> TriggerUserIterator<'a> {
    const fn new(trigger_type: TriggerType, bytes: &'a [u8]) -> Self {
        Self {
            trigger_type,
            remaining: bytes,
            padding: &[],
            finished: false,
        }
    }

    /// Return the padding sentinel and bytes after it once iteration stops.
    ///
    /// Before the iterator reaches the sentinel this is an empty slice.
    pub const fn padding(&self) -> &'a [u8] {
        self.padding
    }

    fn fail(
        &mut self,
        error: TriggerParseError,
    ) -> Option<Result<TriggerUserField<'a>, TriggerParseError>> {
        self.finished = true;
        Some(Err(error))
    }
}

impl<'a> Iterator for TriggerUserIterator<'a> {
    type Item = Result<TriggerUserField<'a>, TriggerParseError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.finished || self.remaining.is_empty() {
            return None;
        }
        if self.remaining.len() < TRIGGER_USER_INFO_LEN {
            return self.fail(TriggerParseError::Truncated {
                required: TRIGGER_USER_INFO_LEN,
            });
        }

        let aid12 = u16::from(self.remaining[0]) | (u16::from(self.remaining[1] & 0x0f) << 8);
        let ru_allocation = (self.remaining[1] >> 5) | ((self.remaining[2] & 0x0f) << 3);
        if aid12 == 0x0fff && ru_allocation == 0x7f {
            self.padding = self.remaining;
            self.remaining = &[];
            self.finished = true;
            return None;
        }

        let (dependent_len, terminal) = match self.trigger_type {
            TriggerType::Basic => (TRIGGER_BASIC_DEPENDENT_LEN, false),
            TriggerType::BeamformingReportPoll
            | TriggerType::MultiUserRequestToSend
            | TriggerType::BufferStatusReportPoll
            | TriggerType::BandwidthQueryReportPoll => (0, false),
            TriggerType::MultiUserBlockAckRequest => (TRIGGER_MU_BAR_DEPENDENT_LEN, false),
            TriggerType::NgpaFeedbackReportPoll => (0, true),
            TriggerType::GroupcastMultiUserBlockAckRequest | TriggerType::Reserved(_) => {
                return self.fail(TriggerParseError::UnsupportedUserLayout {
                    trigger_type: self.trigger_type,
                });
            }
        };
        let required = TRIGGER_USER_INFO_LEN + dependent_len;
        if self.remaining.len() < required {
            return self.fail(TriggerParseError::Truncated { required });
        }

        let (field, rest) = self.remaining.split_at(required);
        let user = TriggerUserField {
            user_info: &field[..TRIGGER_USER_INFO_LEN],
            dependent_info: &field[TRIGGER_USER_INFO_LEN..],
        };
        if terminal {
            self.padding = rest;
            self.remaining = &[];
            self.finished = true;
        } else {
            self.remaining = rest;
        }
        Some(Ok(user))
    }
}

/// Admit a complete Trigger control MPDU whose FCS has already been removed.
///
/// The 16-byte MAC header boundary is the one consumed by the hardware
/// Trigger diagnostics before the complete
/// `libpp.a[hal_debug.o]::dbg_dump_trig_common_info` field. The frame
/// control check uses IEEE 802.11 type/subtype bits and allows legal flag bits
/// in the second byte.
pub fn parse_trigger_frame(frame: &[u8]) -> Result<TriggerFrame<'_>, TriggerParseError> {
    require(frame, TRIGGER_FRAME_MIN_LEN)?;
    let frame_control = u16::from_le_bytes([frame[0], frame[1]]);
    if frame_control & 0x00fc != 0x0024 {
        return Err(TriggerParseError::NotTriggerFrame);
    }
    let mut receiver_address = [0; 6];
    receiver_address.copy_from_slice(&frame[4..10]);
    let mut transmitter_address = [0; 6];
    transmitter_address.copy_from_slice(&frame[10..16]);
    Ok(TriggerFrame {
        frame_control,
        duration: u16::from_le_bytes([frame[2], frame[3]]),
        receiver_address,
        transmitter_address,
        common: parse_trigger_common_info(&frame[TRIGGER_MAC_HEADER_LEN..TRIGGER_FRAME_MIN_LEN])?,
        user_info_and_padding: &frame[TRIGGER_FRAME_MIN_LEN..],
    })
}

/// Parse the fixed eight-byte Trigger Common Info field.
///
/// SOURCE(esp32s31)\[BLOB_LIBPP_DBG_DUMP_TRIG_COMMON_INFO]: complete
/// `libpp.a[hal_debug.o]::dbg_dump_trig_common_info`. The blob reads
/// two little-endian words and extracts bits 0:62 exactly as represented here;
/// bit 63 is retained as `trailing_reserved` even though the debug printer
/// omits it.
pub fn parse_trigger_common_info(bytes: &[u8]) -> Result<TriggerCommonInfo, TriggerParseError> {
    require(bytes, TRIGGER_COMMON_INFO_LEN)?;
    let bits = u64::from_le_bytes([
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
    ]);
    let ap_tx_power_encoding = field(bits, 28, 0x3f) as u8;
    let pre_fec_padding_factor_encoding = field(bits, 34, 0x03) as u8;
    Ok(TriggerCommonInfo {
        trigger_type: TriggerType::from_encoding(field(bits, 0, 0x0f) as u8),
        uplink_length: field(bits, 4, 0x0fff) as u16,
        more_trigger_frames: bit(bits, 16),
        carrier_sense_required: bit(bits, 17),
        uplink_bandwidth_encoding: field(bits, 18, 0x03) as u8,
        gi_ltf: TriggerGiLtf::from_encoding(field(bits, 20, 0x03) as u8),
        mu_mimo_ltf_mode: bit(bits, 22),
        he_ltf_symbols_and_midamble_periodicity: field(bits, 23, 0x07) as u8,
        uplink_stbc: bit(bits, 26),
        ldpc_extra_symbol_segment: bit(bits, 27),
        ap_tx_power_encoding,
        ap_tx_power: TriggerApTxPower::from_common_encoding(ap_tx_power_encoding),
        pre_fec_padding_factor_encoding,
        pre_fec_padding_factor: if pre_fec_padding_factor_encoding == 0 {
            4
        } else {
            pre_fec_padding_factor_encoding
        },
        packet_extension_disambiguity: bit(bits, 36),
        uplink_spatial_reuse: field(bits, 37, 0xffff) as u16,
        doppler: bit(bits, 53),
        uplink_he_sig_a2_reserved: field(bits, 54, 0x01ff) as u16,
        trailing_reserved: bit(bits, 63),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerUserRuInfo {
    pub aid12: u16,
    pub ru_allocation_region: bool,
    pub ru_allocation: u8,
    pub coding_type: bool,
    pub mcs: u8,
    pub dcm: bool,
    pub number_of_ra_ru: u8,
    pub more_ra_ru: bool,
    pub target_rssi_encoding: u8,
    pub target_rssi: TriggerTargetRssi,
    pub reserved: bool,
}

/// Parse the random-access RU form of a five-byte Trigger User Info field.
///
/// SOURCE(esp32s31)\[BLOB_LIBPP_DBG_DUMP_TRIG_USER_RU]: complete
/// `libpp.a[hal_debug.o]::dbg_dump_trig_user_ru`.
pub fn parse_trigger_user_ru(bytes: &[u8]) -> Result<TriggerUserRuInfo, TriggerParseError> {
    require(bytes, TRIGGER_USER_INFO_LEN)?;
    let target_rssi_encoding = bytes[4] & 0x7f;
    Ok(TriggerUserRuInfo {
        aid12: u16::from(bytes[0]) | (u16::from(bytes[1] & 0x0f) << 8),
        ru_allocation_region: bytes[1] & 0x10 != 0,
        ru_allocation: (bytes[1] >> 5) | ((bytes[2] & 0x0f) << 3),
        coding_type: bytes[2] & 0x10 != 0,
        mcs: (bytes[2] >> 5) | ((bytes[3] & 0x01) << 3),
        dcm: bytes[3] & 0x02 != 0,
        number_of_ra_ru: (bytes[3] >> 2) & 0x1f,
        more_ra_ru: bytes[3] & 0x80 != 0,
        target_rssi_encoding,
        target_rssi: TriggerTargetRssi::from_user_encoding(target_rssi_encoding),
        reserved: bytes[4] & 0x80 != 0,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerUserSpatialStreamInfo {
    pub aid12: u16,
    pub ru_allocation_region: bool,
    pub ru_allocation: u8,
    pub coding_type: bool,
    pub mcs: u8,
    pub dcm: bool,
    pub starting_spatial_stream_encoding: u8,
    pub starting_spatial_stream: u8,
    pub spatial_stream_count_encoding: u8,
    pub spatial_stream_count: u8,
    pub target_rssi_encoding: u8,
    pub target_rssi: TriggerTargetRssi,
    pub reserved: bool,
}

/// Parse the scheduled spatial-stream form of Trigger User Info.
///
/// SOURCE(esp32s31)\[BLOB_LIBPP_DBG_DUMP_TRIG_USER_SS]: complete
/// `libpp.a[hal_debug.o]::dbg_dump_trig_user_ss`. The blob suppresses
/// this form for AID12 `0xfff`; this bounded parser preserves the bytes and
/// leaves that semantic decision to the caller.
pub fn parse_trigger_user_spatial_stream(
    bytes: &[u8],
) -> Result<TriggerUserSpatialStreamInfo, TriggerParseError> {
    require(bytes, TRIGGER_USER_INFO_LEN)?;
    let starting_spatial_stream_encoding = (bytes[3] >> 2) & 0x07;
    let spatial_stream_count_encoding = bytes[3] >> 5;
    let target_rssi_encoding = bytes[4] & 0x7f;
    Ok(TriggerUserSpatialStreamInfo {
        aid12: u16::from(bytes[0]) | (u16::from(bytes[1] & 0x0f) << 8),
        ru_allocation_region: bytes[1] & 0x10 != 0,
        ru_allocation: (bytes[1] >> 5) | ((bytes[2] & 0x0f) << 3),
        coding_type: bytes[2] & 0x10 != 0,
        mcs: (bytes[2] >> 5) | ((bytes[3] & 0x01) << 3),
        dcm: bytes[3] & 0x02 != 0,
        starting_spatial_stream_encoding,
        starting_spatial_stream: starting_spatial_stream_encoding + 1,
        spatial_stream_count_encoding,
        spatial_stream_count: spatial_stream_count_encoding + 1,
        target_rssi_encoding,
        target_rssi: TriggerTargetRssi::from_user_encoding(target_rssi_encoding),
        reserved: bytes[4] & 0x80 != 0,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerBasicDependentInfo {
    pub mpdu_mu_spacing_factor: u8,
    pub tid_aggregation_limit: u8,
    pub reserved: bool,
    pub preferred_access_category: u8,
}

/// SOURCE(esp32s31)\[BLOB_LIBPP_DBG_DUMP_TRIG_BASIC_DEPENDENT]: complete
/// `libpp.a[hal_debug.o]::dbg_dump_trig_basic_dependent`.
pub fn parse_trigger_basic_dependent(
    bytes: &[u8],
) -> Result<TriggerBasicDependentInfo, TriggerParseError> {
    require(bytes, TRIGGER_BASIC_DEPENDENT_LEN)?;
    Ok(TriggerBasicDependentInfo {
        mpdu_mu_spacing_factor: bytes[0] & 0x03,
        tid_aggregation_limit: (bytes[0] >> 2) & 0x07,
        reserved: bytes[0] & 0x20 != 0,
        preferred_access_category: bytes[0] >> 6,
    })
}

/// SOURCE(esp32s31)\[BLOB_LIBPP_DBG_DUMP_TRIG_BFRP_DEPENDENT]: complete
/// `libpp.a[hal_debug.o]::dbg_dump_trig_bfrp_dependent`.
pub fn parse_trigger_bfrp_dependent(bytes: &[u8]) -> Result<u8, TriggerParseError> {
    require(bytes, TRIGGER_BFRP_DEPENDENT_LEN)?;
    Ok(bytes[0])
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerMuBarDependentInfo {
    pub bar_control: u16,
    pub ack_policy: bool,
    pub bar_type: u8,
    pub tid: u8,
    pub bar_information: u16,
    pub starting_sequence_number: SequenceNumber,
}

/// SOURCE(esp32s31)\[BLOB_LIBPP_DBG_DUMP_TRIG_MUBAR_DEPENDENT]: complete
/// `libpp.a[hal_debug.o]::dbg_dump_trig_mubar_dependent`.
pub fn parse_trigger_mu_bar_dependent(
    bytes: &[u8],
) -> Result<TriggerMuBarDependentInfo, TriggerParseError> {
    require(bytes, TRIGGER_MU_BAR_DEPENDENT_LEN)?;
    let bar_control = u16::from_le_bytes([bytes[0], bytes[1]]);
    let bar_information = u16::from_le_bytes([bytes[2], bytes[3]]);
    Ok(TriggerMuBarDependentInfo {
        bar_control,
        ack_policy: bar_control & 0x0001 != 0,
        bar_type: ((bar_control >> 1) & 0x0f) as u8,
        tid: (bar_control >> 12) as u8,
        bar_information,
        starting_sequence_number: SequenceNumber::from_sequence_control(bar_information),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerNfrpUserInfo {
    pub starting_aid: u16,
    pub reserved_9: u16,
    pub feedback_type: u8,
    pub reserved_7: u8,
    pub uplink_target_rssi: u8,
    pub multiplexing: bool,
}

/// SOURCE(esp32s31)\[BLOB_LIBPP_DBG_DUMP_TRIG_NFRP_USER]: complete
/// `libpp.a[hal_debug.o]::dbg_dump_trig_nfrp_user`.
pub fn parse_trigger_nfrp_user(bytes: &[u8]) -> Result<TriggerNfrpUserInfo, TriggerParseError> {
    require(bytes, TRIGGER_NFRP_USER_INFO_LEN)?;
    Ok(TriggerNfrpUserInfo {
        starting_aid: u16::from(bytes[0]) | (u16::from(bytes[1] & 0x0f) << 8),
        reserved_9: u16::from(bytes[1] >> 4) | (u16::from(bytes[2] & 0x1f) << 4),
        feedback_type: (bytes[2] >> 5) | ((bytes[3] & 0x01) << 3),
        reserved_7: bytes[3] >> 1,
        uplink_target_rssi: bytes[4] & 0x7f,
        multiplexing: bytes[4] & 0x80 != 0,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TriggerResponseSchedulingControl {
    pub control_id: u8,
    pub uplink_data_symbols: u8,
    pub ru_allocation_region: bool,
    pub ru_allocation: u8,
    pub ap_tx_power_encoding: u8,
    pub ap_tx_power: TriggerApTxPower,
    pub uplink_target_rssi_encoding: u8,
    pub uplink_target_rssi: TriggerTargetRssi,
    pub mcs: u8,
    pub trailing_reserved: bool,
}

/// Parse the 32-bit Trigger Response Scheduling HE-control information.
///
/// SOURCE(esp32s31)\[BLOB_LIBPP_DBG_DUMP_TRS_CONTROL]: complete
/// `libpp.a[hal_debug.o]::dbg_dump_trs_control`.
pub fn parse_trigger_response_scheduling_control(
    bytes: &[u8],
) -> Result<TriggerResponseSchedulingControl, TriggerParseError> {
    let bits = parse_he_control_word(bytes)?;
    let ap_tx_power_encoding = field(u64::from(bits), 19, 0x1f) as u8;
    let uplink_target_rssi_encoding = field(u64::from(bits), 24, 0x1f) as u8;
    Ok(TriggerResponseSchedulingControl {
        control_id: field(u64::from(bits), 0, 0x3f) as u8,
        uplink_data_symbols: field(u64::from(bits), 6, 0x1f) as u8,
        ru_allocation_region: bit(u64::from(bits), 11),
        ru_allocation: field(u64::from(bits), 12, 0x7f) as u8,
        ap_tx_power_encoding,
        ap_tx_power: TriggerApTxPower::from_trs_encoding(ap_tx_power_encoding),
        uplink_target_rssi_encoding,
        uplink_target_rssi: TriggerTargetRssi::from_trs_encoding(uplink_target_rssi_encoding),
        mcs: field(u64::from(bits), 29, 0x03) as u8,
        trailing_reserved: bit(u64::from(bits), 31),
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct UplinkPowerHeadroomControl {
    pub control_id: u8,
    pub uplink_power_headroom: u8,
    pub minimum_transmit_power: bool,
    pub reserved: u8,
    pub unparsed_upper_bits: u32,
}

/// Parse the fields printed from a 32-bit UPH HE-control information word.
///
/// SOURCE(esp32s31)\[BLOB_LIBPP_DBG_DUMP_UPH_CONTROL]: complete
/// `libpp.a[hal_debug.o]::dbg_dump_uph_control`. The blob only names
/// bits 6:13; bits 14:31 remain explicitly unparsed instead of being guessed.
pub fn parse_uplink_power_headroom_control(
    bytes: &[u8],
) -> Result<UplinkPowerHeadroomControl, TriggerParseError> {
    let bits = parse_he_control_word(bytes)?;
    Ok(UplinkPowerHeadroomControl {
        control_id: (bits & 0x3f) as u8,
        uplink_power_headroom: ((bits >> 6) & 0x1f) as u8,
        minimum_transmit_power: bits & (1 << 11) != 0,
        reserved: ((bits >> 12) & 0x03) as u8,
        unparsed_upper_bits: bits >> 14,
    })
}

fn parse_he_control_word(bytes: &[u8]) -> Result<u32, TriggerParseError> {
    require(bytes, HE_CONTROL_INFO_LEN)?;
    Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

const fn require(bytes: &[u8], required: usize) -> Result<(), TriggerParseError> {
    if bytes.len() < required {
        Err(TriggerParseError::Truncated { required })
    } else {
        Ok(())
    }
}

const fn field(bits: u64, shift: u32, mask: u64) -> u64 {
    (bits >> shift) & mask
}

const fn bit(bits: u64, shift: u32) -> bool {
    field(bits, shift, 1) != 0
}

#[cfg(test)]
mod tests;
