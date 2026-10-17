//! ieee802154 register and hardware operations.

pub(crate) mod backend;

pub mod coex;

pub mod lifecycle;

pub mod ll;

pub mod mac;

pub(crate) mod policy;

pub(crate) mod role;

pub mod tx_power;

#[cfg(feature = "validation-probes")]
pub(crate) mod validation;

pub use oer_esp32s31_pac::{Ieee802154MultipanIndex, Ieee802154PowerSequence};

/// The memory the ESP32-S31 IEEE 802.15.4 MAC DMA reaches: the internal SRAM
/// of `SOC_DMA_LOW`..`SOC_DMA_HIGH` in ESP-IDF's
/// `soc/esp32s31/include/soc/soc.h` at
/// `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`. PSRAM is outside it: a frame
/// there makes the MAC report a DMA error, and nothing reaches the air or
/// memory. The engine checks its frame buffers against it
/// ([`oer_espressif_ieee802154_engine::engine::Ieee802154Engine::buffers_dma_visible`]).
pub const IEEE802154_DMA_WINDOW: core::ops::Range<usize> = 0x2f00_0000..0x2f08_0000;

/// Validation entries of the IEEE 802.15.4 coexistence and BTBB writes.
#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub use validation::coex_trace;

pub use lifecycle::{
    IEEE802154_MAX_CHANNEL, IEEE802154_MIN_CHANNEL, Ieee802154Channel, Ieee802154ChannelError,
    Ieee802154FoundationCheckpoint, Ieee802154ReadbackError, Ieee802154ResetCheckpoint,
    Ieee802154ResetReadback,
};

pub use policy::{
    IEEE802154_ACK_TIMEOUT_QUANTUM_MICROSECONDS, IEEE802154_MAX_ACK_TIMEOUT_MICROSECONDS,
    Ieee802154AckTimeout, Ieee802154AckTimeoutError, Ieee802154CcaMode, Ieee802154MacControl,
    Ieee802154MacPolicy, Ieee802154MacPolicyCheckpoint, Ieee802154PanIdentity,
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

pub use tx_power::ESP32S31_TX_POWER_LEVELS;
