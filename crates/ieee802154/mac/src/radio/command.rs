//! Caller-owned values of a transmission: how it acquires the channel, who
//! secures it, and how clear-channel assessment decides.

use super::Ieee802154Instant;

/// How one transmit request should acquire the channel.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum TxMode {
    /// Start transmission without a preceding clear-channel assessment.
    Direct,
    /// Perform one clear-channel assessment before transmission.
    ClearChannelAssessment,
    /// Apply bounded CSMA-CA with the supplied maximum backoff count.
    CsmaCa {
        /// Maximum number of backoffs before returning channel-busy.
        max_backoffs: u8,
    },
    /// Start at a monotonic backend timestamp without implicit CSMA-CA,
    /// optionally after one clear-channel assessment that ends at that time.
    Scheduled {
        /// Requested start time.
        at: Ieee802154Instant,
        /// Assess the channel first (ESP-IDF `esp_ieee802154_transmit_at`
        /// with `cca`, as OpenThread's `mCsmaCaEnabled` asks).
        cca: bool,
    },
}

/// Who secures a frame whose security-enabled bit is set, as the transmit
/// information of OpenThread's `otRadioFrame` (`mIsSecurityProcessed`,
/// `mIsARetx`) tells a radio that claims `OT_RADIO_CAPS_TRANSMIT_SEC`.
///
/// A radio without MAC keys transmits every frame as given.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub enum TxSecurity {
    /// The radio secures the frame with its MAC keys and writes a new frame
    /// counter and, in key identifier mode 1, its key index.
    #[default]
    Radio,
    /// The frame retransmits one the radio secured: the radio secures it
    /// again under the frame counter and key index it already carries.
    Retransmission,
    /// The upper layer already secured the frame; the radio transmits it
    /// as given.
    Processed,
    /// The upper layer wrote the frame's auxiliary security header and frame
    /// counter; the radio applies CCM* to every attempt with `key` and the
    /// nonce source `address`, instead of its MAC keys
    /// (`esp_ieee802154_set_transmit_security`).
    Key {
        /// The key.
        key: [u8; 16],
        /// The extended address of the nonce, in frame byte order.
        address: [u8; 8],
    },
}

/// How clear-channel assessment decides that the channel is busy
/// (IEEE 802.15.4 CCA modes, `esp_ieee802154_cca_mode_t`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum CcaMode {
    /// Busy when a carrier is detected (mode 2).
    Carrier,
    /// Busy when the energy is above the threshold (mode 1).
    EnergyDetection,
    /// Busy when a carrier is detected or the energy is above the threshold.
    CarrierOrEnergyDetection,
    /// Busy when a carrier is detected and the energy is above the threshold
    /// (mode 3).
    CarrierAndEnergyDetection,
}
