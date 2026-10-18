//! Application-facing ESP-NOW resources for explicit connected composition.
//!
//! This module does not implicitly enable ESP-NOW in the stock supervisor.
//! An application starts one bounded mailbox epoch, retains the returned
//! handle, and moves the scheduler owner into `attach_esp_now_tx` from the
//! connected driver's `map_services` composition edge.

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;

#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_ieee80211_runtime::roles::esp_now::StandaloneEspNowPhyChannelControl;

pub use oer_radio::wifi::{
    StandaloneEspNowPeerError, StandaloneEspNowRequest, WifiStandaloneEspNowPlan,
};

pub use oer_esp32s31_ieee80211::esp_now::{
    EspNowCryptoDiagnostics, EspNowCryptoError, EspNowKeyOwner, EspNowKeySlot,
    EspNowLongRangeMissing, EspNowLongRangeReached, EspNowLongRangeUnsupported, EspNowPhySupport,
    EspNowRxMetadata, EspNowRxRateNormalization, EspNowTxConfig, EspNowTxConfigError,
    EspNowTxError, esp32s31_esp_now_phy_support, normalize_esp_now_rx_metadata,
};

pub use oer_esp32s31_ieee80211_runtime::roles::{
    esp_now::mailbox::{
        EspNowOffChannelFailureStage, EspNowOwnedV1Tx, EspNowTxBackpressure, EspNowTxCancelReason,
        EspNowTxCompletion, EspNowTxMailboxEpochError, EspNowTxMailboxInvariantError,
        EspNowTxMailboxShutdown, EspNowTxRuntimeFailure, EspNowTxTerminal, EspNowTxTicket,
        EspNowTxTrySendError, EspNowV2TxTrySendError,
    },
    esp_now::{
        EspNowRxMailboxEpochError, EspNowRxMailboxResources, EspNowRxMailboxShutdown,
        EspNowRxPublishOutcome, EspNowRxPublisher, EspNowRxReceiver, EspNowV2RxEvent,
        EspNowV2RxMailboxError, StandaloneEspNowBinding, StandaloneEspNowBindingError,
        StandaloneEspNowChannelControl, StandaloneEspNowOffChannelRunError,
        StandaloneEspNowOffChannelRunFailure, StandaloneEspNowReceive, StandaloneEspNowRunError,
        StandaloneEspNowRunFailure, StandaloneEspNowRunReport, StandaloneEspNowRx,
        StandaloneEspNowRxProgress, StandaloneEspNowService, StandaloneEspNowStopError,
        StandaloneEspNowStopped,
    },
    station::connected::{
        EspNowConnectedControl, EspNowConnectedControlConfigError, EspNowConnectedControlError,
        EspNowConnectedControlShutdown, EspNowTxBinding, attach_esp_now_tx,
    },
};

pub use oer_ieee80211_mac::extensions::espressif::esp_now::{
    ESP_NOW_CCMP_HEADER_LEN, ESP_NOW_CCMP_MIC_LEN, ESP_NOW_V1_MAX_PAYLOAD_LEN,
    ESP_NOW_V1_MAX_PROTECTED_MPDU_LEN, ESP_NOW_V1_MIN_PROTECTED_MPDU_LEN,
    ESP_NOW_V2_ACTION_PREFIX_LEN, ESP_NOW_V2_MAX_ACTION_LEN, ESP_NOW_V2_MAX_ELEMENT_COUNT,
    ESP_NOW_V2_MAX_ELEMENT_PAYLOAD_LEN, ESP_NOW_V2_MAX_MPDU_LEN, ESP_NOW_V2_MAX_PAYLOAD_LEN,
    ESP_NOW_V2_MAX_VENDOR_CONTENT_LEN, ESP_NOW_V2_VERSION, EspNowCcmpPacketNumber,
    EspNowCcmpPacketNumberError, EspNowDestination, EspNowEncryptedV1Unavailable,
    EspNowProtectedV1Envelope, EspNowProtectedV1WireError, EspNowRandomValue, EspNowUnicastAddress,
    EspNowV1WireError, EspNowV2Action, EspNowV2Element, EspNowV2Elements, EspNowV2Frame,
    EspNowV2Payload, EspNowV2Reassembly, EspNowV2WireError, EspNowVersionError, EspNowWireVersion,
    esp_now_wire_version,
};

pub use oer_ieee80211_softmac::{
    ESP_NOW_DEFAULT_ENCRYPTED_PEER_CAPACITY, ESP_NOW_DEFAULT_PEER_CAPACITY, ESP_NOW_KEY_LEN,
    ESP_NOW_RX_REPLAY_WINDOW_BITS, EspNowConfig, EspNowConfigError, EspNowEncryptedPeerConfig,
    EspNowEncryptedPeerDiagnostics, EspNowEncryptedPeerError, EspNowEncryptedPeerId,
    EspNowEncryptedPeerMutationFailure, EspNowEncryptedPeerReplacement,
    EspNowEncryptedPeerRestoreFailure, EspNowEncryptedPeerTable, EspNowEncryptedPeerView,
    EspNowEncryptedProtocol, EspNowEncryptedReceiveError, EspNowEncryptedRxCandidate,
    EspNowEncryptedSendError, EspNowLmk, EspNowPeerCapability, EspNowPeerChannelPolicy,
    EspNowPeerConfig, EspNowPeerId, EspNowPeerSecurity, EspNowPeerTableError, EspNowPhyMode,
    EspNowPmk, EspNowPmkError, EspNowPmkId, EspNowPmkMutationFailure, EspNowPmkOwner,
    EspNowPreparedEncryptedV1Tx, EspNowPreparedV2Tx, EspNowProtocol, EspNowReceivedV2,
    EspNowRemovedEncryptedPeer, EspNowRxReplayCandidate, EspNowV2ReceiveError, EspNowV2RxOutcome,
    EspNowV2SendError, EspressifLongRangeRate, encrypted_peer_destination,
    esp_now_encrypted_v1_codec_status,
};

/// Failed standalone materialization retains the portable owners unchanged.
pub struct StandaloneEspNowPrepareFailure<const PEERS: usize> {
    pub error: StandaloneEspNowBindingError,
    pub plan: WifiStandaloneEspNowPlan,
    pub protocol: EspNowProtocol<PEERS>,
}

/// Join a portable standalone request to the channel currently held by the
/// exclusive radio owner.
///
/// Integration must tune and verify `active_channel` before calling this
/// function. The returned binding is the only chip-runtime input; this hook
/// intentionally performs no scan, association, off-channel fallback or
/// guessed channel programming.
pub fn prepare_esp32s31_standalone_esp_now<const PEERS: usize>(
    request: StandaloneEspNowRequest<PEERS>,
    active_channel: oer_ieee80211_mac::channel::WifiChannel,
    tx: EspNowTxConfig,
) -> Result<(EspNowProtocol<PEERS>, StandaloneEspNowBinding), StandaloneEspNowPrepareFailure<PEERS>>
{
    let (plan, protocol) = request.into_parts();
    match StandaloneEspNowBinding::new(plan, &protocol, active_channel, tx) {
        Ok(binding) => Ok((protocol, binding)),
        Err(error) => Err(StandaloneEspNowPrepareFailure {
            error,
            plan,
            protocol,
        }),
    }
}

/// Small product default; applications may select another fixed capacity.
pub const ESP32S31_DEFAULT_ESP_NOW_TX_QUEUE_DEPTH: usize = 4;
pub const ESP32S31_DEFAULT_ESP_NOW_RX_QUEUE_DEPTH: usize = 4;

pub type EspNowTransmitHandle<
    'resources,
    const CAPACITY: usize = ESP32S31_DEFAULT_ESP_NOW_TX_QUEUE_DEPTH,
> = oer_esp32s31_ieee80211_runtime::roles::esp_now::EspNowTxHandle<
    'resources,
    CriticalSectionRawMutex,
    CAPACITY,
>;

pub type EspNowTransmitMailboxOwner<
    'resources,
    const CAPACITY: usize = ESP32S31_DEFAULT_ESP_NOW_TX_QUEUE_DEPTH,
> = oer_esp32s31_ieee80211_runtime::roles::esp_now::EspNowTxMailboxOwner<
    'resources,
    CriticalSectionRawMutex,
    CAPACITY,
>;

/// Statically locatable, allocation-free TX request/completion storage.
pub struct EspNowTxResources<const CAPACITY: usize = ESP32S31_DEFAULT_ESP_NOW_TX_QUEUE_DEPTH> {
    inner: oer_esp32s31_ieee80211_runtime::roles::esp_now::EspNowTxMailboxResources<
        CriticalSectionRawMutex,
        CAPACITY,
    >,
}

impl<const CAPACITY: usize> EspNowTxResources<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            inner: oer_esp32s31_ieee80211_runtime::roles::esp_now::EspNowTxMailboxResources::new(),
        }
    }

    /// Create the next reconnect generation. Retain the handle in the
    /// application and move the owner into [`attach_esp_now_tx`].
    pub fn begin_epoch(
        &mut self,
    ) -> Result<
        (
            EspNowTransmitHandle<'_, CAPACITY>,
            EspNowTransmitMailboxOwner<'_, CAPACITY>,
        ),
        EspNowTxMailboxEpochError,
    > {
        self.inner.begin_epoch()
    }
}

impl<const CAPACITY: usize> Default for EspNowTxResources<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

pub type EspNowReceivePublisher<
    'resources,
    const CAPACITY: usize = ESP32S31_DEFAULT_ESP_NOW_RX_QUEUE_DEPTH,
> = oer_esp32s31_ieee80211_runtime::roles::esp_now::EspNowRxPublisher<
    'resources,
    CriticalSectionRawMutex,
    CAPACITY,
>;

pub type EspNowReceiveReceiver<
    'resources,
    const CAPACITY: usize = ESP32S31_DEFAULT_ESP_NOW_RX_QUEUE_DEPTH,
> = oer_esp32s31_ieee80211_runtime::roles::esp_now::EspNowRxReceiver<
    'resources,
    CriticalSectionRawMutex,
    CAPACITY,
>;

/// Explicit, allocation-free RX mailbox storage for v1 events and v2 payload
/// slots. Applications opt in by placing this owner and attaching its
/// publisher to the connected or standalone normal-RX sink.
pub struct EspNowRxResources<const CAPACITY: usize = ESP32S31_DEFAULT_ESP_NOW_RX_QUEUE_DEPTH> {
    inner: EspNowRxMailboxResources<CriticalSectionRawMutex, CAPACITY>,
}

impl<const CAPACITY: usize> EspNowRxResources<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            inner: EspNowRxMailboxResources::new(),
        }
    }

    pub fn begin_epoch(
        &mut self,
    ) -> Result<
        (
            EspNowReceivePublisher<'_, CAPACITY>,
            EspNowReceiveReceiver<'_, CAPACITY>,
        ),
        EspNowRxMailboxEpochError,
    > {
        self.inner.begin_epoch()
    }
}

impl<const CAPACITY: usize> Default for EspNowRxResources<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}
