//! ESP32-S31 handoff for plaintext ESP-NOW v1 and v2.
//!
//! The portable protocol owner supplies an already validated vendor Action
//! MPDU. This module binds it to the ordinary station queue only after the
//! caller proves that the active channel context is the prepared peer channel.
//! Connected integrations still bind that context to home; a standalone owner
//! may explicitly retune for a `StandaloneFixed` peer. Plaintext is live;
//! encryption and LR are rejected before
//! descriptor publication at their first unproven contract.

use core::fmt;

use crate::{
    ordinary_tx::{
        OrdinaryTxError, OrdinaryTxInterface, OrdinaryTxOwner, OrdinaryTxPlan, TX_FCS_SIZE,
        TX_METADATA_SIZE,
    },
    tx::{WifiTxEntropy, WifiTxPowerProfile, WifiTxProgress},
};

use oer_esp32s31_ieee80211_mac::{
    rate::{
        low::{MacLowRateGateProbe, MacLowRateTransitionError},
        schedule::{RateScheduleKind, RateScheduleRef},
    },
    rx::{RxBasebandFormat, RxPhyInfo},
    tx::{
        HtChannelWidth, HtGuardInterval, HtMcs, HtRate, LegacyRate, TxHardware, TxPhyRate,
        runtime::{OrdinaryRetryRatePolicy, P2pRetryRateSchedule},
    },
};

use oer_ieee80211_mac::{channel::WifiChannel, qos::WmmAccessCategory};

use oer_ieee80211_softmac::EspressifLongRangeRate;

use oer_ieee80211_softmac::{
    EspNowEncryptedPeerId, EspNowHtGuardInterval, EspNowHtMcs, EspNowLmk, EspNowOfdmRate,
    EspNowPhyMode, EspNowPreparedEncryptedV1Tx, EspNowPreparedV1Tx, EspNowPreparedV2Tx,
    MacRxEvidence, MacRxMetadata, MacTxPlan, interface::BoundVirtualInterface,
};

/// Deepest completed part of one ESP-NOW Long Range TX attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowLongRangeReached {
    RateSelected,
    /// The complete three-edge PHY gate and matching restore leaf ran, with
    /// the ROM status observation returned to its entry value before control
    /// resumed.
    PhyLowRateGateRestored,
}

/// First missing ownership boundary in one Long Range attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowLongRangeMissing {
    /// The supplied queue backend does not own the runtime PHY-low-rate gate.
    RuntimeLowRateOwner,
    /// No reviewed LR-specific PLCP and queue-vector formatter exists.
    TxPlcpQueueVector,
    /// The five-bit public RX rate field has no reviewed mapping back to the
    /// two LR descriptor codes.
    RxRateNormalization,
}

/// Honest static PHY surface of the ESP32-S31 ESP-NOW backend.
///
/// Standard modes are live through both connected and standalone ordinary-TX
/// owners. LR identities and retry records are source-owned, but the physical
/// contract remains unavailable until both missing boundaries below have
/// reviewed evidence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowPhySupport {
    Live,
    LongRangeFailClosed {
        tx_missing: EspNowLongRangeMissing,
        rx_missing: EspNowLongRangeMissing,
    },
}

pub const fn esp32s31_esp_now_phy_support(mode: EspNowPhyMode) -> EspNowPhySupport {
    match mode {
        EspNowPhyMode::LegacyDsss1M
        | EspNowPhyMode::StandardP2pOfdm(_)
        | EspNowPhyMode::StandardP2pHt20(_) => EspNowPhySupport::Live,
        EspNowPhyMode::LongRange(_) => EspNowPhySupport::LongRangeFailClosed {
            tx_missing: EspNowLongRangeMissing::TxPlcpQueueVector,
            rx_missing: EspNowLongRangeMissing::RxRateNormalization,
        },
    }
}

/// Status of the chip RX-rate field after applying the configured ESP-NOW PHY
/// context.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowRxRateNormalization {
    /// The public RX prefix itself identifies one standard ESP-NOW-capable
    /// baseband format. This does not claim the configured preferred rate was
    /// used: a retry may have arrived in another standard format.
    DecodedStandard { format: RxBasebandFormat },
    /// No decoded standard baseband-format evidence accompanies this sample.
    /// Preserve the observation in the diagnostic, but do not publish it as
    /// a normalized standard rate.
    StandardUnavailable { observed: MacRxEvidence<RxPhyInfo> },
    /// LR descriptor codes are wider than the public five-bit RX `rate`
    /// field. Preserve the raw observation here, but do not expose it as a
    /// normalized rate where it could alias an ordinary standard code.
    LongRangeUnavailable {
        observed: MacRxEvidence<RxPhyInfo>,
        missing: EspNowLongRangeMissing,
    },
}

/// ESP-NOW-specific RX metadata with an explicit LR normalization frontier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowRxMetadata {
    pub normalized: MacRxMetadata<RxPhyInfo>,
    pub rate_normalization: EspNowRxRateNormalization,
}

/// Normalize one hardware prefix in the configured peer-PHY context.
///
/// A preferred peer PHY is not proof of the exact received retry rate. The
/// configured mode is used only to quarantine LR's truncated five-bit field;
/// a standard result additionally requires an observed decoded Dot11b, OFDM
/// or HT baseband format. All non-rate evidence is retained unchanged.
pub fn normalize_esp_now_rx_metadata(
    phy_mode: EspNowPhyMode,
    mut metadata: MacRxMetadata<RxPhyInfo>,
) -> EspNowRxMetadata {
    let observed = metadata.rate;
    let rate_normalization = if matches!(phy_mode, EspNowPhyMode::LongRange(_)) {
        metadata.rate = MacRxEvidence::Unavailable;
        EspNowRxRateNormalization::LongRangeUnavailable {
            observed,
            missing: EspNowLongRangeMissing::RxRateNormalization,
        }
    } else if let Some(format) = decoded_standard_format(observed) {
        EspNowRxRateNormalization::DecodedStandard { format }
    } else {
        metadata.rate = MacRxEvidence::Unavailable;
        EspNowRxRateNormalization::StandardUnavailable { observed }
    };
    EspNowRxMetadata {
        normalized: metadata,
        rate_normalization,
    }
}

const fn decoded_standard_format(evidence: MacRxEvidence<RxPhyInfo>) -> Option<RxBasebandFormat> {
    let format = match evidence {
        MacRxEvidence::HardwareObserved(info) | MacRxEvidence::ProtocolValidated(info) => {
            info.baseband_format()
        }
        MacRxEvidence::Unavailable => return None,
    };
    match format {
        RxBasebandFormat::Dot11b | RxBasebandFormat::Ofdm | RxBasebandFormat::Ht => Some(format),
        _ => None,
    }
}

/// Precise fail-closed Long Range frontier returned before DMA publication.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowLongRangeUnsupported {
    pub selection: EspressifLongRangeRate,
    pub reached: EspNowLongRangeReached,
    pub missing: EspNowLongRangeMissing,
}

/// Bounded publication policy for one ESP-NOW Action MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EspNowTxConfig {
    unicast_publication_limit: u8,
    publication_timeout: u64,
}

impl EspNowTxConfig {
    pub const fn new(
        unicast_publication_limit: u8,
        publication_timeout: u64,
    ) -> Result<Self, EspNowTxConfigError> {
        if unicast_publication_limit == 0 {
            return Err(EspNowTxConfigError::ZeroPublicationLimit);
        }
        if publication_timeout == 0 {
            return Err(EspNowTxConfigError::ZeroPublicationTimeout);
        }
        Ok(Self {
            unicast_publication_limit,
            publication_timeout,
        })
    }

    pub const fn unicast_publication_limit(self) -> u8 {
        self.unicast_publication_limit
    }

    pub const fn publication_timeout(self) -> u64 {
        self.publication_timeout
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowTxConfigError {
    ZeroPublicationLimit,
    ZeroPublicationTimeout,
}

impl fmt::Display for EspNowTxConfigError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::ZeroPublicationLimit => "ESP-NOW unicast publication limit must be nonzero",
            Self::ZeroPublicationTimeout => "ESP-NOW publication timeout must be nonzero",
        })
    }
}

impl core::error::Error for EspNowTxConfigError {}

/// S31-specific key namespace for ESP-NOW.
///
/// The tracked evidence assigns WPA2 STA pairwise/group slots 4/1 and AP
/// pairwise slots 8..=22 (plus AP group-key slots). It does not establish an
/// ESP-NOW key-selector mapping for any remaining entry. Consequently this
/// owner has zero installable hardware slots and cannot alias a WPA2 token.
pub struct EspNowKeyOwner {
    diagnostics: EspNowCryptoDiagnostics,
}

impl EspNowKeyOwner {
    pub const fn new() -> Self {
        Self {
            diagnostics: EspNowCryptoDiagnostics {
                key_install_rejections: 0,
                encrypted_tx_rejections: 0,
            },
        }
    }

    pub const fn hardware_slot_capacity(&self) -> usize {
        0
    }

    pub const fn diagnostics(&self) -> EspNowCryptoDiagnostics {
        self.diagnostics
    }

    /// Fail before copying an LMK or touching key-table MMIO. The uninhabited
    /// success token makes it impossible for downstream TX to guess a WPA2 or
    /// AP hardware selector.
    pub fn install(
        &mut self,
        _peer: EspNowEncryptedPeerId,
        _lmk: &EspNowLmk,
    ) -> Result<EspNowKeySlot, EspNowCryptoError> {
        self.diagnostics.key_install_rejections =
            self.diagnostics.key_install_rejections.saturating_add(1);
        Err(EspNowCryptoError::KeySelectorOwnershipUnproven)
    }

    fn reject_encrypted_tx(&mut self) {
        self.diagnostics.encrypted_tx_rejections =
            self.diagnostics.encrypted_tx_rejections.saturating_add(1);
    }
}

impl Default for EspNowKeyOwner {
    fn default() -> Self {
        Self::new()
    }
}

/// Uninhabited ESP-NOW-only hardware key authority.
///
/// This becomes inhabited only after reviewed evidence assigns a disjoint S31
/// selector and exact key image. No conversion exists from STA/AP CCMP slots.
pub enum EspNowKeySlot {}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EspNowCryptoDiagnostics {
    pub key_install_rejections: u32,
    pub encrypted_tx_rejections: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowCryptoError {
    ChannelMismatch {
        prepared: WifiChannel,
        active: WifiChannel,
    },
    StationBindingMismatch {
        prepared: BoundVirtualInterface,
        active: BoundVirtualInterface,
    },
    KeySelectorOwnershipUnproven,
    ActionAadContractUnproven,
}

impl fmt::Display for EspNowCryptoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ChannelMismatch { prepared, active } => write!(
                formatter,
                "encrypted ESP-NOW prepared channel {prepared:?} differs from active channel {active:?}"
            ),
            Self::StationBindingMismatch { prepared, active } => write!(
                formatter,
                "encrypted ESP-NOW station binding {prepared:?} differs from active binding {active:?}"
            ),
            Self::KeySelectorOwnershipUnproven => formatter.write_str(
                "ESP32-S31 has no reviewed ESP-NOW hardware key-selector namespace",
            ),
            Self::ActionAadContractUnproven => formatter.write_str(
                "ESP32-S31 encrypted ESP-NOW Action AAD and construct/decrypt contract are unproven",
            ),
        }
    }
}

impl core::error::Error for EspNowCryptoError {}

/// Validate the live station/channel binding, then fail before touching the
/// ordinary TX buffer. Both exact Action AAD and a disjoint S31 key selector
/// are required before this handoff may become live.
pub fn start_esp_now_v1_encrypted<P, E, T, const BUFFER_SIZE: usize>(
    _ordinary: &mut OrdinaryTxOwner<'_, P, E, T, BUFFER_SIZE>,
    keys: &mut EspNowKeyOwner,
    prepared: EspNowPreparedEncryptedV1Tx<'_>,
    active_channel: WifiChannel,
    active_station: BoundVirtualInterface,
) -> Result<WifiTxProgress, EspNowCryptoError>
where
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    if prepared.home_channel() != active_channel {
        return Err(EspNowCryptoError::ChannelMismatch {
            prepared: prepared.home_channel(),
            active: active_channel,
        });
    }
    if prepared.station() != active_station {
        return Err(EspNowCryptoError::StationBindingMismatch {
            prepared: prepared.station(),
            active: active_station,
        });
    }
    keys.reject_encrypted_tx();
    Err(EspNowCryptoError::ActionAadContractUnproven)
}

/// Encode and publish one validated plaintext ESP-NOW v1 frame.
///
/// `active_channel` and `active_station` must be read from the role/channel
/// owner while it excludes a concurrent retune. A mismatch is reported before
/// the ordinary TX buffer or descriptor is touched.
pub fn start_esp_now_v1_plaintext<H, P, E, T, const BUFFER_SIZE: usize>(
    ordinary: &mut OrdinaryTxOwner<'_, P, E, T, BUFFER_SIZE>,
    hardware: &mut H,
    prepared: EspNowPreparedV1Tx<'_>,
    active_channel: WifiChannel,
    active_station: BoundVirtualInterface,
    config: EspNowTxConfig,
) -> Result<WifiTxProgress, EspNowTxError>
where
    H: TxHardware,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    if prepared.transmit_channel() != active_channel {
        return Err(EspNowTxError::ChannelMismatch {
            prepared: prepared.transmit_channel(),
            active: active_channel,
        });
    }
    if prepared.station() != active_station {
        return Err(EspNowTxError::StationBindingMismatch {
            prepared: prepared.station(),
            active: active_station,
        });
    }
    let (initial_rate, retry_rate_policy) = plaintext_tx_policy(hardware, prepared.phy_mode())?;

    let frame_length = {
        let buffer = ordinary.buffer_mut().map_err(EspNowTxError::Tx)?;
        let Some(frame_buffer) = buffer.get_mut(TX_METADATA_SIZE..) else {
            return Err(EspNowTxError::Tx(OrdinaryTxError::BufferSizeOverflow));
        };
        prepared.encode(frame_buffer).map_err(EspNowTxError::Wire)?
    };
    publish_plaintext(
        ordinary,
        hardware,
        frame_length,
        prepared.destination().is_broadcast(),
        initial_rate,
        retry_rate_policy,
        config,
    )
}

/// Encode and publish one validated plaintext ESP-NOW v2 frame through the
/// same ordinary station owner, retry policy and IRQ lifecycle as v1.
///
/// A generic buffer smaller than the complete metadata plus MPDU requirement
/// is rejected before `buffer_mut` can expose or mutate DMA storage.
pub fn start_esp_now_v2_plaintext<H, P, E, T, const BUFFER_SIZE: usize>(
    ordinary: &mut OrdinaryTxOwner<'_, P, E, T, BUFFER_SIZE>,
    hardware: &mut H,
    prepared: EspNowPreparedV2Tx<'_>,
    active_channel: WifiChannel,
    active_station: BoundVirtualInterface,
    config: EspNowTxConfig,
) -> Result<WifiTxProgress, EspNowTxError>
where
    H: TxHardware,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    if prepared.transmit_channel() != active_channel {
        return Err(EspNowTxError::ChannelMismatch {
            prepared: prepared.transmit_channel(),
            active: active_channel,
        });
    }
    if prepared.station() != active_station {
        return Err(EspNowTxError::StationBindingMismatch {
            prepared: prepared.station(),
            active: active_station,
        });
    }
    let required = TX_METADATA_SIZE
        .checked_add(prepared.encoded_len())
        .and_then(|length| length.checked_add(TX_FCS_SIZE))
        .and_then(|length| length.checked_add(3))
        .map(|length| length & !3)
        .ok_or(EspNowTxError::Tx(OrdinaryTxError::BufferSizeOverflow))?;
    if BUFFER_SIZE < required {
        return Err(EspNowTxError::BufferTooSmall {
            required,
            available: BUFFER_SIZE,
        });
    }
    let (initial_rate, retry_rate_policy) = plaintext_tx_policy(hardware, prepared.phy_mode())?;
    let frame_length = {
        let buffer = ordinary.buffer_mut().map_err(EspNowTxError::Tx)?;
        let frame_buffer = &mut buffer[TX_METADATA_SIZE..];
        prepared
            .encode(frame_buffer)
            .map_err(EspNowTxError::V2Wire)?
    };
    publish_plaintext(
        ordinary,
        hardware,
        frame_length,
        prepared.destination().is_broadcast(),
        initial_rate,
        retry_rate_policy,
        config,
    )
}

fn plaintext_tx_policy<H: TxHardware>(
    hardware: &mut H,
    phy_mode: EspNowPhyMode,
) -> Result<(TxPhyRate, OrdinaryRetryRatePolicy), EspNowTxError> {
    Ok(match phy_mode {
        EspNowPhyMode::LegacyDsss1M => (
            TxPhyRate::Legacy(LegacyRate::Dsss1MLong),
            OrdinaryRetryRatePolicy::Normal,
        ),
        EspNowPhyMode::StandardP2pOfdm(rate) => {
            let (rate, schedule_index) = p2p_ofdm_rate(rate);
            (
                TxPhyRate::Legacy(rate),
                p2p_retry_policy(RateScheduleKind::P2pDot11G, schedule_index),
            )
        }
        EspNowPhyMode::StandardP2pHt20(rate) => {
            let mcs = esp_now_ht_mcs(rate.mcs());
            let (guard_interval, retry_rate_policy) = match rate.guard_interval() {
                EspNowHtGuardInterval::Long800Ns => (
                    HtGuardInterval::Long800Ns,
                    p2p_retry_policy(RateScheduleKind::P2pDot11N, 8 - rate.mcs().index()),
                ),
                EspNowHtGuardInterval::Short400Ns if rate.mcs() == EspNowHtMcs::Mcs7 => (
                    HtGuardInterval::Short400Ns,
                    p2p_retry_policy(RateScheduleKind::P2pDot11N, 0),
                ),
                EspNowHtGuardInterval::Short400Ns => (
                    HtGuardInterval::Short400Ns,
                    p2p_ht_sgi_retry_policy(8 - rate.mcs().index()),
                ),
            };
            (
                TxPhyRate::Ht(HtRate::new(mcs, guard_interval, HtChannelWidth::Mhz20)),
                retry_rate_policy,
            )
        }
        EspNowPhyMode::LongRange(selection) => {
            let probe = hardware
                .probe_phy_low_rate_gate()
                .map_err(|error| EspNowTxError::LongRangeLowRateTransition { selection, error })?;
            let (reached, missing) = match probe {
                MacLowRateGateProbe::OwnerUnavailable => (
                    EspNowLongRangeReached::RateSelected,
                    EspNowLongRangeMissing::RuntimeLowRateOwner,
                ),
                MacLowRateGateProbe::Restored { .. } => (
                    EspNowLongRangeReached::PhyLowRateGateRestored,
                    EspNowLongRangeMissing::TxPlcpQueueVector,
                ),
            };
            return Err(EspNowTxError::LongRangeUnsupported(
                EspNowLongRangeUnsupported {
                    selection,
                    reached,
                    missing,
                },
            ));
        }
    })
}

fn publish_plaintext<H, P, E, T, const BUFFER_SIZE: usize>(
    ordinary: &mut OrdinaryTxOwner<'_, P, E, T, BUFFER_SIZE>,
    hardware: &mut H,
    frame_length: usize,
    broadcast: bool,
    initial_rate: TxPhyRate,
    retry_rate_policy: OrdinaryRetryRatePolicy,
    config: EspNowTxConfig,
) -> Result<WifiTxProgress, EspNowTxError>
where
    H: TxHardware,
    P: WifiTxPowerProfile,
    E: WifiTxEntropy,
    T: oer_time::Timer,
{
    let publication_limit = if broadcast {
        1
    } else {
        config.unicast_publication_limit
    };

    ordinary
        .start_with_retry_rate_policy(
            hardware,
            OrdinaryTxPlan {
                frame_length,
                descriptor_capacity: None,
                exchange: MacTxPlan {
                    access_category: WmmAccessCategory::Voice,
                    initial_rate,
                    publication_limit,
                    publication_timeout: oer_time::Duration::from_micros(
                        config.publication_timeout,
                    ),
                },
                hardware_mic_length: 0,
                // The portable handoff has no encrypted variant and therefore
                // cannot consume any WPA2-owned hardware key slot.
                hardware_key_selector: 0,
                interface: OrdinaryTxInterface::Station,
                scheduler_priority: 1,
                packet_priority: 1,
                priority_count: 1,
            },
            retry_rate_policy,
        )
        .map_err(EspNowTxError::Tx)
}

fn p2p_retry_policy(kind: RateScheduleKind, index: u8) -> OrdinaryRetryRatePolicy {
    let schedule = RateScheduleRef::new(kind, index)
        .expect("ESP-NOW uses an in-range finite P2P schedule index");
    let schedule = P2pRetryRateSchedule::new(schedule)
        .expect("ESP-NOW selects only a standard P2P retry arena");
    OrdinaryRetryRatePolicy::P2p(schedule)
}

fn p2p_ht_sgi_retry_policy(index: u8) -> OrdinaryRetryRatePolicy {
    let schedule = RateScheduleRef::new(RateScheduleKind::P2pDot11N, index)
        .expect("ESP-NOW HT20 SGI selects an in-range same-MCS LGI record");
    let schedule = P2pRetryRateSchedule::new(schedule)
        .expect("ESP-NOW HT20 SGI fallback uses the standard P2P HT arena");
    OrdinaryRetryRatePolicy::P2pHtSgiFallback(schedule)
}

const fn p2p_ofdm_rate(rate: EspNowOfdmRate) -> (LegacyRate, u8) {
    match rate {
        EspNowOfdmRate::Mbps54 => (LegacyRate::Ofdm54M, 0),
        EspNowOfdmRate::Mbps48 => (LegacyRate::Ofdm48M, 1),
        EspNowOfdmRate::Mbps36 => (LegacyRate::Ofdm36M, 2),
        EspNowOfdmRate::Mbps24 => (LegacyRate::Ofdm24M, 3),
        EspNowOfdmRate::Mbps18 => (LegacyRate::Ofdm18M, 4),
        EspNowOfdmRate::Mbps12 => (LegacyRate::Ofdm12M, 5),
        EspNowOfdmRate::Mbps9 => (LegacyRate::Ofdm9M, 6),
        EspNowOfdmRate::Mbps6 => (LegacyRate::Ofdm6M, 7),
    }
}

const fn esp_now_ht_mcs(mcs: EspNowHtMcs) -> HtMcs {
    match mcs {
        EspNowHtMcs::Mcs0 => HtMcs::Mcs0,
        EspNowHtMcs::Mcs1 => HtMcs::Mcs1,
        EspNowHtMcs::Mcs2 => HtMcs::Mcs2,
        EspNowHtMcs::Mcs3 => HtMcs::Mcs3,
        EspNowHtMcs::Mcs4 => HtMcs::Mcs4,
        EspNowHtMcs::Mcs5 => HtMcs::Mcs5,
        EspNowHtMcs::Mcs6 => HtMcs::Mcs6,
        EspNowHtMcs::Mcs7 => HtMcs::Mcs7,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspNowTxError {
    ChannelMismatch {
        prepared: WifiChannel,
        active: WifiChannel,
    },
    StationBindingMismatch {
        prepared: BoundVirtualInterface,
        active: BoundVirtualInterface,
    },
    P2pHtRetryScheduleUnavailable {
        mcs: EspNowHtMcs,
        guard_interval: EspNowHtGuardInterval,
    },
    LongRangeLowRateTransition {
        selection: EspressifLongRangeRate,
        error: MacLowRateTransitionError,
    },
    LongRangeUnsupported(EspNowLongRangeUnsupported),
    OffChannelLongRangeUnsupported {
        channel: WifiChannel,
    },
    BufferTooSmall {
        required: usize,
        available: usize,
    },
    Wire(oer_ieee80211_mac::extensions::espressif::esp_now::EspNowV1WireError),
    V2Wire(oer_ieee80211_mac::extensions::espressif::esp_now::EspNowV2WireError),
    Tx(OrdinaryTxError),
}

impl fmt::Display for EspNowTxError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ChannelMismatch { prepared, active } => write!(
                formatter,
                "ESP-NOW prepared channel {prepared:?} differs from active channel {active:?}"
            ),
            Self::StationBindingMismatch { prepared, active } => write!(
                formatter,
                "ESP-NOW prepared station binding {prepared:?} differs from active binding {active:?}"
            ),
            Self::P2pHtRetryScheduleUnavailable {
                mcs,
                guard_interval,
            } => write!(
                formatter,
                "ESP32-S31 has no recovered ESP-NOW P2P retry record for {mcs:?} {guard_interval:?}"
            ),
            Self::LongRangeLowRateTransition { selection, error } => write!(
                formatter,
                "ESP32-S31 ESP-NOW LR {selection:?} low-rate gate transition failed: {error:?}"
            ),
            Self::LongRangeUnsupported(frontier) => write!(
                formatter,
                "ESP32-S31 ESP-NOW LR {:?} reached {:?}; missing {:?}",
                frontier.selection, frontier.reached, frontier.missing
            ),
            Self::OffChannelLongRangeUnsupported { channel } => write!(
                formatter,
                "ESP32-S31 ESP-NOW LR is unavailable for standalone off-channel {channel:?}"
            ),
            Self::BufferTooSmall {
                required,
                available,
            } => write!(
                formatter,
                "ESP-NOW v2 TX needs {required} buffer bytes, only {available} are available"
            ),
            Self::Wire(error) => write!(formatter, "ESP-NOW frame error: {error}"),
            Self::V2Wire(error) => write!(formatter, "ESP-NOW v2 frame error: {error}"),
            Self::Tx(error) => write!(formatter, "ESP-NOW ordinary TX error: {error:?}"),
        }
    }
}

impl core::error::Error for EspNowTxError {}

#[cfg(test)]
mod tests;
