#![no_std]
#![forbid(unsafe_code)]

//! Executor- and chip-independent access-point protocol owner.
//!
//! This crate implements a bounded Open, WPA2-Personal or WPA3-Personal peer
//! table, including management protection policy, replay and response effects. Frame
//! codecs remain in `oer-ieee80211-mac`; MMIO, DMA, IRQ, hardware key
//! slots and Embassy deadlines remain in chip/runtime crates. Replay and key
//! ownership exist only in RSN epochs. Management protection decisions take
//! integrity evidence as values; lower-MAC key slots and TX remain outside.
//!
//! EAPOL publication returns [`ApEapolTransmit`], which owns its affine
//! [`ApEapolTxContext`]. The caller retains that context through TX and returns
//! it on completion, including an unsent publication. Message 2 or Message 4
//! may arrive before the previous TX terminal; that older context cannot arm
//! the newer message's retry window or refresh a reassociated peer.
//!
//! The peer table stays in caller-owned storage. Its exclusive
//! [`AccessPointPeers`] guard moves into a service and returns by value at
//! retirement. Both retirement and drop clear per-peer secrets, PMKSA and
//! management replay state while preserving the identity generation frontier.

#[cfg(test)]
extern crate std;

pub mod beacon;
pub mod limits;
pub mod pmksa;
pub mod sae;
pub mod service;

pub use service::{
    AP_MAX_CLIENTS, AP_STATUS_INVALID_RSN, AP_STATUS_SUCCESS, AP_STATUS_TOO_MANY_STATIONS,
    AP_TX_BLOCK_ACK_NEGOTIATION_TIMEOUT, AP_TX_BLOCK_ACK_TID, AP_TX_BLOCK_ACK_WINDOW,
    AP_WPA2_FIRST_RETRY_INTERVAL, AP_WPA2_RETRY_ATTEMPTS, AP_WPA2_SUBSEQUENT_RETRY_INTERVAL,
    AccessPointClientLimit, AccessPointClientLimitError, AccessPointInactiveTimeout,
    AccessPointInactiveTimeoutError, AccessPointPeerStorage, AccessPointPeers, AccessPointService,
    AccessPointServiceStatus, ApAssociationCapabilities, ApAssociationIdentity,
    ApBufferedGroupRelease, ApBufferedUnicastRelease, ApDownlinkAdmission, ApDownlinkDisposition,
    ApEapolTransmit, ApEapolTxCompletion, ApEapolTxContext, ApGroupManagementError,
    ApManagementAction, ApManagementProtection, ApManagementRejection, ApManagementRx,
    ApManagementRxProtection, ApMlmeAction, ApPeerBinding, ApPeerClose, ApPeerCloseKind,
    ApPeerPhase, ApPeerPowerState, ApPeerStatus, ApPowerSaveAction, ApServiceError, ApWpa2Error,
    ApWpa2Progress, ApWpa2RetryProgress,
};
