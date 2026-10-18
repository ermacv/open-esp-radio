#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! The portable IEEE 802.15.4 radio contract (`oer-i`) over the
//! ESP32-S31 MAC engine ported from the public ESP-IDF driver.
//!
//! [`IRadio`] admits each [`RadioCommand`] through the portable
//! [`RadioStateMachine`], starts the corresponding engine operation and
//! translates engine notifications into correlated [`RadioEvent`] values for
//! a [`IRadioSink`]. It names no executor and holds no hardware: the
//! caller passes the LL port to each entry and serializes the entries, as
//! the runtime does under its lock.
//!
//! The composition owns power, clocks, PHY and the MAC initialization; the
//! role's [`IRadio::enable`] and [`IRadio::disable`] only
//! open and close admission.
//! Receive-ring slots never outlive one delivery: a frame is lent to the
//! sink and its slot returns to the ring before the entry ends.

#[cfg(test)]
extern crate std;

use oer_espressif_ieee802154_engine::engine::{
    FRAME_SIZE, Ieee802154Engine, Ieee802154EnhancedAck, Ieee802154Environment,
    Ieee802154FrameInfo, Ieee802154ReceivedAck, Ieee802154RxSlot, Ieee802154State,
    Ieee802154TxError, RECEIVE_SENSITIVITY_DBM,
};
use oer_espressif_ieee802154_engine::{
    channel::Ieee802154Channel,
    ll::Ieee802154LowLevel,
    pib::CCA_THRESHOLD_RANGE_DBM,
    types::{Ieee802154CcaMode, Ieee802154MultipanIndex, Ieee802154RxStatus},
};
use oer_ieee802154::{
    AcceptedCommand, AppliedSecurity, AttemptFailure, CSL_IE_TEMPLATE, CancelError, CcaMode,
    CcaThresholdCapability, CcaThresholdError, Channel, CommandError, Configuration, CslCapability,
    CsmaCa, ENH_ACK_PROBING_IE_CAPACITY, EnhAckProbing, FcsStatus, FramePending, FrameRetries,
    FrameType, FrameVersion, FrameView, Ieee802154Capabilities, Ieee802154Instant, Interface,
    InterfaceSetting, KeyIdMode, MacKeys, PendingTableHalf, PhrFrame, RadioCapabilities,
    RadioCommand, RadioEvent, RadioFault, RadioState, RadioStateMachine, ReceivedFrame, RequestId,
    RestingState, RetryStart, RxMetadata, SecurityStatus, SentAcknowledgement, TimeSync,
    TransmitPowerCapability, TransmitPowerError, TxMode, TxSecurity, TxStatus, csl_phase,
    generate_enhanced_ack, write_csl_ie,
};
use oer_time::Clock;

/// The portable capabilities the role implements over any engine.
///
/// Source matching without an interface configures the frame-pending table
/// of interface zero, as ESP-IDF's single-interface API does. An engine
/// built with multi-PAN adds [`RadioCapabilities::MULTI_PAN`].
pub const IEEE802154_RADIO_CAPABILITIES: RadioCapabilities = RadioCapabilities::NONE
    .union(RadioCapabilities::CLEAR_CHANNEL_ASSESSMENT)
    .union(RadioCapabilities::CSMA_CA)
    .union(RadioCapabilities::TRANSMIT_RETRIES)
    .union(RadioCapabilities::SECURITY_OFFLOAD)
    .union(RadioCapabilities::ENERGY_SCAN)
    .union(RadioCapabilities::HARDWARE_ACKNOWLEDGEMENT)
    .union(RadioCapabilities::SCHEDULED_TRANSMIT)
    .union(RadioCapabilities::SCHEDULED_RECEIVE)
    .union(RadioCapabilities::PROMISCUOUS)
    .union(RadioCapabilities::RECEIVE_TIMESTAMP)
    .union(RadioCapabilities::AUTOMATIC_ACKNOWLEDGEMENT)
    .union(RadioCapabilities::SOURCE_MATCH)
    .union(RadioCapabilities::TIME_SYNC);

/// The CSL timing ESP-IDF's OpenThread port reports for the radio: a drift
/// of ± 50 ppm (`CONFIG_OPENTHREAD_CSL_ACCURACY` default) and an uncertainty
/// of 50 × 10 microseconds (`CONFIG_OPENTHREAD_CSL_UNCERTAIN` default).
pub const IEEE802154_CSL_CAPABILITY: CslCapability = CslCapability {
    accuracy_ppm: 50,
    uncertainty_10us: 50,
};

/// Header IE bytes an enhanced ACK carries at most (OpenThread
/// `OT_ACK_IE_MAX_SIZE`).
pub const IEEE802154_ENHANCED_ACK_IE_CAPACITY: usize = 16;

/// Probing initiators the enhanced-ACK generator serves at most:
/// `OPENTHREAD_CONFIG_MLE_LINK_METRICS_MAX_SERIES_SUPPORTED`, which is
/// OpenThread's `OPENTHREAD_CONFIG_MLE_MAX_CHILDREN`, ten in ESP-IDF's
/// default configuration (`CONFIG_OPENTHREAD_MLE_MAX_CHILDREN`).
pub const IEEE802154_ENH_ACK_PROBING_CAPACITY: usize = 10;

/// The CSL receiver state of ESP-IDF's OpenThread port (`s_csl_period`,
/// `s_csl_sample_time`; `otPlatRadioEnableCsl`,
/// `otPlatRadioUpdateCslSampleTime`).
///
/// With a period, generated enhanced ACKs carry a CSL IE, every frame the
/// radio sends gets the period and the phase to the next sample time
/// written into its CSL IE when its SFD goes out, and retransmissions take
/// a new frame counter.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct Ieee802154Csl {
    /// The CSL period in units of ten symbols; zero disables CSL.
    pub period: u16,
    /// The next sample time, in the low 32 bits of the radio clock.
    pub sample_time: u32,
}

/// The header IEs exceed [`IEEE802154_ENHANCED_ACK_IE_CAPACITY`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154EnhancedAckIeTooLong;

/// The enhanced-ACK generator of ESP-IDF's OpenThread port
/// (`ot_radio_enh_ack_generator` of `esp_openthread_radio.c` at ESP-IDF
/// ``), run inside the interrupt
/// handler for each received 2015 frame that requests an ACK.
///
/// It builds the ACK with [`generate_enhanced_ack`], carrying, in the port's
/// order, the CSL IE of a CSL receiver, the enhanced-ACK probing IE of
/// Link Metrics when the frame's source is a probing initiator
/// ([`Self::probing`]), then the configured header IEs, and secures the ACK of a secured frame with the [`MacKeys`]
/// of the interface the frame matched ([`IRadio::interface_mac_keys`]),
/// as the multi-instance port does: the next frame counter and the key of
/// the frame's key index. Without keys, or when the frame cannot
/// be acknowledged, the generator refuses and the engine delivers the frame
/// without an ACK.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Ieee802154EnhancedAckGenerator {
    header_ies: [u8; IEEE802154_ENHANCED_ACK_IE_CAPACITY],
    header_ies_len: usize,
    probing: EnhAckProbing<IEEE802154_ENH_ACK_PROBING_CAPACITY>,
}

impl Ieee802154EnhancedAckGenerator {
    /// A generator without header IEs or probing initiators, measuring
    /// link margins from a zero noise floor.
    pub const fn new() -> Self {
        Self {
            header_ies: [0; IEEE802154_ENHANCED_ACK_IE_CAPACITY],
            header_ies_len: 0,
            probing: EnhAckProbing::new(0),
        }
    }

    /// The Link Metrics probing initiators and the noise floor their link
    /// margins are measured from (`otPlatRadioConfigureEnhAckProbing`).
    /// An ACK to a frame from an initiator carries the probing IE with the
    /// frame's metrics, as the port's `otLinkMetricsEnhAckGenData` gives
    /// them. ESP-IDF's port never sets the noise floor, so its link margin
    /// is zero; the caller sets it, and the OpenThread adapter uses the
    /// receive sensitivity it reports.
    pub fn probing(&mut self) -> &mut EnhAckProbing<IEEE802154_ENH_ACK_PROBING_CAPACITY> {
        &mut self.probing
    }

    /// The header IEs every ACK carries.
    pub fn header_ies(&self) -> &[u8] {
        &self.header_ies[..self.header_ies_len]
    }

    /// Replace the header IEs.
    ///
    /// # Errors
    ///
    /// The IEs exceed [`IEEE802154_ENHANCED_ACK_IE_CAPACITY`]; nothing
    /// changes.
    pub fn set_header_ies(&mut self, ies: &[u8]) -> Result<(), Ieee802154EnhancedAckIeTooLong> {
        let target = self
            .header_ies
            .get_mut(..ies.len())
            .ok_or(Ieee802154EnhancedAckIeTooLong)?;
        target.copy_from_slice(ies);
        self.header_ies_len = ies.len();
        Ok(())
    }

    fn generate(
        &mut self,
        frame: &[u8; FRAME_SIZE],
        info: &Ieee802154FrameInfo,
        ack: &mut [u8; FRAME_SIZE],
        security: &mut RadioSecurity,
    ) -> Ieee802154EnhancedAck {
        // The PHR counts the RSSI and LQI bytes that replace the FCS.
        let length = usize::from(frame[0] & 0x7f);
        let Some(received) = length
            .checked_sub(1)
            .and_then(|end| frame.get(1..end))
            .and_then(|bytes| FrameView::new(bytes).ok())
        else {
            return Ieee802154EnhancedAck::Refused;
        };
        // The port's generator puts the CSL IE template first, then the
        // probing IE of a Link Metrics initiator.
        let mut ies = [0; CSL_IE_TEMPLATE.len()
            + ENH_ACK_PROBING_IE_CAPACITY
            + IEEE802154_ENHANCED_ACK_IE_CAPACITY];
        let mut ies_len = 0;
        if security.csl.period != 0 {
            ies[..CSL_IE_TEMPLATE.len()].copy_from_slice(&CSL_IE_TEMPLATE);
            ies_len = CSL_IE_TEMPLATE.len();
        }
        if let Ok(Some(source)) = PhrFrame::new(frame).source_address() {
            let data = self.probing.data(source, info.lqi, info.rssi);
            ies_len += data.write_ie(&mut ies[ies_len..]);
        }
        ies[ies_len..ies_len + self.header_ies_len]
            .copy_from_slice(&self.header_ies[..self.header_ies_len]);
        ies_len += self.header_ies_len;
        let header_ies = &ies[..ies_len];
        let Ok(mut generated) = generate_enhanced_ack(received, info.pending, header_ies) else {
            return Ieee802154EnhancedAck::Refused;
        };
        let key = match generated.security() {
            Some(header) => {
                // The multi-instance port secures the ACK with the keys of
                // the interface the frame matched, and refuses a frame no
                // single interface claims.
                let Some(interface) = info
                    .mpf_index
                    .and_then(|index| security.interfaces.get_mut(usize::from(index.value())))
                else {
                    return Ieee802154EnhancedAck::Refused;
                };
                let Some(keys) = interface.keys.as_mut() else {
                    return Ieee802154EnhancedAck::Refused;
                };
                let frame_counter = keys.frame_counter();
                let Some(key) = keys.secure(&mut generated) else {
                    return Ieee802154EnhancedAck::Refused;
                };
                // The port's `s_ack_frame_counter` and `s_ack_key_id`.
                interface.enhanced_ack = Some(AppliedSecurity {
                    frame_counter,
                    key_id: header.key_index,
                });
                Some(key)
            }
            None => None,
        };
        let bytes = generated.bytes();
        // The PHR counts the FCS the hardware appends.
        ack[0] = (bytes.len() + 2) as u8;
        ack[1..=bytes.len()].copy_from_slice(bytes);
        key.map_or(Ieee802154EnhancedAck::Generated, |key| {
            Ieee802154EnhancedAck::Secured { key }
        })
    }
}

/// The radio's random words: one for each CSMA-CA backoff and each delay
/// before a retry, as OpenThread draws one from its non-cryptographic
/// generator. The radio holds its source by value and calls it directly, so
/// the interrupt path's call graph names the one implementation.
pub trait Ieee802154Random {
    /// A uniform random word.
    fn random(&mut self) -> u32;
}

/// Why a transmission waits before its next attempt.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Delay {
    /// A CSMA-CA backoff (`kStateCsmaBackoff`).
    CsmaBackoff,
    /// The random delay before a retry after a missing acknowledgement
    /// (`kStateDelayBeforeRetx`).
    Retransmission(RetryStart),
}

/// Where a transmission stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Phase {
    /// A delay must start: [`IRadio::take_delay`].
    DelayDue(Delay),
    /// The delay runs; [`IRadio::delay_elapsed`] ends it.
    Delaying(Delay),
    /// An attempt is on the air.
    Attempting,
}

/// The CCM* key and nonce source a request carries
/// ([`TxSecurity::Key`]).
#[derive(Clone, Copy)]
struct ArmedSecurity {
    key: [u8; 16],
    address: [u8; 8],
}

/// One transmission as OpenThread `SubMac` runs it over ESP-IDF's radio,
/// which makes one attempt per transmit: CSMA-CA backoffs, retries and the
/// delays before them. Every attempt arms the transmission's security, as
/// ESP-IDF's OpenThread port arms it in each `otPlatRadioTransmit`; the
/// engine clears it when an attempt ends.
struct Transmission {
    /// The `[PHR, MAC..., FCS]` image each attempt transmits.
    image: [u8; FRAME_SIZE],
    mode: TxMode,
    csma: CsmaCa,
    retries: FrameRetries,
    security: Option<ArmedSecurity>,
    /// Who secures the frame when it carries no key.
    tx_security: TxSecurity,
    /// The security header fields an attempt wrote into the frame.
    applied: Option<AppliedSecurity>,
    /// The interface whose keys and extended address secure the frame.
    interface: Ieee802154MultipanIndex,
    phase: Phase,
    /// A cancellation stopped the transmission: a failed attempt ends it
    /// instead of backing off or retrying.
    cancelled: bool,
}

impl Transmission {
    fn image(&self) -> &[u8] {
        &self.image[..=usize::from(self.image[0])]
    }

    const fn max_backoffs(&self) -> u8 {
        match self.mode {
            TxMode::CsmaCa { max_backoffs } => max_backoffs,
            _ => 0,
        }
    }

    /// `SubMac::StartCsmaBackoff`: back off before a CSMA-CA attempt, or
    /// attempt at once.
    fn start_access<L, C: Clock + ?Sized>(
        &mut self,
        engine: &mut Ieee802154Engine<'_>,
        ll: &mut L,
        env: &mut Collector<'_, C>,
    ) where
        L: Ieee802154LowLevel + ?Sized,
    {
        if matches!(self.mode, TxMode::CsmaCa { .. }) && self.csma.backoff_micros(0).is_some() {
            self.start_delay(Delay::CsmaBackoff, engine, ll, env);
        } else {
            self.attempt(engine, ll, env);
        }
    }

    /// `SubMac::StartTimerForBackoff`: wait receiving on the transmit
    /// channel when the radio receives when idle, otherwise asleep.
    fn start_delay<L, C: Clock + ?Sized>(
        &mut self,
        delay: Delay,
        engine: &mut Ieee802154Engine<'_>,
        ll: &mut L,
        env: &mut Collector<'_, C>,
    ) where
        L: Ieee802154LowLevel + ?Sized,
    {
        self.phase = Phase::DelayDue(delay);
        if engine.pib().rx_when_idle() {
            engine.receive(ll, env);
        } else {
            engine.sleep(ll, env);
        }
    }

    /// `SubMac::BeginTransmit`: one attempt in the request's channel access,
    /// secured as `otPlatRadioTransmit` secures it.
    fn attempt<L, C: Clock + ?Sized>(
        &mut self,
        engine: &mut Ieee802154Engine<'_>,
        ll: &mut L,
        env: &mut Collector<'_, C>,
    ) where
        L: Ieee802154LowLevel + ?Sized,
    {
        if let Some(security) = self.security {
            engine.set_transmit_security(ll, self.image(), &security.key, &security.address);
        } else if let Some(interface) = env
            .security
            .interfaces
            .get_mut(usize::from(self.interface.value()))
            && let Some(keys) = interface.keys.as_mut()
            && self.tx_security != TxSecurity::Processed
            && PhrFrame::new(self.image()).security_enabled()
        {
            // The port's transmit security: a new frame counter and key index
            // unless this retransmits the frame, the current key, and the
            // extended address read for key identifier mode 1.
            let retransmission =
                self.tx_security == TxSecurity::Retransmission || self.retries.retries() > 0;
            let csl = env.security.csl.period != 0;
            let security = keys.transmit_security_with_csl(retransmission, csl);
            let length = usize::from(self.image[0]) + 1;
            let mode = security.apply(&mut self.image[..length]);
            if mode == Some(KeyIdMode::Index) {
                interface.address = ll.multipan_extended_address(self.interface);
            }
            if let Some(applied) = mode.and_then(|mode| security.applied(mode)) {
                self.applied = Some(applied);
            }
            let address = interface.address;
            engine.set_transmit_security(ll, self.image(), &security.key, &address);
        }
        self.phase = Phase::Attempting;
        let image = self.image();
        let started = match self.mode {
            TxMode::Direct => engine.transmit(ll, env, image, false),
            TxMode::ClearChannelAssessment | TxMode::CsmaCa { .. } => {
                engine.transmit(ll, env, image, true)
            }
            // The engine counts in the low 32 bits of the radio clock.
            TxMode::Scheduled { at, cca } => {
                engine.transmit_at(ll, env, image, cca, at.as_micros() as u32)
            }
        };
        started.expect("a validated MAC frame fits one DMA frame");
    }
}

/// What the role does after a notification.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Follow {
    Nothing,
    /// Resume receive on the resting channel an operation left.
    Restore(Ieee802154Channel),
    /// Acquire the channel for the transmission's next attempt.
    Access,
    /// Wait before the transmission's next attempt.
    Delay(Delay),
}

/// Receives the correlated portable events of one entry. Frames are lent
/// only for the call.
pub trait Ieee802154RadioSink {
    /// Deliver one event.
    fn event(&mut self, event: RadioEvent<'_>);
}

/// One engine notification the portable contract carries.
#[derive(Clone, Copy)]
enum Notification {
    Received(Ieee802154RxSlot, SentAcknowledgement),
    Transmitted(Option<Ieee802154RxSlot>),
    TransmitFailed(Ieee802154TxError),
    EnergyDetected(i8),
    ClearChannelAssessed {
        busy: bool,
    },
    MeasurementFailed,
    /// A scheduled receive window ended (`esp_i_receive_at_done`).
    ScheduledReceiveDone,
}

/// Notifications one engine entry can raise.
const NOTIFICATIONS: usize = 8;

/// The engine environment of one entry: notifications are collected and
/// translated after the engine returns.
struct Collector<'role, C: Clock + ?Sized> {
    /// The radio clock (`esp_timer_get_time`), read fresh at each engine
    /// event: the engine truncates it to the vendor's wrapping 32-bit timer
    /// domain, and receive timestamps use it as the radio epoch.
    clock: &'role C,
    enhanced_ack: &'role mut Option<Ieee802154EnhancedAckGenerator>,
    security: &'role mut RadioSecurity,
    notifications: [Option<Notification>; NOTIFICATIONS],
}

/// The security the radio keeps for one interface, as ESP-IDF's
/// multi-instance OpenThread port keeps it per instance (`s_sec_ctx`).
#[derive(Clone, Copy, Default)]
struct InterfaceSecurity {
    /// Keys and frame counter of secured transmissions and enhanced ACKs.
    keys: Option<MacKeys>,
    /// The port's `security_addr`: the extended address of the interface's
    /// last key identifier mode 1 transmission.
    address: [u8; 8],
    /// The security of the enhanced ACK generated since the interface's
    /// last received frame (`with_security_enh_ack`).
    enhanced_ack: Option<AppliedSecurity>,
}

impl InterfaceSecurity {
    const NONE: Self = Self {
        keys: None,
        address: [0; 8],
        enhanced_ack: None,
    };
}

/// The security and CSL state the radio keeps for the stack, as ESP-IDF's
/// OpenThread port keeps it in its statics: security per interface, CSL
/// for the radio.
#[derive(Default)]
struct RadioSecurity {
    /// Indexed by the interface.
    interfaces: [InterfaceSecurity; Ieee802154MultipanIndex::COUNT as usize],
    /// The CSL receiver state.
    csl: Ieee802154Csl,
    /// The Time IE of the running transmission.
    time_sync: Option<TimeSync>,
}

type Notifications = [Option<Notification>; NOTIFICATIONS];

impl<'role, C: Clock + ?Sized> Collector<'role, C> {
    const fn new(
        clock: &'role C,
        enhanced_ack: &'role mut Option<Ieee802154EnhancedAckGenerator>,
        security: &'role mut RadioSecurity,
    ) -> Self {
        Self {
            clock,
            enhanced_ack,
            security,
            notifications: [None; NOTIFICATIONS],
        }
    }

    fn push(&mut self, notification: Notification) {
        let free = self
            .notifications
            .iter_mut()
            .find(|entry| entry.is_none())
            .expect("one engine entry raises at most eight notifications");
        *free = Some(notification);
    }
}

impl<C: Clock + ?Sized> Ieee802154Environment for Collector<'_, C> {
    fn now_micros(&mut self) -> u64 {
        self.clock.now().as_micros()
    }

    fn receive_done(
        &mut self,
        slot: Ieee802154RxSlot,
        frame: &[u8; FRAME_SIZE],
        info: &Ieee802154FrameInfo,
    ) {
        // `ot_radio_receive_done`: the secured enhanced ACK of the matched
        // interface belongs to a 2015 frame that requested one; it is
        // forgotten either way.
        let frame = PhrFrame::new(frame);
        let enhanced = frame.ack_required() && frame.version() == FrameVersion::V2015;
        let security = info
            .mpf_index
            .and_then(|index| self.security.interfaces.get_mut(usize::from(index.value())))
            .and_then(|interface| interface.enhanced_ack.take())
            .filter(|_| enhanced);
        self.push(Notification::Received(
            slot,
            SentAcknowledgement {
                frame_pending: info.pending,
                security,
            },
        ));
    }

    fn receive_sfd_done(&mut self) {}

    fn transmit_done(&mut self, _frame: &[u8; FRAME_SIZE], ack: Option<Ieee802154ReceivedAck<'_>>) {
        self.push(Notification::Transmitted(ack.map(|ack| ack.slot)));
    }

    fn transmit_failed(&mut self, _frame: &[u8; FRAME_SIZE], error: Ieee802154TxError) {
        self.push(Notification::TransmitFailed(error));
    }

    /// `ot_radio_transmit_sfd_done`: with CSL, the CSL IE of the frame or
    /// enhanced ACK going out gets the period and the phase to the next
    /// sample time, from the radio clock at its SFD; the Time IE of a
    /// transmitted frame - never of an enhanced ACK - gets the time sync
    /// sequence and the network time.
    fn transmit_sfd_done(&mut self, frame: &mut [u8; FRAME_SIZE]) {
        let csl = self.security.csl;
        let now_micros = self.clock.now().as_micros();
        let length = usize::from(frame[0] & 0x7f) + 1;
        if let Some(phase) = csl_phase(now_micros as u32, csl.sample_time, csl.period) {
            write_csl_ie(&mut frame[..length], csl.period, phase);
        }
        // The port writes the Time IE only into its transmit frame; the
        // radio's only other frame, the enhanced ACK, is an ACK frame.
        if let Some(sync) = self.security.time_sync
            && PhrFrame::new(&frame[..]).frame_type() != FrameType::Ack
        {
            sync.write(&mut frame[1..length], now_micros);
        }
    }

    fn energy_detect_done(&mut self, power: i8) {
        self.push(Notification::EnergyDetected(power));
    }

    fn cca_done(&mut self, busy: bool) {
        self.push(Notification::ClearChannelAssessed { busy });
    }

    fn ed_failed(&mut self, _status: Ieee802154RxStatus) {
        self.push(Notification::MeasurementFailed);
    }

    fn receive_at_done(&mut self) {
        self.push(Notification::ScheduledReceiveDone);
    }

    fn generate_enhanced_ack(
        &mut self,
        frame: &[u8; FRAME_SIZE],
        info: &Ieee802154FrameInfo,
        ack: &mut [u8; FRAME_SIZE],
    ) -> Ieee802154EnhancedAck {
        match self.enhanced_ack {
            Some(generator) => generator.generate(frame, info, ack, self.security),
            None => Ieee802154EnhancedAck::Refused,
        }
    }
}

/// How ESP-IDF's OpenThread port reports a failed attempt to `SubMac`: a
/// busy channel, an abort and a coexistence rejection as a channel-access
/// failure, a missing or invalid acknowledgement as no acknowledgement. A
/// security failure is not an attempt outcome it reports.
const fn attempt_failure(error: Ieee802154TxError) -> Option<AttemptFailure> {
    match error {
        Ieee802154TxError::CcaBusy | Ieee802154TxError::Abort | Ieee802154TxError::Coexist => {
            Some(AttemptFailure::ChannelAccess)
        }
        Ieee802154TxError::NoAck | Ieee802154TxError::InvalidAck => {
            Some(AttemptFailure::NoAcknowledgement)
        }
        Ieee802154TxError::Security => None,
    }
}

impl Transmission {
    /// `SubMac::HandleTransmitDone` after a failed attempt: back off again
    /// while CSMA-CA backoffs remain, otherwise retry while retries remain.
    /// `Err` ends the transmission, with the status a CSMA-CA transmission
    /// reports for a channel it could not acquire.
    fn retry(&mut self, failure: AttemptFailure) -> Result<Follow, Option<TxStatus>> {
        if failure == AttemptFailure::ChannelAccess
            && matches!(self.mode, TxMode::CsmaCa { .. })
            && self.csma.channel_busy()
        {
            return Ok(Follow::Access);
        }
        self.csma = CsmaCa::new(self.max_backoffs());
        match self.retries.retry(failure) {
            Some(RetryStart::Now) => Ok(Follow::Access),
            Some(start) => Ok(Follow::Delay(Delay::Retransmission(start))),
            None if failure == AttemptFailure::ChannelAccess
                && matches!(self.mode, TxMode::CsmaCa { .. }) =>
            {
                Err(Some(TxStatus::ChannelBusy))
            }
            None => Err(None),
        }
    }
}

const fn tx_status(error: Ieee802154TxError) -> TxStatus {
    match error {
        Ieee802154TxError::CcaBusy => TxStatus::ChannelBusy,
        Ieee802154TxError::Abort => TxStatus::Aborted,
        Ieee802154TxError::NoAck => TxStatus::NoAcknowledgement,
        Ieee802154TxError::InvalidAck => TxStatus::InvalidAcknowledgement,
        Ieee802154TxError::Coexist => TxStatus::CoexistenceRejected,
        Ieee802154TxError::Security => TxStatus::SecurityFailure,
    }
}

/// The MAC interface of a portable one.
const fn multipan_index(interface: Interface) -> Option<Ieee802154MultipanIndex> {
    Ieee802154MultipanIndex::new(interface.index())
}

/// Apply one interface setting (`esp_i_set_multipan_*`,
/// `esp_i_multipan_*`).
/// Whether admitting `command` writes the MAC registers at once: every
/// command but a configuration of the PIB or the frame-pending table, which
/// the next operation publishes.
pub fn writes_registers(command: &RadioCommand<'_>) -> bool {
    match command {
        RadioCommand::Configure { configuration, .. } => {
            configuration_writes_registers(configuration)
        }
        _ => true,
    }
}

/// Whether `configuration` writes the MAC registers rather than the PIB or
/// the frame-pending table.
const fn configuration_writes_registers(configuration: &Configuration) -> bool {
    match configuration {
        Configuration::PanId(_)
        | Configuration::ShortAddress(_)
        | Configuration::ExtendedAddress(_) => true,
        Configuration::Interface { setting, .. } => matches!(
            setting,
            InterfaceSetting::PanId(_)
                | InterfaceSetting::ShortAddress(_)
                | InterfaceSetting::ExtendedAddress(_)
                | InterfaceSetting::Enabled(_)
        ),
        _ => false,
    }
}

/// Apply a configuration that writes the MAC registers.
fn configure_registers<L: Ieee802154LowLevel + ?Sized>(
    engine: &mut Ieee802154Engine<'_>,
    ll: &mut L,
    configuration: Configuration,
) {
    match configuration {
        Configuration::PanId(panid) => engine.set_panid(ll, panid),
        Configuration::ShortAddress(address) => engine.set_short_address(ll, address),
        Configuration::ExtendedAddress(address) => engine.set_extended_address(ll, address),
        Configuration::Interface { interface, setting } => {
            let index = multipan_index(interface)
                .expect("the state machine admits only the engine's interfaces");
            match setting {
                InterfaceSetting::PanId(panid) => engine.set_multipan_panid(ll, index, panid),
                InterfaceSetting::ShortAddress(address) => {
                    engine.set_multipan_short_address(ll, index, address);
                }
                InterfaceSetting::ExtendedAddress(address) => {
                    engine.set_multipan_extended_address(ll, index, address);
                }
                InterfaceSetting::Enabled(enabled) => {
                    let enable = engine.multipan_enable(ll);
                    let enable = if enabled {
                        enable.with(index)
                    } else {
                        enable.without(index)
                    };
                    engine.set_multipan_enable(ll, enable);
                }
                _ => unreachable!("a software configuration writes no register"),
            }
        }
        _ => unreachable!("a software configuration writes no register"),
    }
}

/// Apply a configuration of the PIB or the frame-pending table.
fn configure_software(engine: &mut Ieee802154Engine<'_>, configuration: Configuration) {
    match configuration {
        Configuration::Promiscuous(enable) => engine.pib().set_promiscuous(enable),
        Configuration::AutomaticAcknowledgement(enable) => {
            engine.pib().set_auto_ack_tx(enable);
        }
        Configuration::PanCoordinator(enable) => engine.pib().set_coordinator(enable),
        Configuration::PendingMode(mode) => engine
            .pib()
            .set_pending_mode(Ieee802154MultipanIndex::CONTEXT0, mode),
        Configuration::AddPendingAddress(address) => {
            // Room was checked before admission.
            let _ = engine.pending_table().add(address);
        }
        Configuration::RemovePendingAddress(address) => {
            engine.pending_table().clear(address);
        }
        Configuration::ResetPendingTable(PendingTableHalf::Short) => {
            engine.pending_table().reset_short();
        }
        Configuration::ResetPendingTable(PendingTableHalf::Extended) => {
            engine.pending_table().reset_extended();
        }
        Configuration::Interface { interface, setting } => {
            let index = multipan_index(interface)
                .expect("the state machine admits only the engine's interfaces");
            configure_interface_software(engine, index, setting);
        }
        Configuration::PanId(_)
        | Configuration::ShortAddress(_)
        | Configuration::ExtendedAddress(_) => {
            unreachable!("a register configuration needs the hardware")
        }
    }
}

/// Apply an interface setting of the PIB or the frame-pending table.
fn configure_interface_software(
    engine: &mut Ieee802154Engine<'_>,
    index: Ieee802154MultipanIndex,
    setting: InterfaceSetting,
) {
    match setting {
        InterfaceSetting::PendingMode(mode) => engine.pib().set_pending_mode(index, mode),
        InterfaceSetting::AddPendingAddress(address) => {
            // Room was checked before admission.
            let _ = engine.pending_table_for(index).add(address);
        }
        InterfaceSetting::RemovePendingAddress(address) => {
            engine.pending_table_for(index).clear(address);
        }
        InterfaceSetting::ResetPendingTable(PendingTableHalf::Short) => {
            engine.pending_table_for(index).reset_short();
        }
        InterfaceSetting::ResetPendingTable(PendingTableHalf::Extended) => {
            engine.pending_table_for(index).reset_extended();
        }
        InterfaceSetting::PanId(_)
        | InterfaceSetting::ShortAddress(_)
        | InterfaceSetting::ExtendedAddress(_)
        | InterfaceSetting::Enabled(_) => {
            unreachable!("a register configuration needs the hardware")
        }
    }
}

fn hal_channel(channel: Channel) -> Ieee802154Channel {
    Ieee802154Channel::new(channel.get()).expect("portable and HAL channels share 11 through 26")
}

/// The portable CCA mode as the MAC's.
const fn hal_cca_mode(mode: CcaMode) -> Ieee802154CcaMode {
    match mode {
        CcaMode::Carrier => Ieee802154CcaMode::Carrier,
        CcaMode::EnergyDetection => Ieee802154CcaMode::EnergyDetection,
        CcaMode::CarrierOrEnergyDetection => Ieee802154CcaMode::CarrierOrEnergyDetection,
        CcaMode::CarrierAndEnergyDetection => Ieee802154CcaMode::CarrierAndEnergyDetection,
    }
}

/// Lend the MAC bytes of a received image: the PHR length counts two
/// trailing bytes that carry RSSI and LQI in place of the FCS.
///
/// RSSI and LQI are read from those bytes, because a frame a stop flushes
/// never had its frame information updated (vendor `stop_rx`); such a frame
/// also reports channel zero, so `flushed_on` names the receive channel.
fn received_frame<'image>(
    image: &'image [u8; FRAME_SIZE],
    info: &Ieee802154FrameInfo,
    acknowledgement: bool,
    flushed_on: Option<Channel>,
) -> Option<ReceivedFrame<'image>> {
    let length = usize::from(image[0] & 0x7f);
    let frame = FrameView::new(image.get(1..length.checked_sub(1)?)?).ok()?;
    let channel = match Channel::new(info.channel) {
        Ok(channel) => channel,
        Err(_) => flushed_on?,
    };
    let frame_pending = if acknowledgement {
        if frame.bytes()[0] & 0x10 == 0 {
            FramePending::Clear
        } else {
            FramePending::Set
        }
    } else {
        FramePending::Unavailable
    };
    Some(ReceivedFrame {
        frame,
        metadata: RxMetadata {
            channel,
            // The S31 RSSI compensation is zero.
            rssi_dbm: image[length - 1] as i8,
            link_quality: image[length],
            timestamp: Some(Ieee802154Instant::from_micros(info.timestamp)),
            fcs: FcsStatus::Valid,
            security: SecurityStatus::Unprocessed,
            frame_pending,
            sent_acknowledgement: SentAcknowledgement::NONE,
            interface: info.mpf_index.map(|index| Interface::new(index.value())),
        },
    })
}

/// The radio role: the engine behind the portable state machine.
pub struct Ieee802154Radio<'storage, R> {
    engine: Ieee802154Engine<'storage>,
    machine: RadioStateMachine,
    random: R,
    enhanced_ack: Option<Ieee802154EnhancedAckGenerator>,
    transmission: Option<Transmission>,
    security: RadioSecurity,
}

impl<'storage, R: Ieee802154Random> Ieee802154Radio<'storage, R> {
    /// Admit portable commands to an engine the composition has enabled and
    /// initialized. The role starts `Disabled` until [`Self::enable`].
    /// An engine built with multi-PAN gives the radio its interfaces.
    pub const fn new(engine: Ieee802154Engine<'storage>, random: R) -> Self {
        let machine = match engine.multipan() {
            Some(interfaces) => RadioStateMachine::with_interfaces(
                IEEE802154_RADIO_CAPABILITIES.union(RadioCapabilities::MULTI_PAN),
                interfaces.count(),
            ),
            None => RadioStateMachine::new(IEEE802154_RADIO_CAPABILITIES),
        };
        let levels = engine.tx_power_levels();
        let machine = machine.with_transmit_power(TransmitPowerCapability {
            levels: levels.portable(),
            // `i_pib_init` starts every channel at the highest level.
            reset_dbm: levels.highest_dbm(),
        });
        Self {
            engine,
            machine,
            random,
            enhanced_ack: None,
            transmission: None,
            security: RadioSecurity {
                interfaces: [InterfaceSecurity::NONE; Ieee802154MultipanIndex::COUNT as usize],
                csl: Ieee802154Csl {
                    period: 0,
                    sample_time: 0,
                },
                time_sync: None,
            },
        }
    }

    /// The MAC keys and frame counter the radio secures with, as ESP-IDF's
    /// OpenThread port keeps them (`otPlatRadioSetMacKey`,
    /// `otPlatRadioSetMacFrameCounter`). With keys, every attempt of a
    /// secured frame takes a new frame counter unless it retransmits the
    /// frame, and the current key index and key; the key identifier mode 1
    /// attempt reads the extended address as the nonce source, others reuse
    /// the last one read. A request's [`TxSecurity`] says who secures it:
    /// `Retransmission` keeps the counter and key index the frame carries
    /// (`mIsARetx`), `Processed` sends a frame the stack secured as given
    /// (`mIsSecurityProcessed`). The transmit completion reports the fields
    /// written, and a received frame reports the security of the enhanced
    /// ACK it was sent (`mAckFrameCounter`, `mAckKeyId`). Secured enhanced
    /// ACKs take their counter from the same keys. A request that carries
    /// its own key ([`TxSecurity::Key`]) is secured with it instead; without
    /// keys a secured frame goes out as given.
    ///
    /// These are the keys of [`Interface::PRIMARY`]; a multi-PAN radio keeps
    /// keys per interface ([`Self::interface_mac_keys`]).
    pub fn mac_keys(&mut self) -> &mut Option<MacKeys> {
        &mut self.security.interfaces[0].keys
    }

    /// The MAC keys and frame counter of one interface, `None` for an
    /// interface the radio does not have. As ESP-IDF's multi-instance
    /// OpenThread port keeps a security context per instance, a
    /// transmission is secured with the keys of its
    /// [`TxRequest::interface`](oer_i::TxRequest::interface) and
    /// that interface's extended address, and an enhanced ACK with the keys
    /// of the interface the acknowledged frame matched; the received frame
    /// reports that ACK's security. Otherwise the keys behave as
    /// [`Self::mac_keys`] describes.
    pub fn interface_mac_keys(&mut self, interface: Interface) -> Option<&mut Option<MacKeys>> {
        if interface.index() >= self.machine.interfaces() {
            return None;
        }
        self.security
            .interfaces
            .get_mut(usize::from(interface.index()))
            .map(|security| &mut security.keys)
    }

    /// The CSL receiver state ([`Ieee802154Csl`]).
    pub fn csl(&mut self) -> &mut Ieee802154Csl {
        &mut self.security.csl
    }

    /// The enhanced-ACK generator; `None` refuses every enhanced ACK.
    pub fn enhanced_ack(&mut self) -> &mut Option<Ieee802154EnhancedAckGenerator> {
        &mut self.enhanced_ack
    }

    /// Return the engine, for the composition's MAC teardown.
    pub fn into_engine(self) -> Ieee802154Engine<'storage> {
        self.engine
    }

    /// The number of addressing interfaces: the engine's multi-PAN
    /// interfaces, or one.
    pub const fn interfaces(&self) -> u8 {
        self.machine.interfaces()
    }

    /// The portable state.
    pub const fn state(&self) -> RadioState {
        self.machine.state()
    }

    /// The portable capabilities: [`IEEE802154_RADIO_CAPABILITIES`], with
    /// [`RadioCapabilities::MULTI_PAN`] over a multi-PAN engine.
    pub const fn capabilities(&self) -> RadioCapabilities {
        self.machine.capabilities()
    }

    /// What the radio supports, as its port publishes it: the portable
    /// capabilities and interfaces, the engine's transmit-power levels, the
    /// CCA thresholds of [`CCA_THRESHOLD_RANGE_DBM`] starting from the PIB
    /// defaults, the engine's [`RECEIVE_SENSITIVITY_DBM`] and
    /// [`IEEE802154_CSL_CAPABILITY`].
    pub const fn port_capabilities(&self) -> Ieee802154Capabilities {
        Ieee802154Capabilities {
            operations: self.capabilities(),
            interfaces: self.interfaces(),
            transmit_power: self.transmit_power(),
            cca_threshold: Some(CcaThresholdCapability {
                lowest_dbm: *CCA_THRESHOLD_RANGE_DBM.start(),
                highest_dbm: *CCA_THRESHOLD_RANGE_DBM.end(),
                reset_dbm: self.engine.pib_defaults().cca_threshold_dbm(),
            }),
            receive_sensitivity_dbm: RECEIVE_SENSITIVITY_DBM,
            csl: Some(IEEE802154_CSL_CAPABILITY),
            enhanced_ack_header_ies: IEEE802154_ENHANCED_ACK_IE_CAPACITY as u8,
        }
    }

    /// The engine, for vendor configuration the portable contract has no
    /// command for (pending table, multi-PAN identity).
    /// Operation entries must go through [`Self::submit`].
    pub fn engine(&mut self) -> &mut Ieee802154Engine<'storage> {
        &mut self.engine
    }

    /// Enable the radio: it rests asleep (`esp_i_enable` leaves the
    /// MAC idle).
    ///
    /// # Errors
    ///
    /// The radio is already enabled; nothing ran.
    pub fn enable<
        L: Ieee802154LowLevel + ?Sized,
        C: Clock + ?Sized,
        S: Ieee802154RadioSink + ?Sized,
    >(
        &mut self,
        ll: &mut L,
        clock: &C,
        sink: &mut S,
    ) -> Result<(), CommandError> {
        self.machine.enable()?;
        self.rest_asleep(ll, clock, None, sink);
        Ok(())
    }

    /// Disable the radio: an operation in flight ends first with its
    /// terminal event, as [`Self::cancel`] ends it, then the MAC sleeps and
    /// the portable state is disabled.
    ///
    /// # Errors
    ///
    /// The radio is disabled; nothing ran.
    pub fn disable<
        L: Ieee802154LowLevel + ?Sized,
        C: Clock + ?Sized,
        S: Ieee802154RadioSink + ?Sized,
    >(
        &mut self,
        ll: &mut L,
        clock: &C,
        sink: &mut S,
    ) -> Result<(), CommandError> {
        if let Some(running) = self.machine.cancellable() {
            self.cancel(ll, clock, running, sink)
                .expect("the state machine names a cancellable operation");
        }
        let previous = self.machine.disable()?;
        let flushed_on = match previous {
            RadioState::Resting(RestingState::Receiving { channel }) => Some(channel),
            _ => None,
        };
        self.rest_asleep(ll, clock, flushed_on, sink);
        Ok(())
    }

    /// Put the MAC to sleep and deliver what stopping produced.
    fn rest_asleep<
        L: Ieee802154LowLevel + ?Sized,
        C: Clock + ?Sized,
        S: Ieee802154RadioSink + ?Sized,
    >(
        &mut self,
        ll: &mut L,
        clock: &C,
        flushed_on: Option<Channel>,
        sink: &mut S,
    ) {
        let mut collector = Collector::new(clock, &mut self.enhanced_ack, &mut self.security);
        self.engine.pib().set_rx_when_idle(false);
        self.engine.sleep(ll, &mut collector);
        let notifications = collector.notifications;
        self.deliver(ll, clock, notifications, Some(flushed_on), sink);
    }

    /// Admit `command`, start its engine operation and deliver the events
    /// the start itself produced.
    ///
    /// # Errors
    ///
    /// The portable state machine rejected the command; nothing ran.
    pub fn submit<
        L: Ieee802154LowLevel + ?Sized,
        C: Clock + ?Sized,
        S: Ieee802154RadioSink + ?Sized,
    >(
        &mut self,
        ll: &mut L,
        clock: &C,
        command: RadioCommand<'_>,
        sink: &mut S,
    ) -> Result<AcceptedCommand, CommandError> {
        if let RadioCommand::Configure { configuration, .. } = command {
            self.check_pending_room(configuration)?;
        }
        let accepted = self.machine.admit(command)?;
        let mut collector = Collector::new(clock, &mut self.enhanced_ack, &mut self.security);
        let engine = &mut self.engine;
        match command {
            RadioCommand::Sleep { .. } => {
                engine.pib().set_rx_when_idle(false);
                engine.sleep(ll, &mut collector);
            }
            RadioCommand::Receive { channel, .. } => {
                engine.pib().set_channel(hal_channel(channel));
                engine.pib().set_rx_when_idle(true);
                engine.receive(ll, &mut collector);
            }
            RadioCommand::Configure { configuration, .. } => {
                if writes_registers(&command) {
                    configure_registers(engine, ll, configuration);
                } else {
                    configure_software(engine, configuration);
                }
            }
            RadioCommand::Transmit(request) => {
                let channel = hal_channel(request.channel);
                engine.pib().set_channel(channel);
                if let Some(power) = request.transmit_power_dbm {
                    engine.pib().set_power_for_channel(channel, power);
                }
                let bytes = request.frame.bytes();
                let mut image = [0; FRAME_SIZE];
                // The PHR counts the two FCS bytes the hardware appends.
                image[0] = (bytes.len() + 2) as u8;
                image[1..=bytes.len()].copy_from_slice(bytes);
                let security = match request.security {
                    TxSecurity::Key { key, address } => Some(ArmedSecurity { key, address }),
                    TxSecurity::Radio | TxSecurity::Retransmission | TxSecurity::Processed => None,
                };
                let mode = request.mode;
                let mut transmission = Transmission {
                    image,
                    mode,
                    csma: CsmaCa::new(match mode {
                        TxMode::CsmaCa { max_backoffs } => max_backoffs,
                        _ => 0,
                    }),
                    retries: FrameRetries::new(request.max_frame_retries),
                    security,
                    tx_security: request.security,
                    applied: None,
                    interface: multipan_index(request.interface)
                        .expect("the state machine admits only the engine's interfaces"),
                    phase: Phase::Attempting,
                    cancelled: false,
                };
                collector.security.time_sync = request.time_sync;
                transmission.start_access(engine, ll, &mut collector);
                self.transmission = Some(transmission);
            }
            RadioCommand::ScheduledReceive(request) => {
                // `otPlatRadioReceiveAt`: the radio sleeps outside the window.
                engine.pib().set_channel(hal_channel(request.channel));
                engine.pib().set_rx_when_idle(false);
                let start = request.start.as_micros() as u32;
                if !engine.receive_at(ll, &mut collector, start, request.duration_us) {
                    // A window that already ended does not start; the radio
                    // sleeps and the window ends at once.
                    engine.sleep(ll, &mut collector);
                    collector.push(Notification::ScheduledReceiveDone);
                }
            }
            RadioCommand::EnergyScan(request) => {
                engine.pib().set_channel(hal_channel(request.channel));
                let symbols = request.duration_us.div_ceil(16).min(u32::from(u16::MAX)) as u16;
                engine.energy_detect(ll, &mut collector, symbols);
            }
            RadioCommand::ClearChannelAssessment { channel, .. } => {
                engine.pib().set_channel(hal_channel(channel));
                engine.cca(ll, &mut collector);
            }
        }
        let flushed_on = match accepted.previous {
            RadioState::Resting(RestingState::Receiving { channel }) => Some(channel),
            _ => None,
        };
        let notifications = collector.notifications;
        self.deliver(ll, clock, notifications, Some(flushed_on), sink);
        Ok(accepted)
    }

    /// Admit a configuration that writes no MAC register
    /// ([`writes_registers`]): a change of the PIB or of the frame-pending
    /// table, which the next operation publishes, so it needs no hardware
    /// and no event follows.
    ///
    /// # Errors
    ///
    /// The portable state machine rejected the command, or the pending
    /// table has no room; nothing changed.
    ///
    /// # Panics
    ///
    /// `command` is no configuration, or one that writes a register.
    pub fn submit_software_configuration(
        &mut self,
        command: RadioCommand<'_>,
    ) -> Result<AcceptedCommand, CommandError> {
        let RadioCommand::Configure { configuration, .. } = command else {
            panic!("only a configuration applies without hardware");
        };
        assert!(
            !writes_registers(&command),
            "a register configuration needs the hardware"
        );
        self.check_pending_room(configuration)?;
        let accepted = self.machine.admit(command)?;
        configure_software(&mut self.engine, configuration);
        Ok(accepted)
    }

    /// Apply `configuration` in any radio state, disabled or during an
    /// operation, as ESP-IDF's `esp_i_set_*` setters do: a register
    /// configuration writes the MAC registers, any other the PIB or the
    /// frame-pending table. It changes no radio state, so the state machine
    /// does not admit it; an owner-shape backend, whose types state the
    /// capability and the interface, applies its configurations here.
    ///
    /// # Errors
    ///
    /// [`CommandError::PendingTableFull`]: the table has no room for an
    /// added source.
    ///
    /// # Panics
    ///
    /// The capabilities do not state the configuration, or its interface
    /// is not one of the radio's.
    pub fn configure<L: Ieee802154LowLevel + ?Sized>(
        &mut self,
        ll: &mut L,
        configuration: Configuration,
    ) -> Result<(), CommandError> {
        assert!(
            self.machine
                .capabilities()
                .supports_configuration(configuration),
            "the capabilities state the configuration"
        );
        if let Configuration::Interface { interface, .. } = configuration {
            assert!(
                interface.index() < self.machine.interfaces(),
                "the interface is one of the radio's"
            );
        }
        self.check_pending_room(configuration)?;
        if configuration_writes_registers(&configuration) {
            configure_registers(&mut self.engine, ll, configuration);
        } else {
            configure_software(&mut self.engine, configuration);
        }
        Ok(())
    }

    /// Refuse an added pending source the table has no room for.
    fn check_pending_room(&mut self, configuration: Configuration) -> Result<(), CommandError> {
        // The pending table's room is the backend's to judge, after the
        // capability and interface the state machine checks.
        if let Some((index, address)) = match configuration {
            Configuration::AddPendingAddress(address) => {
                Some((Ieee802154MultipanIndex::CONTEXT0, address))
            }
            Configuration::Interface {
                interface,
                setting: InterfaceSetting::AddPendingAddress(address),
            } if interface.index() < self.machine.interfaces() => {
                multipan_index(interface).map(|index| (index, address))
            }
            _ => None,
        } && self
            .machine
            .capabilities()
            .contains(RadioCapabilities::SOURCE_MATCH)
            && !self.engine.pending_table_for(index).has_room(address)
        {
            return Err(CommandError::PendingTableFull);
        }
        Ok(())
    }

    /// End the running operation `target` before its own end
    /// ([`RadioStateMachine::admit_cancel`]) and deliver its terminal event.
    ///
    /// # Errors
    ///
    /// No such operation runs; nothing ran.
    pub fn cancel<
        L: Ieee802154LowLevel + ?Sized,
        C: Clock + ?Sized,
        S: Ieee802154RadioSink + ?Sized,
    >(
        &mut self,
        ll: &mut L,
        clock: &C,
        target: RequestId,
        sink: &mut S,
    ) -> Result<(), CancelError> {
        self.machine.admit_cancel(target)?;
        // A cancellation flushes frames on the channel the stopped
        // operation received on.
        let flushed_on = self.backoff_receive_channel();
        // A stopped attempt ends the transmission.
        if let Some(transmission) = self.transmission.as_mut() {
            transmission.cancelled = true;
        }
        let mut collector = Collector::new(clock, &mut self.enhanced_ack, &mut self.security);
        // `i_sleep` stops whatever the MAC runs: a stopped
        // transmission reports its abort, or the outcome it had already
        // reached; a measurement and a window end silently.
        self.engine.sleep(ll, &mut collector);
        let notifications = collector.notifications;
        self.deliver(ll, clock, notifications, Some(flushed_on), sink);
        self.end_cancelled(ll, clock, target, sink);
        Ok(())
    }

    /// Report the terminal event of a cancelled operation the stopped
    /// engine did not report itself, then rest as the operation would
    /// have left the radio: receiving again on the resting channel.
    fn end_cancelled<
        L: Ieee802154LowLevel + ?Sized,
        C: Clock + ?Sized,
        S: Ieee802154RadioSink + ?Sized,
    >(
        &mut self,
        ll: &mut L,
        clock: &C,
        target: RequestId,
        sink: &mut S,
    ) {
        let event = match self.machine.state() {
            RadioState::Transmitting { id, .. } if id == target => Some(RadioEvent::TransmitDone {
                id,
                status: TxStatus::Aborted,
                acknowledgement: None,
                security: self.transmission.as_ref().and_then(|t| t.applied),
            }),
            RadioState::EnergyScanning { id, .. } if id == target => {
                Some(RadioEvent::EnergyScanFailed { id })
            }
            RadioState::AssessingChannel { id, .. } if id == target => {
                Some(RadioEvent::ClearChannelAssessmentFailed { id })
            }
            RadioState::Resting(RestingState::ScheduledReceiving { id, .. }) if id == target => {
                let event = RadioEvent::ScheduledReceiveDone { id };
                if self.machine.observe(event).is_ok() {
                    sink.event(event);
                }
                None
            }
            // The engine reported the operation's end while stopping it.
            _ => None,
        };
        if let Some(event) = event {
            // The engine sleeps, so a restore is the receive below.
            let _ = self.finish(event, sink);
        }
        if let RadioState::Resting(RestingState::Receiving { channel }) = self.machine.state() {
            let mut collector = Collector::new(clock, &mut self.enhanced_ack, &mut self.security);
            self.engine.pib().set_channel(hal_channel(channel));
            self.engine.receive(ll, &mut collector);
            let notifications = collector.notifications;
            self.deliver(ll, clock, notifications, Some(Some(channel)), sink);
        }
    }

    /// The transmit power a request or setting may name: the engine's
    /// provider levels, every channel starting at the highest.
    pub const fn transmit_power(&self) -> Option<TransmitPowerCapability> {
        self.machine.transmit_power()
    }

    /// Set the transmit power of every channel
    /// (`esp_i_set_txpower`), or of `channel` alone
    /// (`esp_i_set_power_with_channel`); the next operation
    /// publishes it.
    ///
    /// # Errors
    ///
    /// The power lies outside [`Self::transmit_power`]; nothing changed.
    pub fn set_transmit_power(
        &mut self,
        channel: Option<Channel>,
        power_dbm: i8,
    ) -> Result<(), TransmitPowerError> {
        TransmitPowerCapability::check(self.transmit_power(), power_dbm)?;
        match channel {
            Some(channel) => self
                .engine
                .pib()
                .set_power_for_channel(hal_channel(channel), power_dbm),
            None => self.engine.pib().set_power_table([power_dbm; 16]),
        }
        Ok(())
    }

    /// Set the CCA energy threshold (`esp_i_set_cca_threshold`);
    /// the next operation publishes it.
    ///
    /// # Errors
    ///
    /// The threshold lies outside [`CCA_THRESHOLD_RANGE_DBM`]; nothing
    /// changed.
    pub fn set_cca_threshold(&mut self, threshold_dbm: i8) -> Result<(), CcaThresholdError> {
        CcaThresholdCapability::check(self.port_capabilities().cca_threshold, threshold_dbm)?;
        self.engine.pib().set_cca_threshold(threshold_dbm);
        Ok(())
    }

    /// Set the CCA mode (`esp_i_set_cca_mode`); the next operation
    /// publishes it.
    pub fn set_cca_mode(&mut self, mode: CcaMode) {
        self.engine.pib().set_cca_mode(hal_cca_mode(mode));
    }

    /// The delay a transmission must wait now before its next attempt, in
    /// microseconds: a CSMA-CA backoff or the delay before a retry
    /// (`SubMac::StartTimerForBackoff`). The caller runs the timer and then
    /// calls [`Self::delay_elapsed`]. `None` when no delay is due.
    pub fn take_delay(&mut self) -> Option<u32> {
        let transmission = self.transmission.as_mut()?;
        let Phase::DelayDue(delay) = transmission.phase else {
            return None;
        };
        transmission.phase = Phase::Delaying(delay);
        let random = self.random.random();
        Some(match delay {
            Delay::CsmaBackoff => transmission.csma.backoff_micros(random).unwrap_or(0),
            Delay::Retransmission(start) => start.delay_micros(random),
        })
    }

    /// End the running delay (`SubMac::HandleTimer`): after a CSMA-CA
    /// backoff attempt with one CCA, after a retry delay acquire the channel
    /// again; deliver the events the step produced.
    pub fn delay_elapsed<
        L: Ieee802154LowLevel + ?Sized,
        C: Clock + ?Sized,
        S: Ieee802154RadioSink + ?Sized,
    >(
        &mut self,
        ll: &mut L,
        clock: &C,
        sink: &mut S,
    ) {
        let receiving = self.backoff_receive_channel();
        let Some(transmission) = self.transmission.as_mut() else {
            return;
        };
        let Phase::Delaying(delay) = transmission.phase else {
            return;
        };
        let mut collector = Collector::new(clock, &mut self.enhanced_ack, &mut self.security);
        match delay {
            Delay::CsmaBackoff => transmission.attempt(&mut self.engine, ll, &mut collector),
            Delay::Retransmission(_) => {
                transmission.start_access(&mut self.engine, ll, &mut collector);
            }
        }
        let notifications = collector.notifications;
        self.deliver(ll, clock, notifications, Some(receiving), sink);
    }

    /// The channel a backoff receives on, when the radio receives when idle.
    fn backoff_receive_channel(&mut self) -> Option<Channel> {
        let pib = self.engine.pib();
        if pib.rx_when_idle() {
            Channel::new(pib.channel().number()).ok()
        } else {
            None
        }
    }

    /// The engine interrupt handler; deliver the events it produced.
    pub fn isr<
        L: Ieee802154LowLevel + ?Sized,
        C: Clock + ?Sized,
        S: Ieee802154RadioSink + ?Sized,
    >(
        &mut self,
        ll: &mut L,
        clock: &C,
        sink: &mut S,
    ) {
        let mut collector = Collector::new(clock, &mut self.enhanced_ack, &mut self.security);
        self.engine.isr(ll, &mut collector);
        let notifications = collector.notifications;
        self.deliver(ll, clock, notifications, None, sink);
    }

    /// Translate collected notifications. Frames flushed by an engine call
    /// the role made (`flushed` is `Some`, naming the receive channel if the
    /// radio was receiving) belong to the receive that call ended, so they
    /// are delivered without admission checks.
    fn deliver<
        L: Ieee802154LowLevel + ?Sized,
        C: Clock + ?Sized,
        S: Ieee802154RadioSink + ?Sized,
    >(
        &mut self,
        ll: &mut L,
        clock: &C,
        notifications: Notifications,
        flushed: Option<Option<Channel>>,
        sink: &mut S,
    ) {
        let mut pending = notifications;
        let mut flushed = flushed;
        loop {
            let mut follow = Follow::Nothing;
            for notification in pending.into_iter().flatten() {
                match self.translate(notification, flushed, sink) {
                    Follow::Nothing => {}
                    next => follow = next,
                }
            }
            match follow {
                Follow::Nothing => return,
                Follow::Restore(channel) => {
                    // Resume receive on the resting channel an operation left.
                    let mut collector =
                        Collector::new(clock, &mut self.enhanced_ack, &mut self.security);
                    let previous = self.engine.pib().channel();
                    self.engine.pib().set_channel(channel);
                    self.engine.receive(ll, &mut collector);
                    pending = collector.notifications;
                    flushed = Channel::new(previous.number()).ok().map(Some);
                }
                Follow::Access | Follow::Delay(_) => {
                    let receiving = self.backoff_receive_channel();
                    let Some(transmission) = self.transmission.as_mut() else {
                        return;
                    };
                    let mut collector =
                        Collector::new(clock, &mut self.enhanced_ack, &mut self.security);
                    match follow {
                        Follow::Delay(delay) => {
                            transmission.start_delay(delay, &mut self.engine, ll, &mut collector);
                        }
                        _ => transmission.start_access(&mut self.engine, ll, &mut collector),
                    }
                    pending = collector.notifications;
                    flushed = Some(receiving);
                }
            }
        }
    }

    /// Deliver one notification, and say what follows it: resuming receive
    /// on the resting channel a terminal event left, or another CSMA-CA
    /// backoff.
    fn translate<S: Ieee802154RadioSink + ?Sized>(
        &mut self,
        notification: Notification,
        flushed: Option<Option<Channel>>,
        sink: &mut S,
    ) -> Follow {
        let id = match self.machine.state() {
            RadioState::Transmitting { id, .. }
            | RadioState::EnergyScanning { id, .. }
            | RadioState::AssessingChannel { id, .. } => Some(id),
            RadioState::Disabled | RadioState::Resting(_) => None,
        };
        match notification {
            Notification::Received(slot, sent) => {
                let (image, info) = self.engine.rx_frame(slot);
                if let Some(mut frame) = received_frame(&image, &info, false, flushed.flatten()) {
                    frame.metadata.sent_acknowledgement = sent;
                    let event = RadioEvent::Received(frame);
                    if flushed.is_some() || self.machine.observe(event).is_ok() {
                        sink.event(event);
                    }
                }
                let _ = self.engine.receive_handle_done(slot);
                self.end_window_after_frame(sink);
                Follow::Nothing
            }
            Notification::Transmitted(ack) => {
                let restore = match id {
                    Some(id) => {
                        let acknowledgement = ack.map(|slot| self.engine.rx_frame(slot));
                        let event = RadioEvent::TransmitDone {
                            id,
                            status: TxStatus::Success,
                            acknowledgement: acknowledgement
                                .as_ref()
                                .and_then(|(image, info)| received_frame(image, info, true, None)),
                            security: self.transmission.as_ref().and_then(|t| t.applied),
                        };
                        self.finish(event, sink)
                    }
                    None => {
                        self.fault(sink);
                        Follow::Nothing
                    }
                };
                if let Some(slot) = ack {
                    let _ = self.engine.receive_handle_done(slot);
                }
                restore
            }
            Notification::TransmitFailed(error) => {
                let Some(id) = id else {
                    self.fault(sink);
                    return Follow::Nothing;
                };
                let mut status = tx_status(error);
                if let Some(transmission) = self.transmission.as_mut()
                    && !transmission.cancelled
                    && let Some(failure) = attempt_failure(error)
                {
                    match transmission.retry(failure) {
                        Ok(follow) => return follow,
                        Err(Some(ended)) => status = ended,
                        Err(None) => {}
                    }
                }
                self.finish(
                    RadioEvent::TransmitDone {
                        id,
                        status,
                        acknowledgement: None,
                        security: self.transmission.as_ref().and_then(|t| t.applied),
                    },
                    sink,
                )
            }
            Notification::EnergyDetected(power) => {
                let Some(id) = id else {
                    self.fault(sink);
                    return Follow::Nothing;
                };
                self.finish(
                    RadioEvent::EnergyScanDone {
                        id,
                        energy_dbm: power,
                    },
                    sink,
                )
            }
            Notification::ClearChannelAssessed { busy } => {
                let Some(id) = id else {
                    self.fault(sink);
                    return Follow::Nothing;
                };
                self.finish(
                    RadioEvent::ClearChannelAssessmentDone { id, idle: !busy },
                    sink,
                )
            }
            Notification::ScheduledReceiveDone => match self.machine.state() {
                RadioState::Resting(RestingState::ScheduledReceiving { id, .. }) => {
                    let event = RadioEvent::ScheduledReceiveDone { id };
                    if self.machine.observe(event).is_ok() {
                        sink.event(event);
                    }
                    Follow::Nothing
                }
                // The end of a window an operation already replaced: the
                // engine still reports a reception it let finish, which
                // ESP-IDF's OpenThread port ignores too.
                _ => Follow::Nothing,
            },
            Notification::MeasurementFailed => match self.machine.state() {
                RadioState::EnergyScanning { id, .. } => {
                    self.finish(RadioEvent::EnergyScanFailed { id }, sink)
                }
                RadioState::AssessingChannel { id, .. } => {
                    self.finish(RadioEvent::ClearChannelAssessmentFailed { id }, sink)
                }
                _ => {
                    self.fault(sink);
                    Follow::Nothing
                }
            },
        }
    }

    /// A received frame ends a scheduled window: the vendor's receive path
    /// stops the window timer (`event_end_process`) and, without
    /// receive-on-when-idle, leaves the receiver (`next_operation`) without
    /// `esp_i_receive_at_done`. Report the window's end once the
    /// engine has left reception.
    fn end_window_after_frame<S: Ieee802154RadioSink + ?Sized>(&mut self, sink: &mut S) {
        if let RadioState::Resting(RestingState::ScheduledReceiving { id, .. }) =
            self.machine.state()
            && self.engine.state() != Ieee802154State::Rx
        {
            let event = RadioEvent::ScheduledReceiveDone { id };
            if self.machine.observe(event).is_ok() {
                sink.event(event);
            }
        }
    }

    /// Deliver one terminal event; an event the state machine rejects is a
    /// broken event sequence and disables the radio.
    fn finish<S: Ieee802154RadioSink + ?Sized>(
        &mut self,
        event: RadioEvent<'_>,
        sink: &mut S,
    ) -> Follow {
        self.transmission = None;
        self.security.time_sync = None;
        let operation_channel = self.engine.pib().channel();
        if self.machine.observe(event).is_err() {
            self.fault(sink);
            return Follow::Nothing;
        }
        sink.event(event);
        match self.machine.state() {
            RadioState::Resting(RestingState::Receiving { channel })
                if hal_channel(channel) != operation_channel =>
            {
                Follow::Restore(hal_channel(channel))
            }
            _ => Follow::Nothing,
        }
    }

    fn fault<S: Ieee802154RadioSink + ?Sized>(&mut self, sink: &mut S) {
        self.transmission = None;
        self.security.time_sync = None;
        let id: Option<RequestId> = match self.machine.state() {
            RadioState::Transmitting { id, .. }
            | RadioState::EnergyScanning { id, .. }
            | RadioState::AssessingChannel { id, .. } => Some(id),
            RadioState::Disabled | RadioState::Resting(_) => None,
        };
        let event = RadioEvent::Fault {
            id,
            fault: RadioFault::InvalidEventSequence,
        };
        if self.machine.observe(event).is_ok() {
            sink.event(event);
        }
    }
}

#[cfg(test)]
mod tests;
