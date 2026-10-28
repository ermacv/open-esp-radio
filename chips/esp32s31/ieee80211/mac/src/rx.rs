//! RX descriptor metadata decoding and bounded raw MPDU extraction.

pub use oer_esp32s31_hal::ieee80211::dma::{
    rx_dma::{
        RxDma, RxDmaBinding, RxDmaCursorObservation, RxDmaNextDescriptor, RxDmaReloadSettled,
        RxDmaWalkerEnabled, RxDmaWalkerStopped,
    },
    rx_ring::{
        RxCompletedDescriptor, RxCompletedDescriptorFrontier, RxCompletedUnit,
        RxCompletedUnitFrontier, RxDescriptorSnapshot, RxDmaArenaState, RxDmaBufferAddresses,
        RxFrozenCursor, RxLiveAppend, RxObservedMask, RxReloadObservation, RxRingError,
        RxRingHalted, RxRingLive, RxRingStopped, RxRingTopologySnapshot, RxSegment,
    },
};
use oer_esp32s31_hal::layouts::{WifiRxControlPrefix, WifiRxControlTail};

/// The MAC's receive-buffer starvation counter (`RX_STATISTICS.BUFFER_FULL`),
/// sampled at RX service boundaries for telemetry. It never participates in
/// descriptor ownership, and it is a MAC statistics register, not one of the
/// RX DMA engine's.
pub trait RxBufferFullCounter {
    /// The counter, or `None` where the hardware has none.
    fn buffer_full_count(&mut self) -> Option<u16>;
}

impl RxBufferFullCounter for oer_esp32s31_hal::ieee80211::mac::WifiMacHal<'_> {
    fn buffer_full_count(&mut self) -> Option<u16> {
        Some(self.rx_buffer_full_count())
    }
}

use oer_ieee80211_port::MacRxMetadata;
use oer_ieee80211_wire::ccmp::CcmpHeader;
use oer_ieee80211_wire::frame::{FrameControlFields, FrameType};
use oer_ieee80211_wire::he::HeMuSigBUser;
use oer_ieee80211_wire::ht::HtDuplicateMcs32;
use oer_ieee80211_wire::sequence::SequenceControl;
use oer_ieee80211_wire::signal::{
    HeMuSigA1, HeMuSigA2, HeSuSigA1, HeSuSigA2, HeTriggerBasedSigA1, HeTriggerBasedSigA2, HtSig,
};
use sig_b::{
    He20MuSigBMimoStreamError, He20MuSigBMimoUsers, He20MuSigBNonMimoStreamError,
    He20MuSigBNonMimoUsers,
};

pub const INGRESS_STRICT_RXEND: u32 = 0x01;
pub const INGRESS_STRICT_DUMP: u32 = 0x02;
pub const INGRESS_VALID_FLAGS: u32 = 0x03;

/// The fixed RX-control prefix's bytes ([`WifiRxControlPrefix`]).
pub const FIXED_PREFIX_SIZE: usize = WifiRxControlPrefix::SIZE;
/// The dynamic tail's bytes ([`WifiRxControlTail`]).
pub const DYNAMIC_TAIL_SIZE: usize = WifiRxControlTail::SIZE;
pub const PUBLIC_HEADER_SIZE: usize = FIXED_PREFIX_SIZE + DYNAMIC_TAIL_SIZE;
pub const FCS_SIZE: usize = 4;
pub const CCMP_HEADER_SIZE: usize = 8;
pub const CCMP_MIC_SIZE: usize = 8;

const MLME_HEADER_SIZE: usize = 24;
const CONTROL_HEADER_MIN_SIZE: usize = 10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxIngressConfig {
    pub ring_entry_limit: usize,
    pub csi_config: u32,
    pub flags: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxMpduFrame {
    pub length: usize,
    pub tail_offset: usize,
    pub signal_length: u16,
    pub dump_length: u16,
    pub rxend_state: u8,
    pub rx_state: u8,
    pub internal_state: u8,
    pub dump_length_matches: bool,
}

/// Geometry decoded from the first completed S31 RX descriptor.
///
/// The fields mirror the recovered `wDev_IndicateFrame` boundary: the descriptor
/// publishes its received byte count independently from the 14-bit on-air
/// `sig_len`, and the latter includes the four-byte FCS. Keeping both values
/// visible is necessary for protected RX, where the MAC may consume cipher
/// trailer bytes before publishing the DMA view.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxFirstSegmentLayout {
    pub received_length: usize,
    pub tail_offset: usize,
    pub frame_offset: usize,
    pub signal_length: usize,
    pub dump_length: usize,
    pub expected_frame_length: usize,
    pub available_frame_bytes: usize,
    pub frame_shortfall: usize,
    pub rxend_state: u8,
    pub rx_state: u8,
    pub internal_state: u8,
    pub dump_length_matches: bool,
}

/// Stable radio fields from the public 64-byte ESP32-S31 RX-control prefix.
///
/// SOURCE\[ESP_WIFI_SYS_S31_RXCTRL]: public ESP32-S31 `esp_wifi_rxctrl_t`. The packed ABI
/// places the five-bit `rate`
/// at byte 1, HE-SIGA1 at bytes 4..8, HE-SIGA2 at bytes 9..11 and the
/// four-bit `cur_bb_format` in the high nibble of byte 37.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxPhyInfo {
    pub rate: u8,
    pub bb_format: u8,
    pub he_siga1: u32,
    pub he_siga2: u16,
}

/// Typed IEEE 802.11n HT-SIG fields captured verbatim by the S31 RX prefix.
///
/// The prefix word called `he_siga1` by the public ABI overlaps the first
/// 32 bits of the format-specific signal field. For HT PPDUs those bits are
/// HT-SIG: MCS at 6:0, CBW at bit 7, Aggregation at bit 27 and Short GI at
/// bit 31. The format discriminator is therefore checked before decoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HtSignal {
    pub mcs: u8,
    pub channel_width_mhz: u8,
    pub aggregation: bool,
    pub short_guard_interval: bool,
}

/// Geometry-aware classification of an HT-SIG MCS32 observation.
///
/// This is metadata normalization, not a claim that the local PHY has been
/// qualified to receive MCS32 on air. The invalid-width case remains visible
/// because collapsing it into `None` would make a malformed HT20/MCS32 vector
/// indistinguishable from every ordinary MCS.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HtDuplicateRxClassification {
    NotMcs32,
    Mismatch { channel_width_mhz: u8 },
    Ht40(HtDuplicateMcs32),
}

impl HtSignal {
    pub const fn ht_duplicate_mcs32_classification(self) -> HtDuplicateRxClassification {
        if self.mcs != HtDuplicateMcs32::INDEX {
            HtDuplicateRxClassification::NotMcs32
        } else if self.channel_width_mhz == 40 {
            HtDuplicateRxClassification::Ht40(HtDuplicateMcs32::new())
        } else {
            HtDuplicateRxClassification::Mismatch {
                channel_width_mhz: self.channel_width_mhz,
            }
        }
    }

    /// Decode the special MCS32 selector only with its required HT40 geometry.
    pub const fn ht_duplicate_mcs32(self) -> Option<HtDuplicateMcs32> {
        match self.ht_duplicate_mcs32_classification() {
            HtDuplicateRxClassification::Ht40(mcs) => Some(mcs),
            HtDuplicateRxClassification::NotMcs32
            | HtDuplicateRxClassification::Mismatch { .. } => None,
        }
    }
}

/// PHY format published in the S31 RX-control prefix.
///
/// SOURCE\[ESP_WIFI_SYS_S31_RX_BB_FORMAT]: ESP-IDF 4d80c1b2
/// `components/esp_wifi/include/esp_wifi_he_types.h::wifi_rx_bb_format_t`.
/// The explicit `Unknown` case preserves forward compatibility instead of
/// interpreting a new hardware value as one of the currently named formats.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RxBasebandFormat {
    Dot11b,
    Ofdm,
    Ht,
    Vht,
    HeSu,
    HeMu,
    HeExtendedRangeSu,
    HeTriggerBased,
    VhtMu,
    Unknown(u8),
}

// Reached by the RX-prefix decode: nothing here panics on received bits.
#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
impl RxBasebandFormat {
    pub const fn decode(raw: u8) -> Self {
        match raw {
            0 => Self::Dot11b,
            1 => Self::Ofdm,
            2 => Self::Ht,
            3 => Self::Vht,
            4 => Self::HeSu,
            5 => Self::HeMu,
            6 => Self::HeExtendedRangeSu,
            7 => Self::HeTriggerBased,
            11 => Self::VhtMu,
            value => Self::Unknown(value),
        }
    }

    pub const fn raw(self) -> u8 {
        match self {
            Self::Dot11b => 0,
            Self::Ofdm => 1,
            Self::Ht => 2,
            Self::Vht => 3,
            Self::HeSu => 4,
            Self::HeMu => 5,
            Self::HeExtendedRangeSu => 6,
            Self::HeTriggerBased => 7,
            Self::VhtMu => 11,
            Self::Unknown(value) => value,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeBandwidth {
    Mhz20,
    Mhz40,
    Mhz80,
    Mhz160Or80Plus80,
}

// Reached by the RX-prefix decode: nothing here panics on received bits.
#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
impl HeBandwidth {
    const fn decode(raw: u8) -> Self {
        match raw {
            0 => Self::Mhz20,
            1 => Self::Mhz40,
            2 => Self::Mhz80,
            _ => Self::Mhz160Or80Plus80,
        }
    }

    pub const fn mhz(self) -> u16 {
        match self {
            Self::Mhz20 => 20,
            Self::Mhz40 => 40,
            Self::Mhz80 => 80,
            Self::Mhz160Or80Plus80 => 160,
        }
    }
}

/// HE SU guard-interval and LTF encoding from HE-SIG-A1.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeGuardIntervalAndLtf {
    OneLtf800Ns,
    TwoLtf800Ns,
    TwoLtf1600Ns,
    FourLtf3200Ns,
}

// Reached by the RX-prefix decode: nothing here panics on received bits.
#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
impl HeGuardIntervalAndLtf {
    const fn decode(raw: u8) -> Self {
        match raw {
            0 => Self::OneLtf800Ns,
            1 => Self::TwoLtf800Ns,
            2 => Self::TwoLtf1600Ns,
            _ => Self::FourLtf3200Ns,
        }
    }

    pub const fn guard_interval_ns(self) -> u16 {
        match self {
            Self::OneLtf800Ns | Self::TwoLtf800Ns => 800,
            Self::TwoLtf1600Ns => 1_600,
            Self::FourLtf3200Ns => 3_200,
        }
    }

    pub const fn ltf_count(self) -> u8 {
        match self {
            Self::OneLtf800Ns => 1,
            Self::TwoLtf800Ns | Self::TwoLtf1600Ns => 2,
            Self::FourLtf3200Ns => 4,
        }
    }

    /// Two-bit IEEE HE-SIG-A1 GI/LTF encoding used by both RX metadata and
    /// the typed HE SU TX formatter.
    pub const fn encoding(self) -> u8 {
        match self {
            Self::OneLtf800Ns => 0,
            Self::TwoLtf800Ns => 1,
            Self::TwoLtf1600Ns => 2,
            Self::FourLtf3200Ns => 3,
        }
    }
}

/// Typed HE SU signal fields captured from the S31 RX metadata.
///
/// SOURCE\[ESP_WIFI_SYS_S31_HE_SU_SIG]: ESP-IDF 4d80c1b2
/// `components/esp_wifi/include/esp_private/esp_wifi_he_types_private.h`: complete
/// `esp_wifi_su_siga1_t` and packed `esp_wifi_su_siga2_t` layouts. The same
/// MCS/BW/GI bit positions are independently used by the qualified vendor
/// `libpp.a` `dbg_read_tx_sig` diagnostic. Complete
/// `libpp[hal_debug.o]::dbg_dump_rx_ppdu`, size `0xa36`, names
/// bits 25:23 `nsts_and_midamble_periodicity`; they must not be reported
/// unconditionally as the number of spatial streams.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeSuSignal {
    pub format: bool,
    pub beam_change: bool,
    pub uplink: bool,
    pub mcs: u8,
    pub dcm: bool,
    pub bss_color: u8,
    pub spatial_reuse: u8,
    pub bandwidth: HeBandwidth,
    pub guard_interval_and_ltf: HeGuardIntervalAndLtf,
    pub nsts_and_midamble_periodicity: u8,
    pub txop: u8,
    pub ldpc: bool,
    pub ldpc_extra_symbol: bool,
    pub stbc: bool,
    pub beamformed: bool,
    pub pre_fec_padding_factor: u8,
    pub packet_extension_disambiguity: bool,
    pub doppler: bool,
}

// Reached by the RX-prefix decode: nothing here panics on received bits.
#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
impl HeSuSignal {
    pub const fn decode(siga1: u32, siga2: u16) -> Self {
        let siga1 = HeSuSigA1::from_bytes(siga1.to_le_bytes());
        let siga2 = HeSuSigA2::from_bytes(siga2.to_le_bytes());
        Self {
            format: siga1.format(),
            beam_change: siga1.beam_change(),
            uplink: siga1.uplink(),
            mcs: siga1.mcs(),
            dcm: siga1.dcm(),
            bss_color: siga1.bss_color(),
            spatial_reuse: siga1.spatial_reuse(),
            bandwidth: HeBandwidth::decode(siga1.bandwidth()),
            guard_interval_and_ltf: HeGuardIntervalAndLtf::decode(siga1.guard_interval_and_ltf()),
            nsts_and_midamble_periodicity: siga1.nsts_and_midamble_periodicity(),
            txop: siga2.txop(),
            ldpc: siga2.ldpc(),
            ldpc_extra_symbol: siga2.ldpc_extra_symbol(),
            stbc: siga2.stbc(),
            beamformed: siga2.beamformed(),
            pre_fec_padding_factor: siga2.pre_fec_padding_factor(),
            packet_extension_disambiguity: siga2.packet_extension_disambiguity(),
            doppler: siga2.doppler(),
        }
    }

    /// Returns the number of space-time streams when the field is not shared
    /// with the Doppler midamble-periodicity encoding.
    ///
    /// With STBC enabled, one spatial stream occupies two space-time streams.
    /// For example, the live 1T1R RX vector `HE-SIG-A1=0x00e0591b`,
    /// `HE-SIG-A2=0x4a0c` has encoding one, hence two space-time streams but
    /// only one spatial stream.
    pub const fn space_time_stream_count(self) -> Option<u8> {
        if self.doppler {
            None
        } else {
            // The field is three bits; a wider value, which only a
            // hand-built signal can hold, has no count.
            self.nsts_and_midamble_periodicity.checked_add(1)
        }
    }

    /// Returns the spatial-stream count for a non-Doppler HE SU vector.
    pub const fn spatial_stream_count(self) -> Option<u8> {
        match self.space_time_stream_count() {
            Some(nsts) if self.stbc && nsts & 1 == 0 => Some(nsts / 2),
            Some(nsts) if !self.stbc => Some(nsts),
            _ => None,
        }
    }
}

/// HE MU bandwidth encoding from HE-SIG-A1.
///
/// Unlike the two-bit SU/TB field, the MU field is three bits wide. Values
/// four through seven are retained instead of being folded onto a known
/// bandwidth; that matters when inspecting metadata produced by a newer PHY
/// mode than this driver currently supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HeMuBandwidth {
    Mhz20,
    Mhz40,
    Mhz80,
    Mhz160Or80Plus80,
    Unknown(u8),
}

// Reached by the RX-prefix decode: nothing here panics on received bits.
#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
impl HeMuBandwidth {
    const fn decode(raw: u8) -> Self {
        match raw {
            0 => Self::Mhz20,
            1 => Self::Mhz40,
            2 => Self::Mhz80,
            3 => Self::Mhz160Or80Plus80,
            value => Self::Unknown(value),
        }
    }

    pub const fn raw(self) -> u8 {
        match self {
            Self::Mhz20 => 0,
            Self::Mhz40 => 1,
            Self::Mhz80 => 2,
            Self::Mhz160Or80Plus80 => 3,
            Self::Unknown(value) => {
                oer_ieee80211_wire::signal::HeMuBandwidthCode::from_bytes([value]).code()
            }
        }
    }

    pub const fn mhz(self) -> Option<u16> {
        match self {
            Self::Mhz20 => Some(20),
            Self::Mhz40 => Some(40),
            Self::Mhz80 => Some(80),
            Self::Mhz160Or80Plus80 => Some(160),
            Self::Unknown(_) => None,
        }
    }
}

/// Typed HE MU common signal fields captured from S31 RX metadata.
///
/// SOURCE\[ESP_WIFI_SYS_S31_HE_MU_SIG]: ESP-IDF 4d80c1b2
/// `components/esp_wifi/include/esp_private/esp_wifi_he_types_private.h`, complete
/// `esp_wifi_mu_siga1_t` and packed `esp_wifi_mu_siga2_t` layouts.
///
/// SOURCE\[BLOB_LIBPP_DBG_DUMP_RX_PPDU]: complete
/// `libpp[hal_debug.o]::dbg_dump_rx_ppdu`, size `0xa36`,
/// instructions `0x4b4..0x586`. The format-five branch independently loads
/// HE-SIG-A1 at RX-prefix offset `0x04` and HE-SIG-A2 at offsets `0x09..0x0a`,
/// then applies the masks represented below. In particular, it names the
/// SIG-B DCM bit, three-bit bandwidth, GI/LTF, STBC and padding fields.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeMuSignal {
    pub uplink: bool,
    pub sig_b_mcs: u8,
    pub sig_b_dcm: bool,
    pub bss_color: u8,
    pub spatial_reuse: u8,
    pub bandwidth: HeMuBandwidth,
    pub sig_b_symbols_or_mu_mimo_users_minus_one: u8,
    pub sig_b_compression: bool,
    pub guard_interval_and_ltf: HeGuardIntervalAndLtf,
    pub doppler: bool,
    pub txop: u8,
    pub nltf_and_midamble_periodicity: u8,
    pub ldpc_extra_symbol_segment: bool,
    pub stbc: bool,
    pub pre_fec_padding_factor: u8,
    pub packet_extension_disambiguity: bool,
}

impl HeMuSignal {
    #[deny(
        clippy::indexing_slicing,
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )]
    pub const fn decode(siga1: u32, siga2: u16) -> Self {
        let siga1 = HeMuSigA1::from_bytes(siga1.to_le_bytes());
        let siga2 = HeMuSigA2::from_bytes(siga2.to_le_bytes());
        Self {
            uplink: siga1.uplink(),
            sig_b_mcs: siga1.sig_b_mcs(),
            sig_b_dcm: siga1.sig_b_dcm(),
            bss_color: siga1.bss_color(),
            spatial_reuse: siga1.spatial_reuse(),
            bandwidth: HeMuBandwidth::decode(siga1.bandwidth()),
            sig_b_symbols_or_mu_mimo_users_minus_one: siga1
                .sig_b_symbols_or_mu_mimo_users_minus_one(),
            sig_b_compression: siga1.sig_b_compression(),
            guard_interval_and_ltf: HeGuardIntervalAndLtf::decode(siga1.guard_interval_and_ltf()),
            doppler: siga1.doppler(),
            txop: siga2.txop(),
            nltf_and_midamble_periodicity: siga2.nltf_and_midamble_periodicity(),
            ldpc_extra_symbol_segment: siga2.ldpc_extra_symbol_segment(),
            stbc: siga2.stbc(),
            pre_fec_padding_factor: siga2.pre_fec_padding_factor(),
            packet_extension_disambiguity: siga2.packet_extension_disambiguity(),
        }
    }

    /// One-based value printed by the complete blob diagnostic.
    pub const fn sig_b_symbols_or_mu_mimo_users(self) -> u8 {
        self.sig_b_symbols_or_mu_mimo_users_minus_one + 1
    }

    /// HE-LTF symbol count derived by the complete blob diagnostic.
    pub const fn he_ltf_symbols(self) -> u8 {
        if self.nltf_and_midamble_periodicity == 0 {
            1
        } else {
            self.nltf_and_midamble_periodicity * 2
        }
    }
}

/// Typed HE trigger-based common signal fields captured from RX metadata.
///
/// SOURCE: ESP-IDF 4d80c1b2
/// `components/esp_wifi/include/esp_private/esp_wifi_he_types_private.h:55-65,93-96`,
/// `esp_wifi_tb_siga1_t` and packed `esp_wifi_tb_siga2_t` layouts.
/// SOURCE\[BLOB_LIBPP_DBG_DUMP_RX_PPDU]: complete
/// `libpp[hal_debug.o]::dbg_dump_rx_ppdu`, instructions
/// `0x412..0x4b2`, independently decodes the same prefix words for baseband
/// format seven.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeTriggerBasedSignal {
    pub format: bool,
    pub bss_color: u8,
    pub spatial_reuse: [u8; 4],
    pub bandwidth: HeBandwidth,
    pub txop: u8,
}

// Reached by the RX-prefix decode: nothing here panics on received bits.
#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
impl HeTriggerBasedSignal {
    pub const fn decode(siga1: u32, siga2: u16) -> Self {
        let siga1 = HeTriggerBasedSigA1::from_bytes(siga1.to_le_bytes());
        let siga2 = HeTriggerBasedSigA2::from_bytes(siga2.to_le_bytes());
        Self {
            format: siga1.format(),
            bss_color: siga1.bss_color(),
            spatial_reuse: [
                siga1.spatial_reuse0(),
                siga1.spatial_reuse1(),
                siga1.spatial_reuse2(),
                siga1.spatial_reuse3(),
            ],
            bandwidth: HeBandwidth::decode(siga1.bandwidth()),
            txop: siga2.txop(),
        }
    }
}

/// Bounded HE-MU SIG-B view from one completed S31 RX buffer.
///
/// SOURCE\[BLOB_LIBPP_DBG_DUMP_RX_SIGB]: complete
/// `libpp[test_hal_rx_mu_sigb.o]::dbg_dump_rx_sigb`, size `0x1e6`.
/// The blob reads the exact offsets used by [`decode_rx_he_mu_sig_b`]:
/// bit length from `0x2a[7:5]` and `0x2b[6:0]`, presence at `0x2b[7]`,
/// the selected user at `0x28..0x2a`, common information at `0x2d..0x2f`,
/// RU metadata at `0x1a`/`0x1e`, and optional complete bytes from `0x38`.
///
/// The complete bytes borrow the descriptor buffer; this is the Rust
/// ownership equivalent of the blob's raw pointer walk. The decoder checks
/// the advertised length before constructing the slice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxHeMuSigBInfo<'a> {
    pub signal: HeMuSignal,
    pub bit_length: u16,
    pub common_info_raw: u32,
    pub selected_user_info_raw: u32,
    pub selected_user: HeMuSigBUser,
    pub ru_size: u8,
    pub ru_position: u8,
    pub complete_bytes: &'a [u8],
}

/// Why a complete RX HE-SIG-B stream cannot use the HE20 non-MU-MIMO layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RxHe20MuSigBUsersError {
    WiderOrUnknownBandwidth,
    MuMimoCompressed,
    CompleteStream(He20MuSigBNonMimoStreamError),
}

/// Why a complete RX HE-SIG-B stream cannot use the HE20 MU-MIMO layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RxHe20MuSigBMimoUsersError {
    WiderOrUnknownBandwidth,
    NotMuMimoCompressed,
    CompleteStream(He20MuSigBMimoStreamError),
}

impl RxHeMuSigBInfo<'_> {
    /// Iterates all users when the captured stream is the exact HE20
    /// non-MU-MIMO layout recovered from the blob.
    ///
    /// Wider bandwidths and compressed/MU-MIMO SIG-B fail closed because the
    /// complete test parser proves configuration-dependent layouts for them.
    pub fn he20_non_mimo_users(
        &self,
    ) -> Result<He20MuSigBNonMimoUsers<'_>, RxHe20MuSigBUsersError> {
        if self.signal.bandwidth != HeMuBandwidth::Mhz20 {
            return Err(RxHe20MuSigBUsersError::WiderOrUnknownBandwidth);
        }
        if self.signal.sig_b_compression {
            return Err(RxHe20MuSigBUsersError::MuMimoCompressed);
        }
        He20MuSigBNonMimoUsers::try_new(self.complete_bytes, self.bit_length)
            .map_err(RxHe20MuSigBUsersError::CompleteStream)
    }

    /// Iterates the at-most-four compressed/MU-MIMO users using the exact
    /// non-linear offsets from the complete blob test parser.
    pub fn he20_mimo_users(&self) -> Result<He20MuSigBMimoUsers<'_>, RxHe20MuSigBMimoUsersError> {
        if self.signal.bandwidth != HeMuBandwidth::Mhz20 {
            return Err(RxHe20MuSigBMimoUsersError::WiderOrUnknownBandwidth);
        }
        if !self.signal.sig_b_compression {
            return Err(RxHe20MuSigBMimoUsersError::NotMuMimoCompressed);
        }
        He20MuSigBMimoUsers::try_new(
            self.complete_bytes,
            self.bit_length,
            self.signal.sig_b_symbols_or_mu_mimo_users(),
        )
        .map_err(RxHe20MuSigBMimoUsersError::CompleteStream)
    }
}

pub fn decode_rx_he_mu_sig_b(buffer: &[u8]) -> Option<RxHeMuSigBInfo<'_>> {
    let prefix = WifiRxControlPrefix::new(buffer.first_chunk::<FIXED_PREFIX_SIZE>()?);
    let phy = decode_rx_phy_info(buffer)?;
    let signal = phy.he_mu_signal()?;
    let sig_b = prefix.he_sig_b();
    let bit_length = u16::from(sig_b.length_bytes()) * 8 + u16::from(sig_b.length_bits());
    let byte_length = usize::from(bit_length).div_ceil(8);
    let complete_bytes = if sig_b.present() {
        buffer.get(FIXED_PREFIX_SIZE..FIXED_PREFIX_SIZE.checked_add(byte_length)?)?
    } else {
        &[]
    };
    let selected_user_info_raw = sig_b.selected_user();
    Some(RxHeMuSigBInfo {
        signal,
        bit_length,
        common_info_raw: prefix.he_mu_common().common_info(),
        selected_user_info_raw,
        selected_user: HeMuSigBUser::decode(selected_user_info_raw, signal.sig_b_compression),
        ru_size: prefix.he_mu_ru_size().ru_size(),
        ru_position: prefix.he_mu_ru_position().ru_position(),
        complete_bytes,
    })
}

// Reached by the RX-prefix decode: nothing here panics on received bits.
#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
impl RxPhyInfo {
    pub const fn baseband_format(self) -> RxBasebandFormat {
        RxBasebandFormat::decode(self.bb_format)
    }

    pub const fn he_su_signal(self) -> Option<HeSuSignal> {
        match self.baseband_format() {
            RxBasebandFormat::HeSu | RxBasebandFormat::HeExtendedRangeSu => {
                Some(HeSuSignal::decode(self.he_siga1, self.he_siga2))
            }
            _ => None,
        }
    }

    pub const fn he_mu_signal(self) -> Option<HeMuSignal> {
        match self.baseband_format() {
            RxBasebandFormat::HeMu => Some(HeMuSignal::decode(self.he_siga1, self.he_siga2)),
            _ => None,
        }
    }

    pub const fn he_trigger_based_signal(self) -> Option<HeTriggerBasedSignal> {
        match self.baseband_format() {
            RxBasebandFormat::HeTriggerBased => {
                Some(HeTriggerBasedSignal::decode(self.he_siga1, self.he_siga2))
            }
            _ => None,
        }
    }

    pub const fn ht_signal(self) -> Option<HtSignal> {
        match self.baseband_format() {
            RxBasebandFormat::Ht => {
                let signal = HtSig::from_bytes(self.he_siga1.to_le_bytes());
                Some(HtSignal {
                    mcs: signal.mcs(),
                    channel_width_mhz: if signal.width40() { 40 } else { 20 },
                    aggregation: signal.aggregation(),
                    short_guard_interval: signal.short_gi(),
                })
            }
            _ => None,
        }
    }

    /// HT-SIG Aggregation bit, when the RX vector is an HT PPDU.
    ///
    /// SOURCE: complete `libpp[hal_debug.o]::dbg_dump_rx_ppdu`,
    /// offsets `0x592..0x5e0`. The format-two branch reads the word at prefix
    /// bytes `4..8`, shifts bit 27 into the sixth argument of the HT record and
    /// names it `Aggregation`.
    pub const fn ht_aggregation(self) -> Option<bool> {
        match self.ht_signal() {
            Some(signal) => Some(signal.aggregation),
            None => None,
        }
    }

    /// A-MPDU containment established by the decoded PPDU format.
    ///
    /// HT carries a dedicated Aggregation bit and is handled separately.
    /// IEEE 802.11ax-2021 clause 26.6 defines A-MPDU operation in an HE PPDU,
    /// while the VHT/HE S-MPDU definition is explicitly the sole MPDU in an
    /// A-MPDU carried by a VHT or HE PPDU. Legacy PPDUs cannot carry an
    /// A-MPDU. Unknown future formats remain fail-closed.
    pub const fn protocol_ampdu_containment(self) -> Option<bool> {
        match self.baseband_format() {
            RxBasebandFormat::Dot11b | RxBasebandFormat::Ofdm => Some(false),
            RxBasebandFormat::Vht
            | RxBasebandFormat::HeSu
            | RxBasebandFormat::HeMu
            | RxBasebandFormat::HeExtendedRangeSu
            | RxBasebandFormat::HeTriggerBased
            | RxBasebandFormat::VhtMu => Some(true),
            RxBasebandFormat::Ht | RxBasebandFormat::Unknown(_) => None,
        }
    }
}

/// Borrowed ESP32-S31 MPDU at the normalized monitor boundary.
///
/// The vendor RX-control prefix has been decoded into portable metadata and
/// is not included in `mpdu`. The hardware-stripped FCS is likewise absent.
/// Hardware may also consume part of a verified cipher trailer; in that case
/// `mpdu.len()` is smaller than `logical_length` without making the receive
/// unit malformed.
#[derive(Clone, Copy, Debug)]
pub struct NormalizedRxFrame<'frame> {
    pub mpdu: &'frame [u8],
    /// Where `mpdu` starts in the receive buffer, after the hardware's
    /// prefix: an owner of the buffer finds the MPDU again from it.
    pub mpdu_offset: usize,
    pub metadata: MacRxMetadata<RxPhyInfo>,
    /// The frame's RSSI from the receive prefix, which every completed
    /// frame carries ([`decode_normalized_rx_metadata`] observes the same
    /// byte).
    pub rssi_dbm: i8,
    pub logical_length: usize,
    /// The receive timestamp of the prefix: the raw MAC local time of the
    /// frame's reception ([`decode_rx_local_timestamp`]).
    pub stamp: Option<u32>,
}

/// Hardware-verified CCMP data MPDU before 802.11-to-Ethernet decapsulation.
///
/// The S31 MAC decrypts the payload in place and reports MIC failure through
/// RX metadata. It leaves the eight-byte CCMP header in the DMA view. The
/// The eight-byte MIC can be wholly or partially absent from the completed
/// DMA view. The reviewed vendor path passed the logical `sig_len - FCS`
/// to `ccmp_decap`, which removed the complete MIC without reading it.
/// [`mic_bytes_in_dma`](Self::mic_bytes_in_dma) records the bounded physical
/// view while [`mic_offset`](Self::mic_offset) remains the logical payload
/// boundary.
///
/// These offsets reproduce the finite `ccmp_decap` pointer/length adjustment
/// from the pinned net80211 oracle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RxCcmpDataFrame {
    pub mpdu: RxMpduFrame,
    pub ccmp_header: CcmpHeader,
    pub ccmp_header_offset: usize,
    pub payload_offset: usize,
    pub payload_length: usize,
    pub mic_offset: usize,
    pub mic_bytes_in_dma: usize,
    pub mic_present_in_dma: bool,
}

/// Borrowed view of one contiguous hardware-verified CCMP MPDU.
///
/// The ordinary connected path first copies a completed DMA unit into an
/// independently owned staging slot. When that unit contains one complete
/// MPDU, parsing can borrow the staged bytes directly instead of copying the
/// complete MPDU into a second scratch buffer. [`frame`](Self::frame) retains
/// the same validated offsets and metadata as [`extract_ccmp_data`].
#[derive(Clone, Copy, Debug)]
pub struct RxCcmpDataView<'frame> {
    pub mpdu: &'frame [u8],
    pub frame: RxCcmpDataFrame,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RxError {
    Invalid,
    Chain,
    Bounds,
    RxFailure,
    MicFailure,
    Quarantined,
    OutputSmall,
    Metadata,
    Ignored,
    Unsupported,
}

fn copy_frame(
    segments: &[RxSegment<'_>],
    first_offset: usize,
    length: usize,
    output: &mut [u8],
) -> bool {
    let mut copied = 0;
    for (index, segment) in segments.iter().enumerate() {
        if copied == length {
            break;
        }
        let segment_length = usize::from(segment.descriptor_word0.length());
        let offset = if index == 0 { first_offset } else { 0 };
        let Some(available) = segment_length.checked_sub(offset) else {
            return false;
        };
        let take = available.min(length - copied);
        let Some(source) = segment.buffer.get(offset..offset + take) else {
            return false;
        };
        output[copied..copied + take].copy_from_slice(source);
        copied += take;
    }
    copied == length
}

oer_esp32s31_hal::place_rx_hot_path! {
#[inline(never)]
fn extract_mpdu_allowing_consumed_trailer(
    segments: &[RxSegment<'_>],
    config: RxIngressConfig,
    output: &mut [u8],
    maximum_consumable_trailer_length: usize,
) -> Result<(RxMpduFrame, usize), RxError> {
    if segments.is_empty()
        || config.ring_entry_limit == 0
        || segments.len() > config.ring_entry_limit
        || config.flags & !INGRESS_VALID_FLAGS != 0
    {
        return Err(RxError::Invalid);
    }

    for (index, segment) in segments.iter().enumerate() {
        if !segment_valid(segment)
            || segments[..index]
                .iter()
                .any(|old| old.descriptor_address == segment.descriptor_address)
        {
            return Err(RxError::Chain);
        }
        if let Some(next) = segments.get(index + 1)
            && (segment.descriptor_word0.rx_done()
                || segment.next_descriptor_address != next.descriptor_address)
        {
            return Err(RxError::Chain);
        }
    }
    if segments
        .last()
        .is_none_or(|segment| !segment.descriptor_word0.rx_done())
    {
        return Err(RxError::Chain);
    }

    let layout = first_segment_layout(&segments[0], config)?;
    let mut available = layout.available_frame_bytes;
    for segment in &segments[1..] {
        available = available
            .checked_add(usize::from(segment.descriptor_word0.length()))
            .ok_or(RxError::Bounds)?;
    }
    let expected_frame_length = layout.expected_frame_length;
    let consumed_trailer_length = expected_frame_length.saturating_sub(available);
    let frame_length = if consumed_trailer_length == 0 {
        expected_frame_length
    } else if consumed_trailer_length <= maximum_consumable_trailer_length {
        available
    } else {
        return Err(RxError::Bounds);
    };
    if frame_length > output.len() {
        return Err(RxError::OutputSmall);
    }
    if !copy_frame(segments, layout.frame_offset, frame_length, output) {
        return Err(RxError::Bounds);
    }

    Ok((
        RxMpduFrame {
            length: frame_length,
            tail_offset: layout.tail_offset,
            signal_length: layout.signal_length as u16,
            dump_length: layout.dump_length as u16,
            rxend_state: layout.rxend_state,
            rx_state: layout.rx_state,
            internal_state: layout.internal_state,
            dump_length_matches: layout.dump_length_matches,
        },
        consumed_trailer_length,
    ))
}}

fn extract_mpdu(
    segments: &[RxSegment<'_>],
    config: RxIngressConfig,
    output: &mut [u8],
) -> Result<RxMpduFrame, RxError> {
    extract_mpdu_allowing_consumed_trailer(segments, config, output, 0).map(|(frame, _)| frame)
}

/// Validate and copy one control MPDU whose four-byte FCS is stripped by the
/// S31 RX boundary.
///
/// This function owns only descriptor geometry and the IEEE frame-type
/// admission check. Subtype-specific formats, including 802.11ax Trigger
/// Common/User Info, belong to `oer-ieee80211-wire`.
pub fn extract_control(
    segments: &[RxSegment<'_>],
    config: RxIngressConfig,
    output: &mut [u8],
) -> Result<RxMpduFrame, RxError> {
    let frame = extract_mpdu(segments, config, output)?;
    if frame.length < CONTROL_HEADER_MIN_SIZE {
        return Err(RxError::Bounds);
    }
    if FrameControlFields::from_bytes([output[0], output[1]]).version() != 0
        || FrameControlFields::from_bytes([output[0], output[1]]).kind() != FrameType::Control
    {
        return Err(RxError::Ignored);
    }
    Ok(frame)
}

/// Validates one completed chain, strips the four-byte FCS and copies one
/// unfragmented, CCMP-protected management MPDU into caller-owned storage.
///
/// Hardware does not decrypt received management frames: under management
/// frame protection the station opens them in software, as the vendor's
/// `sta_recv_mgmt` does, so the copy keeps its CCMP header and MIC.
pub fn extract_protected_management(
    segments: &[RxSegment<'_>],
    config: RxIngressConfig,
    output: &mut [u8],
) -> Result<RxMpduFrame, RxError> {
    let frame = extract_mpdu(segments, config, output)?;
    if frame.length < MLME_HEADER_SIZE {
        return Err(RxError::Bounds);
    }
    if FrameControlFields::from_bytes([output[0], output[1]]).version() != 0
        || FrameControlFields::from_bytes([output[0], output[1]]).kind() != FrameType::Management
    {
        return Err(RxError::Ignored);
    }
    if !FrameControlFields::from_bytes([output[0], output[1]]).protected()
        || FrameControlFields::from_bytes([output[0], output[1]]).more_fragments()
        || SequenceControl::from_bytes([output[22], output[23]]).fragment() != 0
    {
        return Err(RxError::Unsupported);
    }
    Ok(frame)
}

oer_esp32s31_hal::place_rx_hot_path! {
/// Extracts one unfragmented CCMP data MPDU after hardware MIC verification.
///
/// This entry requires the Protected bit. A returned
/// frame therefore has both a successful RX crypto status and a valid CCMP
/// ExtIV/header shape. The payload bytes between `payload_offset` and
/// `mic_offset` are the hardware-decrypted LLC/SNAP payload.
///
/// S31 hardware runs show that successful hardware verification can publish a
/// DMA view ending anywhere inside the eight-byte CCMP MIC while `sig_len` still
/// describes the complete on-air MPDU. This reproduces the reviewed
/// `ccmp_decap` invariant: only bytes after the logical payload boundary may
/// be absent. Unprotected extraction remains strict.
#[inline(never)]
pub fn extract_ccmp_data(
    segments: &[RxSegment<'_>],
    config: RxIngressConfig,
    output: &mut [u8],
) -> Result<RxCcmpDataFrame, RxError> {
    let (frame, consumed_trailer_length) =
        extract_mpdu_allowing_consumed_trailer(segments, config, output, CCMP_MIC_SIZE)?;
    validate_ccmp_data(
        &output[..frame.length],
        frame,
        consumed_trailer_length,
        CcmpFragmentAdmission::Unfragmented,
    )
}}

oer_esp32s31_hal::place_rx_hot_path! {
/// Validate and borrow one contiguous CCMP MPDU from independently owned RX
/// storage.
///
/// This has the same admission contract as [`extract_ccmp_data`], but it is
/// intentionally restricted to one completed descriptor. Split hardware
/// units must continue through the copying extractor so no non-contiguous
/// bytes can escape as a slice.
#[inline(never)]
pub fn view_ccmp_data<'frame>(
    segment: &RxSegment<'frame>,
    config: RxIngressConfig,
) -> Result<RxCcmpDataView<'frame>, RxError> {
    view_ccmp_data_with_fragment_admission(segment, config, CcmpFragmentAdmission::Unfragmented)
}}

oer_esp32s31_hal::place_rx_hot_path! {
/// Validate and borrow one fragmented CCMP MPDU after hardware MIC success.
///
/// This is deliberately distinct from [`view_ccmp_data`]: callers cannot
/// accidentally route a fragment into ordinary LLC decapsulation, while the
/// shared validator preserves the exact S31 crypto/trailer contract.
#[inline(never)]
pub fn view_ccmp_data_fragment<'frame>(
    segment: &RxSegment<'frame>,
    config: RxIngressConfig,
) -> Result<RxCcmpDataView<'frame>, RxError> {
    view_ccmp_data_with_fragment_admission(segment, config, CcmpFragmentAdmission::Fragmented)
}}

#[derive(Clone, Copy)]
enum CcmpFragmentAdmission {
    Unfragmented,
    Fragmented,
}

oer_esp32s31_hal::place_rx_hot_path! {
#[inline(never)]
fn view_ccmp_data_with_fragment_admission<'frame>(
    segment: &RxSegment<'frame>,
    config: RxIngressConfig,
    fragment_admission: CcmpFragmentAdmission,
) -> Result<RxCcmpDataView<'frame>, RxError> {
    if config.ring_entry_limit == 0
        || config.flags & !INGRESS_VALID_FLAGS != 0
        || !segment_valid(segment)
        || !segment.descriptor_word0.rx_done()
    {
        return Err(RxError::Invalid);
    }
    let layout = first_segment_layout(segment, config)?;
    let consumed_trailer_length = layout.frame_shortfall;
    if consumed_trailer_length > CCMP_MIC_SIZE {
        return Err(RxError::Bounds);
    }
    let frame_length = if consumed_trailer_length == 0 {
        layout.expected_frame_length
    } else {
        layout.available_frame_bytes
    };
    let frame_end = layout
        .frame_offset
        .checked_add(frame_length)
        .ok_or(RxError::Bounds)?;
    let mpdu = segment
        .buffer
        .get(layout.frame_offset..frame_end)
        .ok_or(RxError::Bounds)?;
    let metadata = RxMpduFrame {
        length: frame_length,
        tail_offset: layout.tail_offset,
        signal_length: layout.signal_length as u16,
        dump_length: layout.dump_length as u16,
        rxend_state: layout.rxend_state,
        rx_state: layout.rx_state,
        internal_state: layout.internal_state,
        dump_length_matches: layout.dump_length_matches,
    };
    let frame = validate_ccmp_data(mpdu, metadata, consumed_trailer_length, fragment_admission);
    let frame = frame?;
    Ok(RxCcmpDataView { mpdu, frame })
}}

oer_esp32s31_hal::place_rx_hot_path! {
#[inline(never)]
fn validate_ccmp_data(
    mpdu: &[u8],
    frame: RxMpduFrame,
    consumed_trailer_length: usize,
    fragment_admission: CcmpFragmentAdmission,
) -> Result<RxCcmpDataFrame, RxError> {
    if frame.length < MLME_HEADER_SIZE {
        return Err(RxError::Bounds);
    }
    let frame_control = FrameControlFields::from_bytes([mpdu[0], mpdu[1]]);
    if frame_control.version() != 0 || frame_control.kind() != FrameType::Data {
        return Err(RxError::Ignored);
    }
    let fragmented = frame_control.more_fragments() || SequenceControl::from_bytes([mpdu[22], mpdu[23]]).fragment() != 0;
    let fragmentation_matches = matches!(
        (fragment_admission, fragmented),
        (CcmpFragmentAdmission::Unfragmented, false) | (CcmpFragmentAdmission::Fragmented, true)
    );
    if !frame_control.protected() || !fragmentation_matches {
        return Err(RxError::Unsupported);
    }

    let to_ds = frame_control.to_ds();
    let from_ds = frame_control.from_ds();
    let qos = frame_control.qos();
    let ordered = frame_control.order();
    let mut ccmp_header_offset = if to_ds && from_ds {
        MLME_HEADER_SIZE + 6
    } else {
        MLME_HEADER_SIZE
    };
    if qos {
        ccmp_header_offset += 2;
        if ordered {
            ccmp_header_offset += 4;
        }
    }
    let payload_offset = ccmp_header_offset
        .checked_add(CCMP_HEADER_SIZE)
        .ok_or(RxError::Bounds)?;
    let expected_frame_length = usize::from(frame.signal_length)
        .checked_sub(FCS_SIZE)
        .ok_or(RxError::Bounds)?;
    let mic_offset = expected_frame_length
        .checked_sub(CCMP_MIC_SIZE)
        .ok_or(RxError::Bounds)?;
    let mic_bytes_in_dma = CCMP_MIC_SIZE - consumed_trailer_length;
    let mic_present_in_dma = mic_bytes_in_dma == CCMP_MIC_SIZE;
    if payload_offset > mic_offset {
        return Err(RxError::Bounds);
    }
    if mic_offset
        .checked_add(mic_bytes_in_dma)
        .is_none_or(|physical_end| physical_end > frame.length)
    {
        return Err(RxError::Bounds);
    }
    let ccmp_header_end = ccmp_header_offset
        .checked_add(CCMP_HEADER_SIZE)
        .ok_or(RxError::Bounds)?;
    let ccmp_header_bytes: [u8; CCMP_HEADER_SIZE] = mpdu
        .get(ccmp_header_offset..ccmp_header_end)
        .ok_or(RxError::Bounds)?
        .try_into()
        .map_err(|_| RxError::Bounds)?;
    // Do not rely on the decrypt engine to define public-header admission.
    // ExtIV, the reserved octet and reserved Key ID bits are authenticated AAD
    // inputs but remain software protocol invariants.
    let ccmp_header = CcmpHeader::parse(ccmp_header_bytes).map_err(|_| RxError::Unsupported)?;

    Ok(RxCcmpDataFrame {
        mpdu: frame,
        ccmp_header,
        ccmp_header_offset,
        payload_offset,
        payload_length: mic_offset - payload_offset,
        mic_offset,
        mic_bytes_in_dma,
        mic_present_in_dma,
    })
}}

pub mod ampdu;
pub mod hardware;
pub mod pool;
mod prefix;
pub mod sig_b;

use prefix::segment_valid;
pub use prefix::{
    decode_normalized_rx_metadata, decode_rx_local_timestamp, decode_rx_phy_info,
    first_segment_layout, view_normalized_rx_frame,
};

impl NormalizedRxFrame<'_> {
    /// The PHY fields actually decoded from this frame's hardware prefix.
    pub fn observed_phy(&self) -> Option<RxPhyInfo> {
        match self.metadata.rate {
            oer_ieee80211_port::RxEvidence::HardwareObserved(rate) => Some(rate),
            _ => None,
        }
    }
}
