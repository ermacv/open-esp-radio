#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]

//! Portable ESP-NOW and Espressif long-range protocol owners.
//!
//! [`extensions`] retains peer tables, security, duplicate detection and
//! bounded v2 reassembly above the wire codecs and lower-MAC port, and the
//! Espressif PHY request of an ESP-NOW peer. [`interface`] names a virtual
//! interface and binds it to a channel context; [`configuration`] checks a
//! requested owner topology against a backend's service description and
//! assigns those interfaces.

pub mod configuration;
pub mod extensions;
pub mod interface;

pub use extensions::espressif::esp_now::protocol::{
    ESP_NOW_DEFAULT_PEER_CAPACITY, ESP_NOW_RX_DUPLICATE_HISTORY_CAPACITY, EspNowConfig,
    EspNowConfigError, EspNowOwnedReceivedV1, EspNowOwnedReceivedV2, EspNowPeerCapability,
    EspNowPeerChannelPolicy, EspNowPeerConfig, EspNowPeerId, EspNowPeerSecurity, EspNowPeerTable,
    EspNowPeerTableError, EspNowPeers, EspNowPreparedV1Tx, EspNowPreparedV2Tx, EspNowProtocol,
    EspNowReceiveError, EspNowReceivedV1, EspNowReceivedV2, EspNowRxEpoch, EspNowRxOutcome,
    EspNowSendError, EspNowV2ReceiveError, EspNowV2RxOutcome, EspNowV2SendError,
};
pub use extensions::espressif::esp_now::security::{
    ESP_NOW_DEFAULT_ENCRYPTED_PEER_CAPACITY, ESP_NOW_KEY_LEN, EspNowEncryptedPeerConfig,
    EspNowEncryptedPeerDiagnostics, EspNowEncryptedPeerError, EspNowEncryptedPeerId,
    EspNowEncryptedPeerMutationFailure, EspNowEncryptedPeerReplacement,
    EspNowEncryptedPeerRestoreFailure, EspNowEncryptedPeerTable, EspNowEncryptedPeerView,
    EspNowEncryptedProtocol, EspNowEncryptedReceiveError, EspNowEncryptedRxCandidate,
    EspNowEncryptedSendError, EspNowLmk, EspNowPmk, EspNowPmkError, EspNowPmkId,
    EspNowPmkMutationFailure, EspNowPmkOwner, EspNowPreparedEncryptedV1Tx,
    EspNowRemovedEncryptedPeer, EspNowRxReplayCandidate, encrypted_peer_destination,
    esp_now_encrypted_v1_codec_status,
};
