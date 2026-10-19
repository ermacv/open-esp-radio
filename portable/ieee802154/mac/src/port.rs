//! The IEEE 802.15.4 radio values every backend shares: its capabilities
//! ([`Ieee802154Capabilities`]) and the radio configuration
//! ([`RadioSetting`]): the state the backend reads
//! from its own interrupt context (MAC keys and frame counters, the CSL
//! receiver, the enhanced-ACK generator), and the transmit power and
//! clear-channel assessment the next operation publishes. The port itself
//! is the owner shape of [`crate::owner`].

pub use oer_radio_port::{CancelError, ClockError};
pub use oer_radio_port::{ClockInfo, EventsLost, Poisoned, PortResult};

use crate::mac::link_metrics::ProbingInitiator;
use crate::radio::{
    capabilities::RadioCapabilities,
    channel::Channel,
    command::CcaMode,
    interface::Interface,
    power::{
        CcaThresholdCapability, CcaThresholdError, TransmitPowerCapability, TransmitPowerError,
    },
};

/// What a backend supports, read before submission.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Ieee802154Capabilities {
    /// The optional operations it implements.
    pub operations: RadioCapabilities,
    /// Its addressing interfaces: one without
    /// [`RadioCapabilities::MULTI_PAN`].
    pub interfaces: u8,
    /// The transmit power it radiates, for [`RadioSetting::TransmitPowerDbm`],
    /// [`RadioSetting::ChannelTransmitPowerDbm`] and a transmission's own
    /// power; `None` when the power is fixed.
    pub transmit_power: Option<TransmitPowerCapability>,
    /// The clear-channel assessment energy thresholds it accepts
    /// ([`RadioSetting::CcaThresholdDbm`]); `None` when it has none.
    pub cca_threshold: Option<CcaThresholdCapability>,
    /// Its receive sensitivity in dBm: the weakest signal it receives.
    pub receive_sensitivity_dbm: i8,
    /// The timing a CSL peer relies on ([`RadioSetting::Csl`]); `None`
    /// when it publishes none.
    pub csl: Option<CslCapability>,
    /// The header IE bytes every enhanced ACK carries at most
    /// ([`RadioSetting::EnhancedAckHeaderIes`]).
    pub enhanced_ack_header_ies: u8,
}

/// The timing of a backend's Coordinated Sampled Listening, as a CSL
/// receiver reports it to its peers (`otPlatRadioGetCslAccuracy`,
/// `otPlatRadioGetCslUncertainty`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct CslCapability {
    /// The drift of the clock that times the samples, in ± ppm.
    pub accuracy_ppm: u8,
    /// The fixed uncertainty of a sample time, in units of 10 microseconds.
    pub uncertainty_10us: u8,
}

/// How a frame counter changes.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FrameCounterUpdate {
    /// Replace the counter (`otPlatRadioSetMacFrameCounter`).
    Set(u32),
    /// Raise the counter to at least this value
    /// (`otPlatRadioSetMacFrameCounterIfLarger`).
    SetIfLarger(u32),
}

/// The CSL receiver state of the radio (`otPlatRadioEnableCsl`,
/// `otPlatRadioUpdateCslSampleTime`).
///
/// With a period, generated enhanced ACKs carry a CSL IE, every frame the
/// radio sends gets the period and the phase to the next sample time
/// written into its CSL IE when its SFD goes out, and retransmissions take
/// a new frame counter.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct CslReceiver {
    /// The CSL period in units of ten symbols; zero disables CSL.
    pub period: u16,
    /// The next sample time, in the low 32 bits of the radio clock.
    pub sample_time: u32,
}

/// How the radio generates enhanced acknowledgements of IEEE 802.15.4-2015
/// frames.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct EnhancedAckGeneration {
    /// The noise floor in dBm Link Metrics probing measures link margins
    /// from.
    pub noise_floor_dbm: i8,
}

/// A radio setting: state outside an operation.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum RadioSetting<'a> {
    /// Replace the MAC keys of `interface` (`otPlatRadioSetMacKey`),
    /// keeping its frame counter; an interface without keys starts from
    /// [`MacKeys::ZEROED`](crate::MacKeys::ZEROED).
    MacKeys {
        /// The interface whose keys change.
        interface: Interface,
        /// The key index of `current`.
        key_id: u8,
        /// The key of key index `key_id - 1`.
        previous: [u8; 16],
        /// The key of key index `key_id`.
        current: [u8; 16],
        /// The key of key index `key_id + 1`.
        next: [u8; 16],
    },
    /// Change the frame counter of `interface`; an interface without keys
    /// starts from [`MacKeys::ZEROED`](crate::MacKeys::ZEROED).
    FrameCounter {
        /// The interface whose frame counter changes.
        interface: Interface,
        /// The change.
        update: FrameCounterUpdate,
    },
    /// Forget the MAC keys of `interface`: its secured frames go out as
    /// given and secured enhanced ACKs to its frames are refused.
    RemoveMacKeys {
        /// The interface whose keys go.
        interface: Interface,
    },
    /// Replace the CSL receiver state.
    Csl(CslReceiver),
    /// Generate enhanced ACKs with the header IEs and probing initiators
    /// set so far, or refuse every enhanced ACK (`None`), keeping them.
    EnhancedAck(Option<EnhancedAckGeneration>),
    /// Replace the header IEs every enhanced ACK carries; kept while
    /// generation is off.
    EnhancedAckHeaderIes(&'a [u8]),
    /// Replace the Link Metrics probing initiators, most recently added
    /// first ([`EnhAckProbing::replace`](crate::EnhAckProbing::replace));
    /// kept while generation is off.
    EnhancedAckProbing(&'a [ProbingInitiator]),
    /// Set the transmit power of every channel in dBm
    /// (`esp_ieee802154_set_txpower`, `otPlatRadioSetTransmitPower`).
    TransmitPowerDbm(i8),
    /// Set the transmit power of one channel in dBm
    /// (`esp_ieee802154_set_power_with_channel`).
    ChannelTransmitPowerDbm {
        /// The channel whose power changes.
        channel: Channel,
        /// Transmit power in dBm.
        power_dbm: i8,
    },
    /// Set the energy threshold of clear-channel assessment in dBm
    /// (`esp_ieee802154_set_cca_threshold`,
    /// `otPlatRadioSetCcaEnergyDetectThreshold`).
    CcaThresholdDbm(i8),
    /// Set how clear-channel assessment decides that the channel is busy
    /// (`esp_ieee802154_set_cca_mode`); refused
    /// [`SettingError::Unsupported`] without
    /// [`RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT`].
    CcaMode(CcaMode),
}

/// Why the backend refused a [`RadioSetting`]; nothing changed.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SettingError {
    /// The setting names an interface the radio does not have.
    UnknownInterface {
        /// The rejected interface.
        interface: Interface,
        /// The radio's interface count.
        interfaces: u8,
    },
    /// The header IEs exceed what an enhanced ACK carries.
    HeaderIesTooLong {
        /// The IE bytes the backend holds at most.
        capacity: usize,
    },
    /// The transmit power was refused.
    TransmitPower(TransmitPowerError),
    /// The CCA threshold was refused.
    CcaThreshold(CcaThresholdError),
    /// The backend does not implement the setting.
    Unsupported,
}
