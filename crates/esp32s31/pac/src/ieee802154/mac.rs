//! Typed lower-level ownership for the ESP32-S31 IEEE 802.15.4 MAC.
//!
//! Every MMIO operation in this module is routed through the reviewed
//! generated `IEEE802154_MAC` peripheral. The narrow lease exposes only the
//! first field-sized operations needed by HAL; neither the generated register
//! block nor numeric addresses can escape it.

#![forbid(unsafe_code)]

use core::ops::{Deref, DerefMut};

pub use crate::generated::{
    Ieee802154EdDurationUnits, Ieee802154Timer0ThresholdWord, Ieee802154Timer0ValueWord,
    Ieee802154Timer1ThresholdWord, Ieee802154Timer1ValueWord, Ieee802154TxPowerCode,
};
use crate::{Ieee802154InterruptRegisters, Ieee802154InterruptSetup, Ieee802154TaskRegisters};
pub use oer_ieee802154_pac::{
    Ieee802154AckTimeoutUnits, Ieee802154CcaMode, Ieee802154EdCommand, Ieee802154EdSampleMode,
    Ieee802154EdSampleRate, Ieee802154Event, Ieee802154EventEnableState, Ieee802154EventMask,
    Ieee802154EventObservation, Ieee802154EventObservationError, Ieee802154MacCommand,
    Ieee802154MacControl, Ieee802154MultipanEnableState, Ieee802154MultipanIndex,
    Ieee802154ObservedEventState, Ieee802154OperationEventEnableObservation,
    Ieee802154OperationRxAbortEnableObservation, Ieee802154PanIdentity, Ieee802154RouteState,
    Ieee802154RxAbortEnableSet, Ieee802154RxAbortEnableState, Ieee802154RxAbortReason,
    Ieee802154RxAbortReasonObservation, Ieee802154RxStateCode, Ieee802154SecurityPayloadOffset,
    Ieee802154StateSnapshot, Ieee802154TransmitSecurityControl, Ieee802154TxAbortEnableSet,
    Ieee802154TxAbortReason, Ieee802154TxAbortReasonObservation, Ieee802154TxSecurityError,
    Ieee802154TxSecurityErrorObservation, Ieee802154TxStateCode, Ieee802154TxStatus,
    Ieee802154ValidationEdDurationState, Ieee802154ValidationEventEnableState,
};
use oer_ieee802154_pac::{
    Ieee802154InterruptActivationPlan, Ieee802154InterruptTransitionPort,
    execute_interrupt_activation, execute_interrupt_deactivation,
};

/// The shared observation of one chip `EVENT_ENABLE` or `EVENT_STATUS`
/// readback.
const fn event_observation(
    readback: crate::ieee802154::ownership::Ieee802154EventReadback,
) -> Ieee802154EventObservation {
    let mut events = Ieee802154EventMask::NONE;
    if readback.tx_done() {
        events = events.with(Ieee802154Event::TxDone);
    }
    if readback.rx_done() {
        events = events.with(Ieee802154Event::RxDone);
    }
    if readback.ack_tx_done() {
        events = events.with(Ieee802154Event::AckTxDone);
    }
    if readback.ack_rx_done() {
        events = events.with(Ieee802154Event::AckRxDone);
    }
    if readback.rx_abort() {
        events = events.with(Ieee802154Event::RxAbort);
    }
    if readback.tx_abort() {
        events = events.with(Ieee802154Event::TxAbort);
    }
    if readback.ed_done() {
        events = events.with(Ieee802154Event::EdDone);
    }
    if readback.timer0_overflow() {
        events = events.with(Ieee802154Event::Timer0Overflow);
    }
    if readback.timer1_overflow() {
        events = events.with(Ieee802154Event::Timer1Overflow);
    }
    if readback.clock_count_match() {
        events = events.with(Ieee802154Event::ClockCountMatch);
    }
    if readback.tx_sfd_done() {
        events = events.with(Ieee802154Event::TxSfdDone);
    }
    if readback.rx_sfd_done() {
        events = events.with(Ieee802154Event::RxSfdDone);
    }
    Ieee802154EventObservation::from_parts(events, readback.has_unclassified())
}

/// The shared observation of one affine `EVENT_STATUS` snapshot.
fn snapshot_observation(
    snapshot: &crate::svd::w1c_register_snapshot::Ieee802154EventStatusSnapshot,
) -> Ieee802154EventObservation {
    event_observation(
        crate::ieee802154::ownership::Ieee802154EventReadback::from_event_status_snapshot(snapshot),
    )
}

const fn operation_event_enable_observation(
    readback: crate::ieee802154::ownership::OperationEventEnableReadback,
) -> Ieee802154OperationEventEnableObservation {
    use crate::ieee802154::ownership::OperationEventEnableReadback as Readback;
    match readback {
        Readback::AllMasked => Ieee802154OperationEventEnableObservation::AllMasked,
        Readback::EdOperation => Ieee802154OperationEventEnableObservation::EdDoneAndRxAbortOnly,
        Readback::Unexpected => Ieee802154OperationEventEnableObservation::Unexpected,
    }
}

const fn operation_rx_abort_enable_observation(
    readback: crate::ieee802154::ownership::OperationRxAbortEnableReadback,
) -> Ieee802154OperationRxAbortEnableObservation {
    use crate::ieee802154::ownership::OperationRxAbortEnableReadback as Readback;
    match readback {
        Readback::AllMasked => Ieee802154OperationRxAbortEnableObservation::AllMasked,
        Readback::EdOperationReasons => {
            Ieee802154OperationRxAbortEnableObservation::EdOperationReasonsOnly
        }
        Readback::Unexpected => Ieee802154OperationRxAbortEnableObservation::Unexpected,
    }
}

#[cfg(feature = "validation-probes")]
const fn validation_event_enable_state(
    readback: crate::ieee802154::ownership::ValidationEventEnableReadback,
) -> Ieee802154ValidationEventEnableState {
    use crate::ieee802154::ownership::ValidationEventEnableReadback as Readback;
    match readback {
        Readback::AllMasked => Ieee802154ValidationEventEnableState::AllMasked,
        Readback::TimerPair => Ieee802154ValidationEventEnableState::TimerPairOnly,
        Readback::EdTimerAbort => Ieee802154ValidationEventEnableState::EdDoneTimer0RxAbortOnly,
        Readback::Unexpected => Ieee802154ValidationEventEnableState::Unexpected,
    }
}

/// Opaque eight-bit value accepted by the MAC frequency-code register.
///
/// This is deliberately not an IEEE channel number. The checked 2.4 GHz
/// channel mapping is source-confirmed and owned by the HAL; the PAC type
/// still represents the complete recovered field rather than silently
/// narrowing register geometry to one operating mode.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154FrequencyCode(u8);

impl Ieee802154FrequencyCode {
    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    /// Return the field value, not a complete register image.
    pub const fn value(self) -> u8 {
        self.0
    }
}

impl From<u8> for Ieee802154FrequencyCode {
    fn from(value: u8) -> Self {
        Self::new(value)
    }
}

/// One five-bit coexistence priority value.
///
/// The value is intentionally not a complete PTI register image. The PAC
/// lease places it through named generated fields and preserves all unrelated
/// bits.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154Pti(u8);

impl Ieee802154Pti {
    pub const MAX: u8 = 0x1f;

    pub const fn new(value: u8) -> Option<Self> {
        if value <= Self::MAX {
            Some(Self(value))
        } else {
            None
        }
    }

    /// Return the bounded field value, not a shifted register image.
    pub const fn value(self) -> u8 {
        self.0
    }
}

impl Ieee802154EdDurationUnits {
    const fn from_field(value: u32) -> Option<Self> {
        Self::new(value)
    }
}

/// One ordered DMA-free energy-detection/CCA register sample.
///
/// `EVENT_STATUS` remains observation-only in this copyable diagnostic value.
/// It can never be replayed as a W1C image.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154EdCcaSnapshot {
    duration: Option<Ieee802154EdDurationUnits>,
    enabled_events: Ieee802154EventObservation,
    pending_events: Ieee802154EventObservation,
    rss_code: i8,
    cca_busy: bool,
}

impl Ieee802154EdCcaSnapshot {
    #[doc(hidden)]
    pub const fn new(
        duration: Option<Ieee802154EdDurationUnits>,
        enabled_events: Ieee802154EventObservation,
        pending_events: Ieee802154EventObservation,
        rss_code: i8,
        cca_busy: bool,
    ) -> Self {
        Self {
            duration,
            enabled_events,
            pending_events,
            rss_code,
            cca_busy,
        }
    }

    /// Return the configured finite LL duration field.
    /// `None` means the observed twenty-four-bit field is outside the strict
    /// public-LL `uint16_t` subset; no truncation is performed.
    pub const fn duration(self) -> Option<Ieee802154EdDurationUnits> {
        self.duration
    }

    /// Return the complete observed `EVENT_ENABLE` field.
    pub const fn enabled_events(self) -> Ieee802154EventObservation {
        self.enabled_events
    }

    /// Return the complete non-acknowledging `EVENT_STATUS` observation.
    pub const fn pending_events(self) -> Ieee802154EventObservation {
        self.pending_events
    }

    /// Return the signed source-defined ED RSS code.
    ///
    /// Conversion to dBm remains a HAL/radio-policy responsibility.
    pub const fn rss_code(self) -> i8 {
        self.rss_code
    }

    /// Return the sampled generated `CCA_BUSY` bit.
    pub const fn cca_busy(self) -> bool {
        self.cca_busy
    }
}

/// Read-back image of the interrupt-masked IEEE 802.15.4 MAC foundation.
///
/// This snapshot deliberately excludes `EVENT_STATUS` because the foundation
/// transition neither owns nor acknowledges pending runtime events. Polled and
/// hard-IRQ paths use the generated affine W1C snapshot transaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154FoundationSnapshot {
    events_masked: bool,
    rx_aborts_masked: bool,
    tx_aborts_masked: bool,
    ed_uses_average: bool,
    txrx_pti: Ieee802154Pti,
    ack_pti: Ieee802154Pti,
    txon_delay_applied: bool,
}

/// Readback of the static, interrupt-masked MAC policy subset.
///
/// This is exactly the subset written and compared by the current runtime
/// refresh transaction. Dynamic frame-pending state and newly modeled
/// diagnostic/configuration fields are intentionally absent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154MacPolicySnapshot {
    frequency_code: Ieee802154FrequencyCode,
    cca_mode: Ieee802154CcaMode,
    cca_threshold_code: i8,
    ack_timeout: Ieee802154AckTimeoutUnits,
    control: Ieee802154MacControl,
    multipan_enable_state: Ieee802154MultipanEnableState,
    identity: Ieee802154PanIdentity,
}

/// Complete read-only observation of the currently modeled MAC configuration.
///
/// This diagnostic DTO is not a static runtime policy and is never compared
/// by command refresh. In particular, `frame_pending` is dynamic per-ACK
/// state. The transmit-power value is raw eight-bit PAC geometry with no dBm
/// or calibration-table claim.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154MacConfigurationReadback {
    frequency_code: Ieee802154FrequencyCode,
    tx_power_code: Ieee802154TxPowerCode,
    cca_mode: Ieee802154CcaMode,
    cca_threshold_code: i8,
    ed_sample_rate: Ieee802154EdSampleRate,
    ack_timeout: Ieee802154AckTimeoutUnits,
    control: Ieee802154MacControl,
    multipan_enable_state: Ieee802154MultipanEnableState,
    identities: [Ieee802154PanIdentity; 4],
    frame_pending: bool,
}

fn raw_identity_to_typed(
    readback: crate::ieee802154::ownership::MultipanIdentityReadback,
) -> Ieee802154PanIdentity {
    Ieee802154PanIdentity::new(
        readback.pan_id(),
        readback.short_address(),
        readback.extended_address(),
    )
}

impl Ieee802154MacPolicySnapshot {
    #[doc(hidden)]
    pub const fn new(
        frequency_code: Ieee802154FrequencyCode,
        cca_mode: Ieee802154CcaMode,
        cca_threshold_code: i8,
        ack_timeout: Ieee802154AckTimeoutUnits,
        control: Ieee802154MacControl,
        multipan_enable_state: Ieee802154MultipanEnableState,
        identity: Ieee802154PanIdentity,
    ) -> Self {
        Self {
            frequency_code,
            cca_mode,
            cca_threshold_code,
            ack_timeout,
            control,
            multipan_enable_state,
            identity,
        }
    }

    pub const fn frequency_code(self) -> Ieee802154FrequencyCode {
        self.frequency_code
    }

    pub const fn cca_mode(self) -> Ieee802154CcaMode {
        self.cca_mode
    }

    pub const fn cca_threshold_code(self) -> i8 {
        self.cca_threshold_code
    }

    pub const fn ack_timeout(self) -> Ieee802154AckTimeoutUnits {
        self.ack_timeout
    }

    pub const fn control(self) -> Ieee802154MacControl {
        self.control
    }

    pub const fn multipan_enable_state(self) -> Ieee802154MultipanEnableState {
        self.multipan_enable_state
    }

    pub const fn identity(self) -> Ieee802154PanIdentity {
        self.identity
    }
}

impl Ieee802154MacConfigurationReadback {
    pub const fn frequency_code(self) -> Ieee802154FrequencyCode {
        self.frequency_code
    }

    pub const fn tx_power_code(self) -> Ieee802154TxPowerCode {
        self.tx_power_code
    }

    pub const fn cca_mode(self) -> Ieee802154CcaMode {
        self.cca_mode
    }

    pub const fn cca_threshold_code(self) -> i8 {
        self.cca_threshold_code
    }

    pub const fn ed_sample_rate(self) -> Ieee802154EdSampleRate {
        self.ed_sample_rate
    }

    pub const fn ack_timeout(self) -> Ieee802154AckTimeoutUnits {
        self.ack_timeout
    }

    pub const fn control(self) -> Ieee802154MacControl {
        self.control
    }

    pub const fn multipan_enable_state(self) -> Ieee802154MultipanEnableState {
        self.multipan_enable_state
    }

    pub const fn multipan_identity(self, index: Ieee802154MultipanIndex) -> Ieee802154PanIdentity {
        self.identities[index.as_usize()]
    }

    pub const fn frame_pending(self) -> bool {
        self.frame_pending
    }
}

impl Ieee802154FoundationSnapshot {
    /// Construct a semantic platform-independent readback.
    ///
    /// Production snapshots are sampled by the PAC lease; this constructor
    /// also lets the HAL verify its transition against a host backend without
    /// duplicating the register model.
    #[doc(hidden)]
    pub const fn new(
        events_masked: bool,
        rx_aborts_masked: bool,
        tx_aborts_masked: bool,
        ed_uses_average: bool,
        txrx_pti: Ieee802154Pti,
        ack_pti: Ieee802154Pti,
        txon_delay_applied: bool,
    ) -> Self {
        Self {
            events_masked,
            rx_aborts_masked,
            tx_aborts_masked,
            ed_uses_average,
            txrx_pti,
            ack_pti,
            txon_delay_applied,
        }
    }

    /// Whether `TXON_DELAY`, `TXOFF_DELAY`, `RXON_DELAY` and
    /// `TXRX_SWITCH_DELAY` hold the vendor MAC-initialization values.
    pub const fn txon_delay_applied(self) -> bool {
        self.txon_delay_applied
    }

    pub const fn events_masked(self) -> bool {
        self.events_masked
    }

    pub const fn rx_aborts_masked(self) -> bool {
        self.rx_aborts_masked
    }

    pub const fn tx_aborts_masked(self) -> bool {
        self.tx_aborts_masked
    }

    pub const fn ed_uses_average(self) -> bool {
        self.ed_uses_average
    }

    pub const fn txrx_pti(self) -> Ieee802154Pti {
        self.txrx_pti
    }

    pub const fn ack_pti(self) -> Ieee802154Pti {
        self.ack_pti
    }
}

/// Narrow borrow reserving the unique radio-register owner for one
/// IEEE 802.15.4 transaction.
///
/// The generated peripheral remains inside the active whole-radio route. Only
/// named field operations are available through this lease, so HAL cannot
/// recover its register block, addresses, or raw images.
///
/// Interrupt status and W1C operations require the combined inactive-route
/// lease and cannot be called through this task-only capability:
///
/// ```compile_fail
/// use oer_esp32s31_pac::Ieee802154RegisterLease;
///
/// fn sample_irq_status(task: &Ieee802154RegisterLease<'_>) {
///     let _ = task.event_status_observation();
/// }
/// ```
///
/// ```compile_fail
/// use oer_esp32s31_pac::Ieee802154RegisterLease;
///
/// fn acknowledge_irq_status(task: &mut Ieee802154RegisterLease<'_>) {
///     let _ = task.acknowledge_pending_events();
/// }
/// ```
///
/// ```compile_fail
/// use oer_esp32s31_pac::Ieee802154RegisterLease;
///
/// fn literal_status_write(task: &mut Ieee802154RegisterLease<'_>) {
///     task.validation_write_event_timer0();
/// }
/// ```
#[must_use = "dropping the lease releases the unique radio-register borrow"]
#[doc(hidden)]
pub struct Ieee802154RegisterLease<'registers> {
    registers: &'registers mut crate::ieee802154::ownership::TaskRegisters,
    interrupt_route: &'registers crate::svd::Ieee802154InterruptRoute,
    etm: &'registers crate::modem::etm::Ieee802154EtmChannels,
    rx_info: &'registers crate::ieee802154::baseband::Ieee802154BasebandRxInfo,
}

/// Exclusive task-side lease for the two reviewed MAC timers.
///
/// The lease exposes complete register-specific timer words and fixed command
/// images only. It cannot sample or acknowledge interrupt status:
///
/// ```compile_fail
/// use oer_esp32s31_pac::Ieee802154TimerLease;
///
/// fn sample_irq(timer: &Ieee802154TimerLease<'_>) {
///     let _ = timer.event_status_observation();
/// }
/// ```
///
/// The parent task lease cannot be mutably borrowed twice while a timer lease
/// is alive:
///
/// ```compile_fail
/// use oer_esp32s31_pac::Ieee802154RegisterLease;
///
/// fn overlap(task: &mut Ieee802154RegisterLease<'_>) {
///     let first = task.timer_lease();
///     let second = task.timer_lease();
///     let _ = (first, second);
/// }
/// ```
#[must_use = "dropping the timer lease releases its exclusive task-register borrow"]
pub struct Ieee802154TimerLease<'registers> {
    registers: &'registers mut crate::ieee802154::ownership::TaskRegisters,
}

impl Ieee802154TimerLease<'_> {
    /// Publish one complete TIMER0 threshold without assigning clock units.
    pub fn set_timer0_threshold(&mut self, threshold: Ieee802154Timer0ThresholdWord) {
        self.registers.publish_timer0_threshold(threshold.get());
    }

    /// Observe one complete TIMER0 counter word without assigning clock units.
    pub fn timer0_value(&self) -> Ieee802154Timer0ValueWord {
        Ieee802154Timer0ValueWord::new(self.registers.observe_timer0_value())
    }

    /// Issue exactly the reviewed TIMER0-start command image.
    pub fn start_timer0(&mut self) {
        self.registers.issue_timer0_start();
    }

    /// Issue exactly the reviewed TIMER0-stop command image.
    pub fn stop_timer0(&mut self) {
        self.registers.issue_timer0_stop();
    }

    /// Publish one complete TIMER1 threshold without assigning clock units.
    pub fn set_timer1_threshold(&mut self, threshold: Ieee802154Timer1ThresholdWord) {
        self.registers.publish_timer1_threshold(threshold.get());
    }

    /// Observe one complete TIMER1 counter word without assigning clock units.
    pub fn timer1_value(&self) -> Ieee802154Timer1ValueWord {
        Ieee802154Timer1ValueWord::new(self.registers.observe_timer1_value())
    }

    /// Issue exactly the reviewed TIMER1-start command image.
    pub fn start_timer1(&mut self) {
        self.registers.issue_timer1_start();
    }

    /// Issue exactly the reviewed TIMER1-stop command image.
    pub fn stop_timer1(&mut self) {
        self.registers.issue_timer1_stop();
    }
}

impl Ieee802154RegisterLease<'_> {
    /// Exclusively borrow both reviewed MAC timers from the task owner.
    pub fn timer_lease(&mut self) -> Ieee802154TimerLease<'_> {
        Ieee802154TimerLease {
            registers: self.registers,
        }
    }

    /// Replace the source-confirmed sixteen-bit ED-duration subset.
    ///
    /// The generated masked transaction clears unused bits 23:16 exactly as
    /// the public `uint16_t` bitfield assignment does and preserves the
    /// adjacent unowned high byte.
    pub fn set_ed_duration(&mut self, duration: Ieee802154EdDurationUnits) {
        let duration = match u16::try_from(duration.get()) {
            Ok(duration) => duration,
            Err(_) => unreachable!("generated ED-duration domain is bounded to sixteen bits"),
        };
        self.registers.set_ed_duration(duration);
    }

    /// Issue one finite ED command through a generated fixed-image bridge.
    ///
    /// `Stop` remains scoped to the finite ED/CCA transaction. This method
    /// does not establish that STOP is synchronous in another MAC state.
    pub fn issue_ed_command(&mut self, command: Ieee802154EdCommand) {
        self.request_mac_command(match command {
            Ieee802154EdCommand::Start => Ieee802154MacCommand::EnergyDetection,
            Ieee802154EdCommand::Stop => Ieee802154MacCommand::Stop,
        });
    }

    /// Publish the complete TX frame-buffer address through its generated
    /// register-specific domain.
    ///
    /// Buffer provenance, DMA accessibility, alignment, and lifetime remain
    /// obligations of the higher DMA owner.
    pub fn publish_transmit_dma_address(&mut self, address: u32) {
        self.registers.publish_transmit_dma_address(address);
    }

    /// Publish the complete RX frame-buffer address through its generated
    /// register-specific domain.
    pub fn publish_receive_dma_address(&mut self, address: u32) {
        self.registers.publish_receive_dma_address(address);
    }

    /// Issue exactly one source-confirmed complete MAC command image.
    pub fn request_mac_command(&mut self, command: Ieee802154MacCommand) {
        match command {
            Ieee802154MacCommand::Transmit => self.registers.issue_transmit(),
            Ieee802154MacCommand::Receive => self.registers.issue_receive(),
            Ieee802154MacCommand::ClearChannelThenTransmit => {
                self.registers.issue_clear_channel_then_transmit();
            }
            Ieee802154MacCommand::EnergyDetection => self.registers.issue_energy_detection(),
            Ieee802154MacCommand::Stop => self.registers.issue_stop(),
        }
    }

    /// Issue exactly the source-confirmed finite `ED_START` command.
    ///
    /// This operation-specific entry point prevents a polled backend from
    /// selecting `STOP` while still reusing the same generated command leaf.
    pub fn request_ed_start(&mut self) {
        self.issue_ed_command(Ieee802154EdCommand::Start);
    }

    /// Replace only the generated eight-bit MAC frequency-code field.
    pub fn set_frequency_code(&mut self, code: Ieee802154FrequencyCode) {
        self.registers.set_frequency_code(code.value());
    }

    /// Replace only the raw eight-bit transmit-power field.
    ///
    /// This preserving update makes no dBm or calibration-table claim.
    pub fn set_tx_power_code(&mut self, code: Ieee802154TxPowerCode) {
        self.registers.set_tx_power_code(code.get());
    }

    /// Replace the CCA mode through the generated enumerated field.
    pub fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.registers.set_cca_mode(mode.field_value());
    }

    /// Replace the source-defined signed CCA threshold code.
    pub fn set_cca_threshold_code(&mut self, threshold: i8) {
        self.registers.set_cca_threshold_code(threshold);
    }

    /// Replace the source-confirmed two-bit ED sample-rate field.
    pub fn set_ed_sample_rate(&mut self, rate: Ieee802154EdSampleRate) {
        self.registers.set_ed_sample_rate(rate.field_value());
    }

    /// Replace the ACK timeout field without assigning units at the PAC layer.
    pub fn set_ack_timeout(&mut self, timeout: Ieee802154AckTimeoutUnits) {
        self.registers.set_ack_timeout(timeout.value());
    }

    /// Apply the six public PIB control fields in vendor update order.
    pub fn set_mac_control(&mut self, control: Ieee802154MacControl) {
        self.registers.set_mac_control(
            control.tx_auto_ack(),
            control.rx_auto_ack(),
            control.enhanced_ack_tx(),
            control.coordinator(),
            control.promiscuous(),
            control.enhanced_pending(),
        );
    }

    /// Replace all four named multipan-enable fields exactly.
    pub fn set_multipan_enable_state(&mut self, state: Ieee802154MultipanEnableState) {
        self.registers.set_multipan_enabled(state.enabled());
    }

    /// Program one of the four public PAN identities.
    ///
    /// Matching the public LL, each logical address setter first enables its
    /// context while preserving every other enable bit. Call
    /// [`Self::set_multipan_enable_state`] afterwards when the caller needs one
    /// exact final enable image independent of identity publication.
    pub fn set_multipan_identity(
        &mut self,
        index: Ieee802154MultipanIndex,
        identity: Ieee802154PanIdentity,
    ) {
        self.registers.set_multipan_identity(
            index.as_usize(),
            identity.pan_id(),
            identity.short_address(),
            identity.extended_address(),
        );
    }

    /// Program the public API's primary PAN identity.
    pub fn set_primary_pan_identity(&mut self, identity: Ieee802154PanIdentity) {
        self.set_multipan_identity(Ieee802154MultipanIndex::CONTEXT0, identity);
    }

    /// Read one complete PAN identity without changing its enable bit.
    pub fn multipan_identity(&self, index: Ieee802154MultipanIndex) -> Ieee802154PanIdentity {
        raw_identity_to_typed(self.registers.multipan_identity(index.as_usize()))
    }

    /// Read all four named multipan enable fields.
    pub fn multipan_enable_state(&self) -> Ieee802154MultipanEnableState {
        Ieee802154MultipanEnableState::from_enabled(self.registers.multipan_enabled())
    }

    /// Set the outgoing ACK frame-pending bit through a preserving update.
    pub fn set_frame_pending(&mut self, pending: bool) {
        self.registers.set_frame_pending(pending);
    }

    /// Read the outgoing ACK frame-pending bit.
    pub fn frame_pending(&self) -> bool {
        self.registers.frame_pending()
    }

    /// Publish exactly one enhanced-ACK-generation-done notification write.
    ///
    /// The only accepted image is the public LL's complete value one. This
    /// method does not infer whether hardware self-clears or handshakes the
    /// write; operation lifecycle must make each call non-replayable.
    pub fn notify_enhanced_ack_generated(&mut self) {
        self.registers.notify_enhanced_ack_generated();
    }

    /// Configure and enable transmit security in exact public-driver order.
    ///
    /// The borrowed key is written as four little-endian words and is never
    /// stored in a PAC value or exposed through `Debug`. The transaction is
    /// address low/high, key words zero through three, payload offset, then
    /// `TX_ENABLE = 1`.
    pub fn configure_transmit_security(
        &mut self,
        address: &[u8; 8],
        key: &[u8; 16],
        payload_offset: Ieee802154SecurityPayloadOffset,
    ) {
        self.registers
            .configure_transmit_security(address, key, payload_offset.value());
    }

    /// Disable transmit security without claiming key/address zeroization.
    ///
    /// Pinned ESP-IDF `ieee802154_sec_clear()` clears only `TX_ENABLE`; no
    /// source proves that writing zero to the write-only address/key registers
    /// is a safe hardware zeroization transaction. This method therefore
    /// preserves those registers and makes no erasure claim.
    ///
    /// No misleading clear/zeroize operation exists:
    ///
    /// ```compile_fail
    /// use oer_esp32s31_pac::Ieee802154RegisterLease;
    ///
    /// fn unsupported_zeroization(lease: &mut Ieee802154RegisterLease<'_>) {
    ///     lease.clear_transmit_security();
    /// }
    /// ```
    pub fn disable_transmit_security(&mut self) {
        self.registers.disable_transmit_security();
    }

    /// Read only the non-secret transmit-security control fields.
    pub fn transmit_security_control(&self) -> Ieee802154TransmitSecurityControl {
        let control = self.registers.transmit_security_control();
        Ieee802154TransmitSecurityControl::new(
            control.enabled(),
            Ieee802154SecurityPayloadOffset::from_field(control.payload_offset()),
        )
    }

    /// Replace only the generated five-bit TX/RX coexistence PTI field.
    pub fn set_txrx_pti(&mut self, pti: Ieee802154Pti) {
        self.registers.set_txrx_pti(pti.value());
    }

    /// Replace only the generated five-bit ACK coexistence PTI field.
    pub fn set_ack_pti(&mut self, pti: Ieee802154Pti) {
        self.registers.set_ack_pti(pti.value());
    }

    /// Sample the complete static MAC-policy subset once per backing word.
    pub fn mac_policy_snapshot(&self) -> Ieee802154MacPolicySnapshot {
        let readback = self.registers.static_mac_policy_readback();
        let identity = readback.identity();
        Ieee802154MacPolicySnapshot {
            frequency_code: Ieee802154FrequencyCode(readback.frequency_code()),
            cca_mode: Ieee802154CcaMode::from_field(readback.cca_mode()),
            cca_threshold_code: readback.cca_threshold_code() as i8,
            ack_timeout: Ieee802154AckTimeoutUnits::new(readback.ack_timeout()),
            control: Ieee802154MacControl::new(
                readback.auto_ack_tx(),
                readback.auto_ack_rx(),
                readback.enhanced_ack_tx(),
                readback.coordinator(),
                readback.promiscuous(),
                readback.pending_enhanced(),
            ),
            multipan_enable_state: Ieee802154MultipanEnableState::from_enabled(
                readback.multipan_enabled(),
            ),
            identity: Ieee802154PanIdentity::new(
                identity.pan_id(),
                identity.short_address(),
                identity.extended_address(),
            ),
        }
    }

    /// Sample every currently modeled task-side MAC configuration field.
    ///
    /// This diagnostic readback is deliberately separate from
    /// [`Self::mac_policy_snapshot`]: dynamic frame-pending state and fields
    /// not written by runtime refresh cannot silently join policy equality.
    pub fn mac_configuration_readback(&self) -> Ieee802154MacConfigurationReadback {
        let readback = self.registers.mac_configuration_readback();
        let identities = [
            raw_identity_to_typed(readback.identity(0)),
            raw_identity_to_typed(readback.identity(1)),
            raw_identity_to_typed(readback.identity(2)),
            raw_identity_to_typed(readback.identity(3)),
        ];
        Ieee802154MacConfigurationReadback {
            frequency_code: Ieee802154FrequencyCode(readback.frequency_code()),
            tx_power_code: Ieee802154TxPowerCode::new(u32::from(readback.tx_power_code()))
                .expect("raw PAC field is eight bits"),
            cca_mode: Ieee802154CcaMode::from_field(readback.cca_mode()),
            cca_threshold_code: readback.cca_threshold_code() as i8,
            ed_sample_rate: Ieee802154EdSampleRate::from_field(readback.ed_sample_rate()),
            ack_timeout: Ieee802154AckTimeoutUnits::new(readback.ack_timeout()),
            control: Ieee802154MacControl::new(
                readback.auto_ack_tx(),
                readback.auto_ack_rx(),
                readback.enhanced_ack_tx(),
                readback.coordinator(),
                readback.promiscuous(),
                readback.pending_enhanced(),
            ),
            multipan_enable_state: Ieee802154MultipanEnableState::from_enabled(
                readback.multipan_enabled(),
            ),
            identities,
            frame_pending: readback.frame_pending(),
        }
    }

    /// Order memory and device accesses at a descriptor/MMIO boundary.
    pub fn order_device_accesses(&mut self) {
        crate::device_fence();
    }
}

/// Cold/polled MAC lease that borrows both disjoint ownership halves.
///
/// Construction requires the inactive [`Ieee802154InterruptSetup`]. Because
/// activation consumes that setup, `EVENT_STATUS`, affine W1C acknowledge and
/// RX/TX interrupt sidebands are statically unavailable while a hard-IRQ owner
/// exists. Task-only command, DMA and policy operations remain reachable via
/// the embedded [`Ieee802154RegisterLease`].
#[must_use = "dropping the polled lease releases both inactive ownership borrows"]
#[doc(hidden)]
pub struct Ieee802154PolledRegisterLease<'registers> {
    task: Ieee802154RegisterLease<'registers>,
    interrupt: &'registers mut crate::ieee802154::ownership::InterruptRegisters,
}

impl<'registers> Deref for Ieee802154PolledRegisterLease<'registers> {
    type Target = Ieee802154RegisterLease<'registers>;

    fn deref(&self) -> &Self::Target {
        &self.task
    }
}

impl DerefMut for Ieee802154PolledRegisterLease<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.task
    }
}

impl Ieee802154PolledRegisterLease<'_> {
    /// Mask every MAC event while the inactive interrupt owner is borrowed.
    pub fn mask_all_events(&mut self) {
        self.task.registers.mask_all_events();
    }

    /// Mask every receive-abort source before a receive dataplane exists.
    pub fn mask_all_rx_aborts(&mut self) {
        self.task.registers.mask_all_rx_aborts();
    }

    /// Mask every transmit-abort source before a transmit dataplane exists.
    pub fn mask_all_tx_aborts(&mut self) {
        self.task.registers.mask_all_tx_aborts();
    }

    /// Select the vendor foundation's average energy-detection sampler.
    pub fn select_average_ed_sampling(&mut self) {
        self.task.registers.select_average_ed_sampling();
    }

    /// Apply the vendor MAC-initialization delays of
    /// `ieee802154_txon_delay_set`.
    pub fn apply_txon_delay(&mut self) {
        self.task.registers.set_txon_delay();
    }

    /// Select one closed finite-operation `EVENT_ENABLE` state.
    pub fn set_event_enable(&mut self, state: Ieee802154EventEnableState) {
        match state {
            Ieee802154EventEnableState::AllMasked => self.task.registers.mask_all_events(),
            Ieee802154EventEnableState::EdOperation => {
                self.task.registers.enable_ed_operation_events()
            }
        }
    }

    /// Select one closed finite-operation `RX_ABORT_ENABLE` state.
    pub fn set_rx_abort_enable(&mut self, state: Ieee802154RxAbortEnableState) {
        match state {
            Ieee802154RxAbortEnableState::AllMasked => self.task.registers.mask_all_rx_aborts(),
            Ieee802154RxAbortEnableState::EdOperationReasons => {
                self.task.registers.enable_ed_operation_rx_abort_reasons()
            }
        }
    }

    /// Classify the complete event-delivery field for a finite polled ED/CCA
    /// operation without sampling `EVENT_STATUS`.
    pub fn operation_event_enable_observation(&self) -> Ieee802154OperationEventEnableObservation {
        operation_event_enable_observation(self.task.registers.operation_event_enable_readback())
    }

    /// Classify the complete RX-abort delivery field for a finite polled
    /// ED/CCA operation.
    pub fn operation_rx_abort_enable_observation(
        &self,
    ) -> Ieee802154OperationRxAbortEnableObservation {
        operation_rx_abort_enable_observation(
            self.task.registers.operation_rx_abort_enable_readback(),
        )
    }

    /// Observe the complete fourteen-bit event field without acknowledging it.
    pub fn event_status_observation(&self) -> Ieee802154EventObservation {
        let snapshot = self.interrupt.sample_event_status();
        snapshot_observation(&snapshot)
    }

    /// Classify the IRQ-owned RX-abort reason field without exporting the
    /// containing register image.
    pub fn rx_abort_reason_observation(&self) -> Ieee802154RxAbortReasonObservation {
        Ieee802154RxAbortReasonObservation::from_field(
            self.interrupt.rx_status_readback().abort_reason_code(),
        )
    }

    /// Sample only fields written by the interrupt-masked foundation.
    pub fn foundation_snapshot(&self) -> Ieee802154FoundationSnapshot {
        let readback = self.task.registers.foundation_readback();
        Ieee802154FoundationSnapshot {
            events_masked: readback.events_masked(),
            rx_aborts_masked: readback.rx_aborts_masked(),
            tx_aborts_masked: readback.tx_aborts_masked(),
            ed_uses_average: readback.ed_uses_average(),
            txrx_pti: Ieee802154Pti(readback.txrx_pti()),
            ack_pti: Ieee802154Pti(readback.ack_pti()),
            txon_delay_applied: readback.txon_delay_applied(),
        }
    }

    /// Sample the generated receive and transmit state fields once each.
    pub fn state_snapshot(&self) -> Ieee802154StateSnapshot {
        let (rx, tx) = self.interrupt.state_codes();
        Ieee802154StateSnapshot::new(
            Ieee802154RxStateCode::from_field(rx),
            Ieee802154TxStateCode::from_field(tx),
        )
    }

    /// Sample the DMA-free ED/CCA surface while the interrupt half is inactive.
    pub fn ed_cca_snapshot(&self) -> Ieee802154EdCcaSnapshot {
        let event_status = self.interrupt.sample_event_status();
        Ieee802154EdCcaSnapshot::new(
            Ieee802154EdDurationUnits::from_field(self.task.registers.ed_duration()),
            event_observation(self.task.registers.event_enable_readback()),
            snapshot_observation(&event_status),
            self.interrupt.ed_rss_code(),
            self.interrupt.cca_busy(),
        )
    }

    /// Sample only the signed ED RSS sideband after `ED_DONE`.
    pub fn ed_rss_code(&self) -> i8 {
        self.interrupt.ed_rss_code()
    }

    /// Sample only the CCA-busy sideband after `ED_DONE`.
    pub fn cca_busy(&self) -> bool {
        self.interrupt.cca_busy()
    }

    /// Sample and consume one complete affine W1C event snapshot.
    #[doc(hidden)]
    pub fn acknowledge_pending_events(&mut self) -> Ieee802154EventObservation {
        crate::device_fence();
        let snapshot = self.interrupt.sample_event_status();
        let events = snapshot_observation(&snapshot);
        self.interrupt.acknowledge_event_status(snapshot);
        crate::device_fence();
        events
    }

    /// Classify both source-132 routes without exposing register images.
    #[doc(hidden)]
    pub fn interrupt_route_state(&self) -> Ieee802154RouteState {
        let (core0_map, core0_bits_6_7, core0_pass_level, core0_bits_10_31) =
            crate::svd::field_snapshot_read::observe_ieee802154_core0_route(
                self.task.interrupt_route,
            );
        let (core1_map, core1_bits_6_7, core1_pass_level, core1_bits_10_31) =
            crate::svd::field_snapshot_read::observe_ieee802154_core1_route(
                self.task.interrupt_route,
            );
        Ieee802154RouteState::from_observation(
            core0_map == 0
                && core0_bits_6_7 == 0
                && core0_pass_level == 0
                && core0_bits_10_31 == 0
                && core1_map == 0
                && core1_bits_6_7 == 0
                && core1_pass_level == 0
                && core1_bits_10_31 == 0,
            core0_map != 0 || core1_map != 0,
            core0_pass_level != 0 || core1_pass_level != 0,
        )
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_event_enable_state(&self) -> Ieee802154ValidationEventEnableState {
        validation_event_enable_state(self.task.registers.validation_event_enable_readback())
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_enable_timer_events(&mut self) {
        self.task.registers.validation_enable_timer_events();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_disable_all_events(&mut self) {
        self.task.registers.validation_disable_all_events();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_event_status_state(&self) -> Ieee802154ObservedEventState {
        event_observation(self.interrupt.validation_event_status_events()).state()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_event_timer0_value(&self) -> u32 {
        self.task.registers.validation_timer0_value()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_event_timer1_value(&self) -> u32 {
        self.task.registers.validation_timer1_value()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_set_event_timer_thresholds(&mut self, threshold: u32) {
        self.task
            .registers
            .validation_set_timer_thresholds(threshold);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_start_event_timer0(&mut self) {
        self.task.registers.validation_start_timer0();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_stop_event_timer0(&mut self) {
        self.task.registers.validation_stop_timer0();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_start_event_timer1(&mut self) {
        self.task.registers.validation_start_timer1();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_stop_event_timer1(&mut self) {
        self.task.registers.validation_stop_timer1();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_write_event_timer0(&mut self) {
        self.interrupt.validation_write_timer0_event();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_write_event_timer1(&mut self) {
        self.interrupt.validation_write_timer1_event();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_event_enable_state(&self) -> Ieee802154ValidationEventEnableState {
        validation_event_enable_state(self.task.registers.validation_event_enable_readback())
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_enable_ed_timer_abort_events(&mut self) {
        self.task
            .registers
            .validation_enable_ed_timer_abort_events();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_disable_ed_events(&mut self) {
        self.task.registers.validation_disable_ed_events();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_rx_abort_enable_state(
        &self,
    ) -> Ieee802154OperationRxAbortEnableObservation {
        operation_rx_abort_enable_observation(
            self.task.registers.operation_rx_abort_enable_readback(),
        )
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_enable_ed_abort_reasons(&mut self) {
        self.task.registers.validation_enable_ed_abort_reasons();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_disable_ed_abort_reasons(&mut self) {
        self.task.registers.validation_disable_ed_abort_reasons();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_event_status_state(&self) -> Ieee802154ObservedEventState {
        event_observation(self.interrupt.validation_ed_event_status_events()).state()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_rx_abort_reason(&self) -> Ieee802154RxAbortReasonObservation {
        self.rx_abort_reason_observation()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_duration_state(&self) -> Ieee802154ValidationEdDurationState {
        Ieee802154ValidationEdDurationState::from_field(
            self.task.registers.validation_ed_duration(),
        )
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_set_ed_duration_eight(&mut self) {
        self.task.registers.validation_set_ed_duration_eight();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_ed_timer0_value(&self) -> u32 {
        self.task.registers.validation_ed_timer0_value()
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_set_ed_timer0_threshold(&mut self, threshold: u32) {
        self.task
            .registers
            .validation_set_ed_timer0_threshold(threshold);
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_start_ed_timer0(&mut self) {
        self.task.registers.validation_start_ed_timer0();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_stop_ed_timer0(&mut self) {
        self.task.registers.validation_stop_ed_timer0();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_start_ed(&mut self) {
        self.task.registers.validation_start_ed();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_stop_ed_operation(&mut self) {
        self.task.registers.validation_stop_ed_operation();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_write_ed_done_event(&mut self) {
        self.interrupt.validation_write_ed_done_event();
    }

    #[cfg(feature = "validation-probes")]
    #[doc(hidden)]
    pub fn validation_write_ed_timer0_event(&mut self) {
        self.interrupt.validation_write_ed_timer0_event();
    }
}

/// Borrowed task owner plus the disjoint raw interrupt partition for one
/// activation or teardown transaction.
struct Ieee802154PacInterruptTransitionPort<'task> {
    task: &'task mut Ieee802154TaskRegisters,
    registers: crate::ieee802154::ownership::InterruptRegisters,
}

impl Ieee802154InterruptTransitionPort for Ieee802154PacInterruptTransitionPort<'_> {
    type EventSnapshot = crate::svd::w1c_register_snapshot::Ieee802154EventStatusSnapshot;

    fn stop_operation(&mut self) {
        self.task.peripherals.ieee802154_mac.issue_stop();
    }

    fn stop_timer0(&mut self) {
        self.task.peripherals.ieee802154_mac.issue_timer0_stop();
    }

    fn stop_timer1(&mut self) {
        self.task.peripherals.ieee802154_mac.issue_timer1_stop();
    }

    fn mask_all_events(&mut self) {
        self.task.peripherals.ieee802154_mac.mask_all_events();
    }

    fn enable_runtime_events(&mut self) {
        self.task
            .peripherals
            .ieee802154_mac
            .enable_runtime_events_without_timer0();
    }

    fn mask_all_tx_aborts(&mut self) {
        self.task.peripherals.ieee802154_mac.mask_all_tx_aborts();
    }

    fn enable_runtime_tx_aborts(&mut self) {
        self.task
            .peripherals
            .ieee802154_mac
            .enable_runtime_tx_aborts();
    }

    fn mask_all_rx_aborts(&mut self) {
        self.task.peripherals.ieee802154_mac.mask_all_rx_aborts();
    }

    fn enable_runtime_rx_aborts(&mut self) {
        self.task
            .peripherals
            .ieee802154_mac
            .enable_runtime_rx_aborts();
    }

    fn order_device_accesses(&mut self) {
        crate::device_fence();
    }

    fn sample_events(&mut self) -> Self::EventSnapshot {
        self.registers.sample_event_status()
    }

    fn acknowledge_events(&mut self, snapshot: Self::EventSnapshot) {
        self.registers.acknowledge_event_status(snapshot);
    }
}

impl Ieee802154InterruptSetup {
    /// Borrow both disjoint halves for one inactive-route polled transaction.
    ///
    /// The returned lease cannot outlive either owner. Calling
    /// [`Self::activate`] consumes this setup, so no polled `EVENT_STATUS` or
    /// W1C operation can coexist with the active hard-IRQ capability.
    #[doc(hidden)]
    pub fn polled_register_lease<'registers>(
        &'registers mut self,
        task: &'registers mut Ieee802154TaskRegisters,
    ) -> Ieee802154PolledRegisterLease<'registers> {
        Ieee802154PolledRegisterLease {
            task: task.ieee802154_register_lease(),
            interrupt: &mut self.registers,
        }
    }

    /// Install the source-confirmed runtime baseline and create the finite
    /// hard-IRQ owner.
    ///
    /// The platform CPU route must remain disabled until the returned value is
    /// installed in its final storage. This single consuming transition uses
    /// only the generated runtime-baseline accessors. It keeps event delivery
    /// masked while configuring both abort fields, consumes one complete stale
    /// affine W1C snapshot, publishes runtime events last, and orders those
    /// writes before returning.
    ///
    /// There is no caller-selected mask argument: runtime code cannot activate
    /// an incomplete abort vocabulary or an event absent from the reviewed ISR.
    pub fn activate(self, task: &mut Ieee802154TaskRegisters) -> Ieee802154InterruptRegisters {
        let mut port = Ieee802154PacInterruptTransitionPort {
            task,
            registers: self.registers,
        };
        execute_interrupt_activation(
            &mut port,
            Ieee802154InterruptActivationPlan::SOURCE_CONFIRMED_BASELINE,
        );
        Ieee802154InterruptRegisters {
            registers: port.registers,
        }
    }
}

impl Ieee802154InterruptRegisters {
    /// `ieee802154_ll_get_events`: one read of the event field, without
    /// acknowledging it.
    pub fn events(&self) -> Ieee802154EventObservation {
        event_observation(self.registers.event_readback())
    }

    /// `ieee802154_ll_clear_events`: clear the asserted events of `mask` and
    /// leave every other event latched.
    pub fn clear_events(&mut self, mask: Ieee802154EventMask) {
        self.registers
            .clear_events(|event| mask.contains(event_of(event)));
    }

    /// `ieee802154_ll_get_rx_abort_reason`.
    pub fn rx_abort_reason(&self) -> Ieee802154RxAbortReasonObservation {
        Ieee802154RxAbortReasonObservation::from_field(
            self.registers.rx_status_readback().abort_reason_code(),
        )
    }

    /// `ieee802154_ll_get_ed_rss`.
    pub fn ed_rss(&self) -> i8 {
        self.registers.ed_rss_code()
    }

    /// `ieee802154_ll_is_cca_busy`.
    pub fn cca_busy(&self) -> bool {
        self.registers.cca_busy()
    }

    /// `ieee802154_ll_get_tx_abort_reason`.
    pub fn tx_abort_reason(&self) -> Ieee802154TxAbortReasonObservation {
        Ieee802154TxAbortReasonObservation::from_field(
            self.registers.tx_status_readback().abort_reason_code(),
        )
    }

    /// Close one finite hard-IRQ epoch and return inactive setup ownership.
    ///
    /// The caller must disable the platform CPU route before this method. The
    /// transition stops the active operation and both MAC timers, replaces
    /// event, transmit-abort, and receive-abort enables with exact zero images,
    /// consumes one final complete affine W1C snapshot, and orders both phases
    /// before returning task-side setup authority.
    pub fn deactivate(self, task: &mut Ieee802154TaskRegisters) -> Ieee802154InterruptSetup {
        let mut port = Ieee802154PacInterruptTransitionPort {
            task,
            registers: self.registers,
        };
        execute_interrupt_deactivation(&mut port);
        Ieee802154InterruptSetup {
            registers: port.registers,
        }
    }
}

impl Ieee802154TaskRegisters {
    /// Borrow the MAC capability from the dedicated IEEE 802.15.4 route.
    ///
    /// No Wi-Fi or Bluetooth-controller operation is reachable through the
    /// returned narrow lease.
    #[doc(hidden)]
    pub fn ieee802154_register_lease(&mut self) -> Ieee802154RegisterLease<'_> {
        Ieee802154RegisterLease {
            registers: &mut self.peripherals.ieee802154_mac,
            interrupt_route: &self.peripherals.ieee802154_interrupt_route,
            etm: &self.peripherals.etm,
            rx_info: &self.peripherals.rx_info,
        }
    }
}

const fn event_of(event: crate::ieee802154::ownership::RawEvent) -> Ieee802154Event {
    use crate::ieee802154::ownership::RawEvent;
    match event {
        RawEvent::TxDone => Ieee802154Event::TxDone,
        RawEvent::RxDone => Ieee802154Event::RxDone,
        RawEvent::AckTxDone => Ieee802154Event::AckTxDone,
        RawEvent::AckRxDone => Ieee802154Event::AckRxDone,
        RawEvent::RxAbort => Ieee802154Event::RxAbort,
        RawEvent::TxAbort => Ieee802154Event::TxAbort,
        RawEvent::EdDone => Ieee802154Event::EdDone,
        RawEvent::Timer0Overflow => Ieee802154Event::Timer0Overflow,
        RawEvent::Timer1Overflow => Ieee802154Event::Timer1Overflow,
        RawEvent::ClockCountMatch => Ieee802154Event::ClockCountMatch,
        RawEvent::TxSfdDone => Ieee802154Event::TxSfdDone,
        RawEvent::RxSfdDone => Ieee802154Event::RxSfdDone,
    }
}

mod etm;
mod single_field;

impl Ieee802154RegisterLease<'_> {
    /// The signed RSSI in dBm of the most recent baseband reception, as
    /// `esp_ieee802154_get_recent_rssi` reads it: live, of whichever protocol
    /// last received on the shared baseband. Read side effects are unproven.
    pub fn recent_rssi(&self) -> i8 {
        self.rx_info.recent_rssi()
    }
}

/// The MAC power-sequencing words from `PAON_DELAY` to `DCDC_CTRL`: the
/// power-amplifier-on, transmit-on, transmit-enable stop, transmit-off,
/// receive-on, transmit/receive switch and continuous-receive delays and the
/// MAC's control of the DC-DC converter around transmission, with the
/// vendor-reserved remainder of each word.
///
/// The public driver writes only the transmit-on, transmit-off, receive-on
/// and switch delays (`ieee802154_txon_delay_set`); no pinned vendor code
/// writes the others. Values are field values; the PAC assigns them no
/// physical units.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154PowerSequence {
    pa_on_delay: u16,
    tx_on_delay: u16,
    tx_enable_stop_delay: u8,
    tx_off_delay: u8,
    rx_on_delay: u16,
    txrx_switch_delay: u16,
    continuous_rx_delay: u8,
    dcdc_pre_raise_delay: u8,
    dcdc_drop_delay: u8,
    dcdc_control_enabled: bool,
    dcdc_raise_for_tx: bool,
    vendor_reserved: [u32; 8],
}

impl Ieee802154PowerSequence {
    const fn from_raw(raw: crate::ieee802154::ownership::RawPowerSequence) -> Self {
        Self {
            pa_on_delay: raw.paon_delay,
            tx_on_delay: raw.txon_delay,
            tx_enable_stop_delay: raw.txen_stop_delay,
            tx_off_delay: raw.txoff_delay,
            rx_on_delay: raw.rxon_delay,
            txrx_switch_delay: raw.txrx_switch_delay,
            continuous_rx_delay: raw.cont_rx_delay,
            dcdc_pre_raise_delay: raw.dcdc_pre_up_delay,
            dcdc_drop_delay: raw.dcdc_down_delay,
            dcdc_control_enabled: raw.dcdc_ctrl_enabled,
            dcdc_raise_for_tx: raw.tx_dcdc_up,
            vendor_reserved: raw.reserved,
        }
    }

    /// `PAON_DELAY`: ten-bit power-amplifier-on delay.
    pub const fn pa_on_delay(self) -> u16 {
        self.pa_on_delay
    }

    /// `TXON_DELAY`: ten-bit transmit-on delay.
    pub const fn tx_on_delay(self) -> u16 {
        self.tx_on_delay
    }

    /// `TXEN_STOP_DELAY`: six-bit transmit-enable stop delay.
    pub const fn tx_enable_stop_delay(self) -> u8 {
        self.tx_enable_stop_delay
    }

    /// `TXOFF_DELAY`: six-bit transmit-off delay.
    pub const fn tx_off_delay(self) -> u8 {
        self.tx_off_delay
    }

    /// `RXON_DELAY`: eleven-bit receive-on delay.
    pub const fn rx_on_delay(self) -> u16 {
        self.rx_on_delay
    }

    /// `TXRX_SWITCH_DELAY`: ten-bit transmit/receive switch delay.
    pub const fn txrx_switch_delay(self) -> u16 {
        self.txrx_switch_delay
    }

    /// `CONT_RX_DELAY`: six-bit continuous-receive delay.
    pub const fn continuous_rx_delay(self) -> u8 {
        self.continuous_rx_delay
    }

    /// `DCDC_CTRL.DCDC_PRE_UP_DELAY`: delay before the DC-DC raise.
    pub const fn dcdc_pre_raise_delay(self) -> u8 {
        self.dcdc_pre_raise_delay
    }

    /// `DCDC_CTRL.DCDC_DOWN_DELAY`: delay before the DC-DC drop.
    pub const fn dcdc_drop_delay(self) -> u8 {
        self.dcdc_drop_delay
    }

    /// `DCDC_CTRL.DCDC_CTRL_EN`: the MAC controls the DC-DC converter.
    pub const fn dcdc_control_enabled(self) -> bool {
        self.dcdc_control_enabled
    }

    /// `DCDC_CTRL.TX_DCDC_UP`: raise the DC-DC converter for transmission.
    pub const fn dcdc_raise_for_tx(self) -> bool {
        self.dcdc_raise_for_tx
    }

    /// The vendor-reserved remainder of each word, from `PAON_DELAY` to
    /// `DCDC_CTRL` in address order, as the opaque fields read them: bits
    /// 10..31, 10..31, 6..31, 6..31, 11..31, 10..31, 6..31 and 17..30.
    pub const fn vendor_reserved_bits(self) -> [u32; 8] {
        self.vendor_reserved
    }
}

/// The transmit and receive DMA error codes (`TXDMA_ERR`, `RXDMA_ERR`); no
/// code is assigned a meaning.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Ieee802154DmaErrorCodes {
    transmit: u8,
    receive: u8,
}

impl Ieee802154DmaErrorCodes {
    /// The four-bit transmit DMA error code.
    pub const fn transmit(self) -> u8 {
        self.transmit
    }

    /// The four-bit receive DMA error code.
    pub const fn receive(self) -> u8 {
        self.receive
    }
}

impl Ieee802154RegisterLease<'_> {
    /// Read the MAC power-sequencing fields, for comparison with the vendor
    /// image on hardware.
    pub fn power_sequence(&self) -> Ieee802154PowerSequence {
        Ieee802154PowerSequence::from_raw(self.registers.power_sequence())
    }

    /// Read the transmit and receive DMA error codes.
    pub fn dma_error_codes(&self) -> Ieee802154DmaErrorCodes {
        let (transmit, receive) = self.registers.dma_error_codes();
        Ieee802154DmaErrorCodes { transmit, receive }
    }
}

pub use etm::{Ieee802154EtmChannel, Ieee802154EtmRoute};

pub use single_field::{Ieee802154DebugCounter, Ieee802154RxStatus};

#[cfg(test)]
mod tests;
