//! Standalone plaintext ESP-NOW role over an exclusive station-address VIF.
//!
//! This role never enters scan, authentication, association, WPA2 or a
//! network stack. It uses normal station RX filtering (including hardware
//! auto-ACK), the shared bounded ESP-NOW application mailboxes and the sole
//! pre-connected ordinary TX descriptor.

mod channel;
pub mod mailbox;
mod rx;
mod service;

pub use channel::StandaloneEspNowChannelControl;
#[cfg(target_arch = "riscv32")]
pub use channel::StandaloneEspNowPhyChannelControl;

pub use mailbox::{
    EspNowOffChannelFailureStage, EspNowOwnedRxEvent, EspNowOwnedV1Tx, EspNowRxMailboxEpochError,
    EspNowRxMailboxResources, EspNowRxMailboxShutdown, EspNowRxPublishOutcome, EspNowRxPublisher,
    EspNowRxReceiver, EspNowTxBackpressure, EspNowTxCancelReason, EspNowTxCompletion,
    EspNowTxHandle, EspNowTxMailboxEpochError, EspNowTxMailboxInvariantError, EspNowTxMailboxOwner,
    EspNowTxMailboxResources, EspNowTxMailboxShutdown, EspNowTxRuntimeFailure, EspNowTxTerminal,
    EspNowTxTicket, EspNowTxTrySendError, EspNowV2RxEvent, EspNowV2RxMailboxError,
    EspNowV2TxRequest, EspNowV2TxTrySendError,
};

pub use rx::{StandaloneEspNowReceive, StandaloneEspNowRx, StandaloneEspNowRxProgress};

pub use service::{
    StandaloneEspNowBinding, StandaloneEspNowBindingError, StandaloneEspNowOffChannelRunError,
    StandaloneEspNowOffChannelRunFailure, StandaloneEspNowRunError, StandaloneEspNowRunFailure,
    StandaloneEspNowRunReport, StandaloneEspNowService, StandaloneEspNowStopError,
    StandaloneEspNowStopped,
};
