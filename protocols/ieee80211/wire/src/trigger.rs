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
//! Fixed-width fields use standard-clause declarations; the reviewed vendor
//! decoders remain independent behavior evidence. Derived units such as dBm
//! are exposed separately from their wire encodings.
//!
//! Received bytes reach this module; nothing here panics on them.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

use crate::frame::FrameControlFields;

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

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 9.3.1.22 (Trigger Type subfield).
    pub enum TriggerType: u8 (from_raw, raw) {
        Basic = 0,
        BeamformingReportPoll = 1,
        MultiUserBlockAckRequest = 2,
        MultiUserRequestToSend = 3,
        BufferStatusReportPoll = 4,
        GroupcastMultiUserBlockAckRequest = 5,
        BandwidthQueryReportPoll = 6,
        NgpaFeedbackReportPoll = 7,
    }
}

impl TriggerType {
    // These public value conversions have always selected the low four bits.
    // The raw conversions generated above retain unknown encodings verbatim.
    pub const fn from_encoding(value: u8) -> Self {
        Self::from_raw(TriggerCommonFields::from_bytes([value, 0, 0, 0, 0, 0, 0, 0]).trigger_type())
    }
    pub const fn encoding(self) -> u8 {
        TriggerCommonFields::from_bytes([self.raw(), 0, 0, 0, 0, 0, 0, 0]).trigger_type()
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
        match GiLtfValue::from_bytes([value]).value() {
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

        let mut fields = TriggerCommonFields::from_bytes([0; 8]);
        fields = fields
            .with_trigger_type(
                TriggerCommonFields::from_bytes([trigger_type.encoding(), 0, 0, 0, 0, 0, 0, 0])
                    .trigger_type(),
            )
            .map_err(|_| TriggerEncodeError::UplinkLengthOutOfRange)?;
        fields = fields
            .with_uplink_length(self.uplink_length)
            .map_err(|_| TriggerEncodeError::UplinkLengthOutOfRange)?;
        fields = fields.with_more_trigger_frames(self.more_trigger_frames);
        fields = fields.with_carrier_sense_required(self.carrier_sense_required);
        fields = fields
            .with_uplink_bandwidth_encoding(self.uplink_bandwidth_encoding)
            .map_err(|_| TriggerEncodeError::UplinkBandwidthOutOfRange)?;
        fields = fields
            .with_gi_ltf(self.gi_ltf.encoding())
            .map_err(|_| TriggerEncodeError::UplinkBandwidthOutOfRange)?;
        fields = fields.with_mu_mimo_ltf_mode(self.mu_mimo_ltf_mode);
        fields = fields
            .with_he_ltf_symbols_and_midamble_periodicity(
                self.he_ltf_symbols_and_midamble_periodicity,
            )
            .map_err(|_| TriggerEncodeError::HeLtfSymbolsOutOfRange)?;
        fields = fields.with_uplink_stbc(self.uplink_stbc);
        fields = fields.with_ldpc_extra_symbol_segment(self.ldpc_extra_symbol_segment);
        fields = fields
            .with_ap_tx_power_encoding(self.ap_tx_power_encoding)
            .map_err(|_| TriggerEncodeError::ApTxPowerOutOfRange)?;
        fields = fields
            .with_pre_fec_padding_factor_encoding(self.pre_fec_padding_factor_encoding)
            .map_err(|_| TriggerEncodeError::PreFecPaddingOutOfRange)?;
        fields = fields.with_packet_extension_disambiguity(self.packet_extension_disambiguity);
        fields = fields
            .with_uplink_spatial_reuse(self.uplink_spatial_reuse)
            .map_err(|_| TriggerEncodeError::HeSigA2ReservedOutOfRange)?;
        fields = fields.with_doppler(self.doppler);
        fields = fields
            .with_uplink_he_sig_a2_reserved(self.uplink_he_sig_a2_reserved)
            .map_err(|_| TriggerEncodeError::HeSigA2ReservedOutOfRange)?;
        fields = fields.with_trailing_reserved(self.trailing_reserved);
        Ok(fields.to_bytes())
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

        let mut fields = TriggerScheduledUserFields::from_bytes([0; 5]);
        fields = fields
            .with_aid12(self.association_id)
            .map_err(|_| TriggerEncodeError::AssociationIdOutOfRange)?;
        fields = fields.with_ru_allocation_region(self.ru_allocation_region);
        fields = fields
            .with_ru_allocation(self.ru_allocation)
            .map_err(|_| TriggerEncodeError::RuAllocationOutOfRange)?;
        fields = fields.with_coding_type(self.coding_type);
        fields = fields
            .with_mcs(self.mcs)
            .map_err(|_| TriggerEncodeError::McsOutOfRange)?;
        fields = fields.with_dcm(self.dcm);
        fields = fields
            .with_starting_spatial_stream_encoding(self.starting_spatial_stream_encoding)
            .map_err(|_| TriggerEncodeError::StartingSpatialStreamOutOfRange)?;
        fields = fields
            .with_spatial_stream_count_encoding(self.spatial_stream_count_encoding)
            .map_err(|_| TriggerEncodeError::SpatialStreamCountOutOfRange)?;
        fields = fields
            .with_target_rssi_encoding(self.target_rssi_encoding)
            .map_err(|_| TriggerEncodeError::TargetRssiOutOfRange)?;
        fields = fields.with_reserved(self.reserved);
        Ok(fields.to_bytes())
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
        let mut fields = TriggerBasicFields::from_bytes([0; 1]);
        fields = fields
            .with_mpdu_mu_spacing_factor(self.mpdu_mu_spacing_factor)
            .map_err(|_| TriggerEncodeError::BasicDependentInfoOutOfRange)?;
        fields = fields
            .with_tid_aggregation_limit(self.tid_aggregation_limit)
            .map_err(|_| TriggerEncodeError::BasicDependentInfoOutOfRange)?;
        fields = fields.with_reserved(self.reserved);
        fields = fields
            .with_preferred_access_category(self.preferred_access_category)
            .map_err(|_| TriggerEncodeError::BasicDependentInfoOutOfRange)?;
        Ok(fields.to_bytes()[0])
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
        let fields: [&[u8]; 7] = [
            &0x0024_u16.to_le_bytes(),
            &self.duration.to_le_bytes(),
            &self.receiver_address,
            &self.transmitter_address,
            &common,
            &user,
            &[dependent],
        ];
        for (out, byte) in output.iter_mut().zip(fields.into_iter().flatten()) {
            *out = *byte;
        }
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
                one_based_index: raw.saturating_add(1),
            }),
            37..=40 => Some(Self::Narrow {
                resource_unit: HeResourceUnit::Ru52,
                one_based_index: raw.saturating_sub(36),
            }),
            53..=54 => Some(Self::Narrow {
                resource_unit: HeResourceUnit::Ru106,
                one_based_index: raw.saturating_sub(52),
            }),
            61..=62 => Some(Self::Narrow {
                resource_unit: HeResourceUnit::Ru242,
                one_based_index: raw.saturating_sub(60),
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

// IEEE Std 802.11ax-2021 9.3.1.22 / 9.2.4.6: each field has a finite
// signed dBm mapping. Arithmetic and narrowing are evaluated at compilation.
macro_rules! dbm_values {
    ($value:expr, $offset:literal, $scale:literal; $($code:literal),+; $last:expr) => {
        match $value.get() {
            $($code => Self::Dbm(const {
                let dbm = ($code as i16 - $offset) * $scale;
                assert!(dbm >= i8::MIN as i16 && dbm <= i8::MAX as i16);
                dbm as i8
            }),)+
            _ => $last,
        }
    };
}

impl TriggerTargetRssi {
    const fn from_user_encoding(value: oer_wire::U8<7>) -> Self {
        dbm_values!(value, 110, 1;
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
            16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
            32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47,
            48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62, 63,
            64, 65, 66, 67, 68, 69, 70, 71, 72, 73, 74, 75, 76, 77, 78, 79,
            80, 81, 82, 83, 84, 85, 86, 87, 88, 89, 90, 91, 92, 93, 94, 95,
            96, 97, 98, 99, 100, 101, 102, 103, 104, 105, 106, 107, 108, 109, 110, 111,
            112, 113, 114, 115, 116, 117, 118, 119, 120, 121, 122, 123, 124, 125, 126
            ; Self::Reserved
        )
    }
    const fn from_trs_encoding(value: oer_wire::U8<5>) -> Self {
        dbm_values!(value, 45, 2;
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
            16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30
            ; Self::Reserved
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TriggerApTxPower {
    Dbm(i8),
    Reserved,
}

impl TriggerApTxPower {
    const fn from_common_encoding(value: oer_wire::U8<6>) -> Self {
        dbm_values!(value, 20, 1;
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
            16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31,
            32, 33, 34, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47,
            48, 49, 50, 51, 52, 53, 54, 55, 56, 57, 58, 59, 60, 61, 62
            ; Self::Dbm(43)
        )
    }
    const fn from_trs_encoding(value: oer_wire::U8<5>) -> Self {
        dbm_values!(value, 10, 2;
            0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15,
            16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30
            ; Self::Reserved
        )
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
    pub user_info: &'a [u8; TRIGGER_USER_INFO_LEN],
    /// Basic contributes one byte and MU-BAR contributes four; other
    /// instruction-proven layouts expose an empty slice.
    pub dependent_info: &'a [u8],
}

impl TriggerUserField<'_> {
    pub const fn aid12(&self) -> u16 {
        trigger_user_prefix(self.user_info).aid12()
    }

    pub const fn ru_allocation(&self) -> u8 {
        trigger_user_prefix(self.user_info).ru_allocation()
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
        let mut reader = oer_wire::Reader::new(self.remaining);
        let Ok(user_info) = reader.array::<TRIGGER_USER_INFO_LEN>() else {
            return self.fail(TriggerParseError::Truncated {
                required: TRIGGER_USER_INFO_LEN,
            });
        };
        let aid12 = trigger_user_prefix(user_info).aid12();
        let ru_allocation = trigger_user_prefix(user_info).ru_allocation();
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
        let required = TRIGGER_USER_INFO_LEN.saturating_add(dependent_len);
        let Ok(dependent_info) = reader.take(dependent_len) else {
            return self.fail(TriggerParseError::Truncated { required });
        };
        let rest = reader.remaining();
        let user = TriggerUserField {
            user_info,
            dependent_info,
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
    let truncated = TriggerParseError::Truncated {
        required: TRIGGER_FRAME_MIN_LEN,
    };
    let mut reader = oer_wire::Reader::new(frame);
    let frame_control = reader.read().map_err(|_| truncated)?;
    let duration = reader.read().map_err(|_| truncated)?;
    let receiver_address = reader.read().map_err(|_| truncated)?;
    let transmitter_address = reader.read().map_err(|_| truncated)?;
    let common = reader
        .array::<TRIGGER_COMMON_INFO_LEN>()
        .map_err(|_| truncated)?;
    let user_info_and_padding = reader.remaining();
    let frame_control = u16::from_le_bytes(frame_control);
    if FrameControlFields::from_bytes(frame_control.to_le_bytes()).type_and_subtype() != 9 {
        return Err(TriggerParseError::NotTriggerFrame);
    }
    Ok(TriggerFrame {
        frame_control,
        duration: u16::from_le_bytes(duration),
        receiver_address,
        transmitter_address,
        common: parse_trigger_common_info(common)?,
        user_info_and_padding,
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
    let fields = TriggerCommonFields::from_bytes(*chunk(bytes)?);
    let ap_tx_power_encoding = fields.ap_tx_power_encoding();
    let pre_fec_padding_factor_encoding = fields.pre_fec_padding_factor_encoding();
    Ok(TriggerCommonInfo {
        trigger_type: TriggerType::from_encoding(fields.trigger_type()),
        uplink_length: fields.uplink_length(),
        more_trigger_frames: fields.more_trigger_frames(),
        carrier_sense_required: fields.carrier_sense_required(),
        uplink_bandwidth_encoding: fields.uplink_bandwidth_encoding(),
        gi_ltf: TriggerGiLtf::from_encoding(fields.gi_ltf()),
        mu_mimo_ltf_mode: fields.mu_mimo_ltf_mode(),
        he_ltf_symbols_and_midamble_periodicity: fields.he_ltf_symbols_and_midamble_periodicity(),
        uplink_stbc: fields.uplink_stbc(),
        ldpc_extra_symbol_segment: fields.ldpc_extra_symbol_segment(),
        ap_tx_power_encoding,
        ap_tx_power: TriggerApTxPower::from_common_encoding(oer_wire::U8::from_low_bits(
            ap_tx_power_encoding,
        )),
        pre_fec_padding_factor_encoding,
        pre_fec_padding_factor: if pre_fec_padding_factor_encoding == 0 {
            4
        } else {
            pre_fec_padding_factor_encoding
        },
        packet_extension_disambiguity: fields.packet_extension_disambiguity(),
        uplink_spatial_reuse: fields.uplink_spatial_reuse(),
        doppler: fields.doppler(),
        uplink_he_sig_a2_reserved: fields.uplink_he_sig_a2_reserved(),
        trailing_reserved: fields.trailing_reserved(),
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
    let fields = TriggerRandomAccessUserFields::from_bytes(*chunk(bytes)?);
    let target_rssi_encoding = fields.target_rssi_encoding();
    Ok(TriggerUserRuInfo {
        aid12: fields.aid12(),
        ru_allocation_region: fields.ru_allocation_region(),
        ru_allocation: fields.ru_allocation(),
        coding_type: fields.coding_type(),
        mcs: fields.mcs(),
        dcm: fields.dcm(),
        number_of_ra_ru: fields.number_of_ra_ru(),
        more_ra_ru: fields.more_ra_ru(),
        target_rssi_encoding,
        target_rssi: TriggerTargetRssi::from_user_encoding(oer_wire::U8::from_low_bits(
            target_rssi_encoding,
        )),
        reserved: fields.reserved(),
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
    let fields = TriggerScheduledUserFields::from_bytes(*chunk(bytes)?);
    let starting_spatial_stream_encoding = fields.starting_spatial_stream_encoding();
    let spatial_stream_count_encoding = fields.spatial_stream_count_encoding();
    let target_rssi_encoding = fields.target_rssi_encoding();
    Ok(TriggerUserSpatialStreamInfo {
        aid12: fields.aid12(),
        ru_allocation_region: fields.ru_allocation_region(),
        ru_allocation: fields.ru_allocation(),
        coding_type: fields.coding_type(),
        mcs: fields.mcs(),
        dcm: fields.dcm(),
        starting_spatial_stream_encoding,
        starting_spatial_stream: starting_spatial_stream_encoding.saturating_add(1),
        spatial_stream_count_encoding,
        spatial_stream_count: spatial_stream_count_encoding.saturating_add(1),
        target_rssi_encoding,
        target_rssi: TriggerTargetRssi::from_user_encoding(oer_wire::U8::from_low_bits(
            target_rssi_encoding,
        )),
        reserved: fields.reserved(),
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
    let fields = TriggerBasicFields::from_bytes(*chunk(bytes)?);
    Ok(TriggerBasicDependentInfo {
        mpdu_mu_spacing_factor: fields.mpdu_mu_spacing_factor(),
        tid_aggregation_limit: fields.tid_aggregation_limit(),
        reserved: fields.reserved(),
        preferred_access_category: fields.preferred_access_category(),
    })
}

/// SOURCE(esp32s31)\[BLOB_LIBPP_DBG_DUMP_TRIG_BFRP_DEPENDENT]: complete
/// `libpp.a[hal_debug.o]::dbg_dump_trig_bfrp_dependent`.
pub fn parse_trigger_bfrp_dependent(bytes: &[u8]) -> Result<u8, TriggerParseError> {
    let [b0] = *chunk::<TRIGGER_BFRP_DEPENDENT_LEN>(bytes)?;
    Ok(b0)
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
    let [b0, b1, b2, b3] = *chunk::<TRIGGER_MU_BAR_DEPENDENT_LEN>(bytes)?;
    let bar_control = u16::from_le_bytes([b0, b1]);
    let bar_information = u16::from_le_bytes([b2, b3]);
    Ok(TriggerMuBarDependentInfo {
        bar_control,
        ack_policy: BarControlFields::from_bytes(bar_control.to_le_bytes()).ack_policy(),
        bar_type: BarControlFields::from_bytes(bar_control.to_le_bytes()).bar_type(),
        tid: BarControlFields::from_bytes(bar_control.to_le_bytes())
            .tid()
            .get(),
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
    let fields = TriggerNfrpFields::from_bytes(*chunk(bytes)?);
    Ok(TriggerNfrpUserInfo {
        starting_aid: fields.starting_aid(),
        reserved_9: fields.reserved_9(),
        feedback_type: fields.feedback_type(),
        reserved_7: fields.reserved_7(),
        uplink_target_rssi: fields.uplink_target_rssi(),
        multiplexing: fields.multiplexing(),
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
    let fields = TrsFields::from_bytes(*chunk(bytes)?);
    let ap_tx_power_encoding = fields.ap_tx_power_encoding();
    let uplink_target_rssi_encoding = fields.uplink_target_rssi_encoding();
    Ok(TriggerResponseSchedulingControl {
        control_id: fields.control_id(),
        uplink_data_symbols: fields.uplink_data_symbols(),
        ru_allocation_region: fields.ru_allocation_region(),
        ru_allocation: fields.ru_allocation(),
        ap_tx_power_encoding: fields.ap_tx_power_encoding(),
        uplink_target_rssi_encoding: fields.uplink_target_rssi_encoding(),
        mcs: fields.mcs(),
        trailing_reserved: fields.trailing_reserved(),
        ap_tx_power: TriggerApTxPower::from_trs_encoding(oer_wire::U8::from_low_bits(
            ap_tx_power_encoding,
        )),
        uplink_target_rssi: TriggerTargetRssi::from_trs_encoding(oer_wire::U8::from_low_bits(
            uplink_target_rssi_encoding,
        )),
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
    let fields = UphFields::from_bytes(*chunk(bytes)?);
    Ok(UplinkPowerHeadroomControl {
        control_id: fields.control_id(),
        uplink_power_headroom: fields.uplink_power_headroom(),
        minimum_transmit_power: fields.minimum_transmit_power(),
        reserved: fields.reserved(),
        unparsed_upper_bits: fields.unparsed_upper_bits(),
    })
}

fn chunk<const N: usize>(bytes: &[u8]) -> Result<&[u8; N], TriggerParseError> {
    oer_wire::Reader::new(bytes)
        .array()
        .map_err(|_| TriggerParseError::Truncated { required: N })
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 9.3.1.22 (Trigger frame format).
    pub struct TriggerCommonFields[8] (little) {
        pub trigger_type, with_trigger_type: u8 = 0..4;
        pub uplink_length, with_uplink_length: u16 = 4..16;
        pub more_trigger_frames, with_more_trigger_frames: bool = 16..17;
        pub carrier_sense_required, with_carrier_sense_required: bool = 17..18;
        pub uplink_bandwidth_encoding, with_uplink_bandwidth_encoding: u8 = 18..20;
        pub gi_ltf, with_gi_ltf: u8 = 20..22;
        pub mu_mimo_ltf_mode, with_mu_mimo_ltf_mode: bool = 22..23;
        pub he_ltf_symbols_and_midamble_periodicity, with_he_ltf_symbols_and_midamble_periodicity: u8 = 23..26;
        pub uplink_stbc, with_uplink_stbc: bool = 26..27;
        pub ldpc_extra_symbol_segment, with_ldpc_extra_symbol_segment: bool = 27..28;
        pub ap_tx_power_encoding, with_ap_tx_power_encoding: u8 = 28..34;
        pub pre_fec_padding_factor_encoding, with_pre_fec_padding_factor_encoding: u8 = 34..36;
        pub packet_extension_disambiguity, with_packet_extension_disambiguity: bool = 36..37;
        pub uplink_spatial_reuse, with_uplink_spatial_reuse: u16 = 37..53;
        pub doppler, with_doppler: bool = 53..54;
        pub uplink_he_sig_a2_reserved, with_uplink_he_sig_a2_reserved: u16 = 54..63;
        pub trailing_reserved, with_trailing_reserved: bool = 63..64;
    }
}

oer_wire::bitfield! {
    /// Shared prefix of scheduled and random-access Trigger User Info.
    /// SOURCE: IEEE Std 802.11ax-2021, 9.3.1.22 (Trigger frame format).
    pub struct TriggerUserPrefix[3] (little) {
        pub aid12, with_aid12: u16 = 0..12;
        pub ru_allocation_region, with_ru_allocation_region: bool = 12..13;
        pub ru_allocation, with_ru_allocation: u8 = 13..20;
    }
}
oer_wire::bitfield! {
    /// Scheduled spatial-stream form of Trigger User Info.
    /// SOURCE: IEEE Std 802.11ax-2021, 9.3.1.22 (Trigger frame format).
    pub struct TriggerScheduledUserFields[5] (little) {
        pub aid12, with_aid12: u16 = 0..12;
        pub ru_allocation_region, with_ru_allocation_region: bool = 12..13;
        pub ru_allocation, with_ru_allocation: u8 = 13..20;
        pub coding_type, with_coding_type: bool = 20..21;
        pub mcs, with_mcs: u8 = 21..25;
        pub dcm, with_dcm: bool = 25..26;
        pub starting_spatial_stream_encoding, with_starting_spatial_stream_encoding: u8 = 26..29;
        pub spatial_stream_count_encoding, with_spatial_stream_count_encoding: u8 = 29..32;
        pub target_rssi_encoding, with_target_rssi_encoding: u8 = 32..39;
        pub reserved, with_reserved: bool = 39..40;
    }
}
oer_wire::bitfield! {
    /// Random-access RU form of Trigger User Info.
    /// SOURCE: IEEE Std 802.11ax-2021, 9.3.1.22 (Trigger frame format).
    pub struct TriggerRandomAccessUserFields[5] (little) {
        pub aid12, with_aid12: u16 = 0..12;
        pub ru_allocation_region, with_ru_allocation_region: bool = 12..13;
        pub ru_allocation, with_ru_allocation: u8 = 13..20;
        pub coding_type, with_coding_type: bool = 20..21;
        pub mcs, with_mcs: u8 = 21..25;
        pub dcm, with_dcm: bool = 25..26;
        pub number_of_ra_ru, with_number_of_ra_ru: u8 = 26..31;
        pub more_ra_ru, with_more_ra_ru: bool = 31..32;
        pub target_rssi_encoding, with_target_rssi_encoding: u8 = 32..39;
        pub reserved, with_reserved: bool = 39..40;
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 9.3.1.22 (Trigger frame format).
    pub struct TriggerBasicFields[1] (little) {
        pub mpdu_mu_spacing_factor, with_mpdu_mu_spacing_factor: u8 = 0..2;
        pub tid_aggregation_limit, with_tid_aggregation_limit: u8 = 2..5;
        pub reserved, with_reserved: bool = 5..6;
        pub preferred_access_category, with_preferred_access_category: u8 = 6..8;
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 9.3.1.22 (Trigger frame format).
    pub struct TriggerNfrpFields[5] (little) {
        pub starting_aid, with_starting_aid: u16 = 0..12;
        pub reserved_9, with_reserved_9: u16 = 12..21;
        pub feedback_type, with_feedback_type: u8 = 21..25;
        pub reserved_7, with_reserved_7: u8 = 25..32;
        pub uplink_target_rssi, with_uplink_target_rssi: u8 = 32..39;
        pub multiplexing, with_multiplexing: bool = 39..40;
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 9.2.4.6 (HE variant HT Control, TRS Control).
    pub struct TrsFields[4] (little) {
        pub control_id, with_control_id: u8 = 0..6;
        pub uplink_data_symbols, with_uplink_data_symbols: u8 = 6..11;
        pub ru_allocation_region, with_ru_allocation_region: bool = 11..12;
        pub ru_allocation, with_ru_allocation: u8 = 12..19;
        pub ap_tx_power_encoding, with_ap_tx_power_encoding: u8 = 19..24;
        pub uplink_target_rssi_encoding, with_uplink_target_rssi_encoding: u8 = 24..29;
        pub mcs, with_mcs: u8 = 29..31;
        pub trailing_reserved, with_trailing_reserved: bool = 31..32;
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 9.2.4.6 (HE variant HT Control, UPH Control).
    pub struct UphFields[4] (little) {
        pub control_id, with_control_id: u8 = 0..6;
        pub uplink_power_headroom, with_uplink_power_headroom: u8 = 6..11;
        pub minimum_transmit_power, with_minimum_transmit_power: bool = 11..12;
        pub reserved, with_reserved: u8 = 12..14;
        pub unparsed_upper_bits, with_unparsed_upper_bits: u32 = 14..32;
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.3.1.8 (BlockAckReq frame).
    pub struct BarControlFields[2] (little) {
        pub ack_policy, with_ack_policy: bool = 0..1;
        pub bar_type, with_bar_type: u8 = 1..5;
        pub tid, with_tid: (u8) = 12..16;
    }
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11ax-2021, 9.3.1.22 (GI And HE-LTF Type subfield).
    pub struct GiLtfValue[1] (little) {
        pub value, with_value: u8 = 0..2;
    }
}

const fn trigger_user_prefix(bytes: &[u8; 5]) -> TriggerUserPrefix {
    let [first, second, third, ..] = *bytes;
    TriggerUserPrefix::from_bytes([first, second, third])
}

#[cfg(test)]
mod tests;
