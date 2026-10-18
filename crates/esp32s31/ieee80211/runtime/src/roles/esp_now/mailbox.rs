//! Bounded application mailboxes shared by every ESP-NOW epoch.
//!
//! The standalone ESP-NOW role and the connected station attach the same
//! RX and TX handoffs. The mailboxes do not depend on either role's
//! scheduling.

pub(crate) mod rx;
pub(crate) mod tx;

pub use rx::{
    EspNowOwnedRxEvent, EspNowRxMailboxEpochError, EspNowRxMailboxResources,
    EspNowRxMailboxShutdown, EspNowRxPublishOutcome, EspNowRxPublisher, EspNowRxReceiver,
    EspNowV2RxEvent, EspNowV2RxMailboxError,
};

pub use tx::{
    EspNowOffChannelFailureStage, EspNowOwnedV1Tx, EspNowTxBackpressure, EspNowTxCancelReason,
    EspNowTxCompletion, EspNowTxHandle, EspNowTxMailboxEpochError, EspNowTxMailboxInvariantError,
    EspNowTxMailboxOwner, EspNowTxMailboxResources, EspNowTxMailboxShutdown,
    EspNowTxRuntimeFailure, EspNowTxTerminal, EspNowTxTicket, EspNowTxTrySendError,
    EspNowV2TxRequest, EspNowV2TxTrySendError,
};
