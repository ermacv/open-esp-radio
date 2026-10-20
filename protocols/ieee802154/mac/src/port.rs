//! The IEEE 802.15.4 radio values every backend shares: its capabilities
//! ([`Ieee802154Capabilities`]) and the radio configuration
//! ([`RadioSettings`](crate::owner::RadioSettings)): the state the backend
//! reads from its own interrupt context (MAC keys and frame counters, the CSL
//! receiver, the enhanced-ACK generator), and the transmit power and
//! clear-channel assessment the next transmission publishes. Configuration is
//! admitted in any radio state, while the interface's client lives and its
//! operation runs. Calls replace latest values without queuing or capacity
//! failure. Identity, keys, source match and ACK timeout are in force when
//! the call returns; power and CCA at the next transmission; receive-side
//! values within one frame, at once while listening with no frame in progress. The port itself
//! is the owner shape of [`crate::owner`].

pub use oer_radio_port::{CancelError, ClockError};
pub use oer_radio_port::{ClockInfo, EventsLost, Poisoned, PortResult};

use crate::radio::{
    capabilities::RadioCapabilities,
    power::{CcaThresholdCapability, TransmitPowerCapability},
};

/// What a backend supports, read before submission.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Ieee802154Capabilities {
    /// The optional operations it implements.
    pub operations: RadioCapabilities,
    /// Its addressing interfaces: one without
    /// [`RadioCapabilities::MULTI_PAN`].
    pub interfaces: u8,
    /// The transmit power it radiates, for `RadioSettings::set_transmit_power`,
    /// `RadioSettings::set_channel_power` and a transmission's own power; `None` when the power is fixed.
    pub transmit_power: Option<TransmitPowerCapability>,
    /// The clear-channel assessment energy thresholds it accepts
    /// (`RadioSettings::set_cca_threshold`); `None` when it has none.
    pub cca_threshold: Option<CcaThresholdCapability>,
    /// Its receive sensitivity in dBm: the weakest signal it receives.
    pub receive_sensitivity_dbm: i8,
    /// The timing a CSL peer relies on (`RadioSettings::set_csl`); `None`
    /// when it publishes none.
    pub csl: Option<CslCapability>,
    /// The header IE bytes every enhanced ACK carries at most
    /// (`RadioSettings::set_enhanced_ack_header_ies`).
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

/// A monotonic update of the frame counter. Exact replacement is available
/// only in the same request as new keys (`Ieee802154Client::set_mac_keys`).
///
/// ```compile_fail,E0599
/// use oer_ieee802154_mac::FrameCounterUpdate;
/// let rollback = FrameCounterUpdate::Set(0);
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum FrameCounterUpdate {
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
