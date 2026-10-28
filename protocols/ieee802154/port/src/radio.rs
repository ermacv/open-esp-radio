//! Portable radio values: capabilities, channels, interfaces, transmit power,
//! the transmission and receive values the owner shape carries, correlation
//! identifiers and monotonic timestamps.

/// Published backend operation capabilities.
pub mod capabilities;
/// Validated channels in the implemented 2.4 GHz profile.
pub mod channel;
/// Transmission values: channel access, security and CCA mode.
pub mod command;
/// Backend observations and normalized receive metadata.
pub mod event;
/// Addressing interfaces of a multi-PAN radio.
pub mod interface;
/// Transmit power and CCA threshold capabilities.
pub mod power;

/// Caller-assigned identifier used to correlate an operation and completion.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct RequestId(u32);

impl RequestId {
    /// Preserve one caller-owned identifier.
    pub const fn new(value: u32) -> Self {
        Self(value)
    }

    /// Return the caller-owned identifier image.
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl oer_radio_port::Correlation for RequestId {
    fn from_raw(raw: u32) -> Self {
        Self(raw)
    }

    fn raw(self) -> u32 {
        self.0
    }
}

/// The clock domain of the IEEE 802.15.4 radio port: its instants never mix
/// with another port's.
pub enum Ieee802154Radio {}

/// Microseconds in the backend's monotonic radio epoch.
///
/// The epoch is deliberately not wall-clock time. An adapter must use one
/// stable epoch for every timestamp it publishes in a controller instance.
pub type Ieee802154Instant = oer_time::RadioInstant<Ieee802154Radio>;
