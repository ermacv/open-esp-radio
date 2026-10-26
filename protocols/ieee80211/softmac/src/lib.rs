#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented
)]

//! Portable ESP-NOW and Espressif long-range protocol owners.
//!
//! [`extensions`] retains peer tables, security, duplicate detection and
//! bounded v2 reassembly above the wire codecs and lower-MAC port.

pub mod extensions;
mod handoff;

pub use extensions::espressif::esp_now::protocol::{
    ESP_NOW_DEFAULT_PEER_CAPACITY, ESP_NOW_RX_DUPLICATE_HISTORY_CAPACITY, EspNowConfig,
    EspNowConfigError, EspNowOwnedReceivedV1, EspNowOwnedReceivedV2, EspNowPeerCapability,
    EspNowPeerChannelPolicy, EspNowPeerConfig, EspNowPeerId, EspNowPeerSecurity, EspNowPeerTable,
    EspNowPeerTableError, EspNowPeers, EspNowPreparedV1Tx, EspNowPreparedV2Tx, EspNowProtocol,
    EspNowReceiveError, EspNowReceivedV1, EspNowReceivedV2, EspNowRxEpoch, EspNowRxOutcome,
    EspNowSendError, EspNowV2ReceiveError, EspNowV2RxOutcome, EspNowV2SendError,
};
pub use extensions::espressif::esp_now::security::{
    ESP_NOW_DEFAULT_ENCRYPTED_PEER_CAPACITY, ESP_NOW_KEY_LEN, ESP_NOW_RX_REPLAY_WINDOW_BITS,
    EspNowEncryptedPeerConfig, EspNowEncryptedPeerDiagnostics, EspNowEncryptedPeerError,
    EspNowEncryptedPeerId, EspNowEncryptedPeerMutationFailure, EspNowEncryptedPeerReplacement,
    EspNowEncryptedPeerRestoreFailure, EspNowEncryptedPeerTable, EspNowEncryptedPeerView,
    EspNowEncryptedProtocol, EspNowEncryptedReceiveError, EspNowEncryptedRxCandidate,
    EspNowEncryptedSendError, EspNowLmk, EspNowPmk, EspNowPmkError, EspNowPmkId,
    EspNowPmkMutationFailure, EspNowPmkOwner, EspNowPreparedEncryptedV1Tx,
    EspNowRemovedEncryptedPeer, EspNowRxReplayCandidate, encrypted_peer_destination,
    esp_now_encrypted_v1_codec_status,
};
