//! ieee802154 register and hardware operations.

pub(crate) mod backend;

pub mod lifecycle;

pub mod engine;

pub mod ll;

pub mod mac;

pub(crate) mod policy;

pub(crate) mod role;

pub use mac::Ieee802154PowerSequence;

pub use lifecycle::{
    IEEE802154_MAX_CHANNEL, IEEE802154_MIN_CHANNEL, Ieee802154Channel, Ieee802154ChannelError,
    Ieee802154FoundationCheckpoint, Ieee802154ReadbackError, Ieee802154ResetCheckpoint,
    Ieee802154ResetReadback,
};

pub use policy::{
    IEEE802154_ACK_TIMEOUT_QUANTUM_MICROSECONDS, IEEE802154_MAX_ACK_TIMEOUT_MICROSECONDS,
    Ieee802154AckTimeout, Ieee802154AckTimeoutError, Ieee802154MacControl, Ieee802154MacPolicy,
    Ieee802154MacPolicyCheckpoint, Ieee802154PanIdentity,
};

pub use role::{
    Ieee802154ClockTransitionFailure, Ieee802154Clocked, Ieee802154Cold, Ieee802154Operational,
};

pub use role::{
    Ieee802154FoundationConfigured, Ieee802154FoundationTransitionFailure,
    Ieee802154MacPolicyConfigured, Ieee802154MacPolicyRecovery,
    Ieee802154MacPolicyTransitionFailure, Ieee802154PowerTransitionFailure, Ieee802154Powered,
    Ieee802154Reset, Ieee802154ResetTransitionFailure,
};
