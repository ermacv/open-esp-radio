//! The ESP32-C5 implementation of the chip-neutral MAC low-level interface.
//!
//! [`Ieee802154MacOwners`] implements
//! [`oer_espressif_ieee802154_engine::ll::Ieee802154LowLevel`] over the ESP32-C5 PAC:
//! each method is one accessor of the pinned public LL, and the functions
//! below convert between the engine's semantic values and the PAC's
//! register-level types. The accessor order is the engine's; nothing here
//! adds, merges or reorders register transactions.

use oer_esp32c5_pac::{
    Ieee802154AckTimeoutUnits as PacAckTimeoutUnits, Ieee802154DebugCounter as PacDebugCounter,
    Ieee802154EdSampleMode as PacEdSampleMode, Ieee802154EtmChannel as PacEtmChannel,
    Ieee802154EtmRoute as PacEtmRoute, Ieee802154Event as PacEvent,
    Ieee802154EventMask as PacEventMask, Ieee802154EventObservation as PacEventObservation,
    Ieee802154FrequencyCode as PacFrequencyCode, Ieee802154MacCommand as PacMacCommand,
    Ieee802154MultipanEnableState as PacMultipanEnableState,
    Ieee802154MultipanIndex as PacMultipanIndex, Ieee802154Pti as PacPti,
    Ieee802154RxAbortEnableSet as PacRxAbortEnableSet, Ieee802154RxAbortReason as PacRxAbortReason,
    Ieee802154RxAbortReasonObservation as PacRxAbortReasonObservation,
    Ieee802154RxStatus as PacRxStatus, Ieee802154SecurityPayloadOffset as PacSecurityPayloadOffset,
    Ieee802154Timer0ThresholdWord as PacTimer0ThresholdWord,
    Ieee802154Timer1ThresholdWord as PacTimer1ThresholdWord,
    Ieee802154TxAbortEnableSet as PacTxAbortEnableSet, Ieee802154TxAbortReason as PacTxAbortReason,
    Ieee802154TxAbortReasonObservation as PacTxAbortReasonObservation,
    Ieee802154TxPowerCode as PacTxPowerCode,
};
pub use oer_espressif_ieee802154_engine::ll::{
    Ieee802154LlCommand, Ieee802154LowLevel, Ieee802154Timer, event_end_process,
    mac_init_registers, sec_clear, set_txrx_pti, timer_fire_at,
};
use oer_espressif_ieee802154_engine::{
    channel::Ieee802154Channel,
    coex::CoexPti,
    ll::COEX_DISABLED_PTI,
    tx_power::Ieee802154ResolvedTxPower,
    types::{
        Ieee802154CcaMode, Ieee802154DebugCounter, Ieee802154EdSampleMode, Ieee802154EtmChannel,
        Ieee802154EtmRoute, Ieee802154Event, Ieee802154EventMask, Ieee802154EventObservation,
        Ieee802154MultipanEnableState, Ieee802154MultipanIndex, Ieee802154RxAbortEnableSet,
        Ieee802154RxAbortReason, Ieee802154RxAbortReasonObservation, Ieee802154RxStateCode,
        Ieee802154RxStatus, Ieee802154TxAbortEnableSet, Ieee802154TxAbortReason,
        Ieee802154TxAbortReasonObservation,
    },
};

use crate::ieee802154::mac::{Ieee802154InterruptOwner, Ieee802154TaskOwner};

/// Convert a source-level `uint16_t` ED duration into the register field.
///
/// The reviewed field is twenty-four bits wide, so every duration is
/// accepted.
const fn ed_duration_units(units: u16) -> oer_esp32c5_pac::Ieee802154EdDurationUnits {
    match oer_esp32c5_pac::Ieee802154EdDurationUnits::new(units as u32) {
        Some(duration) => duration,
        None => panic!("the reviewed ED-duration field is wider than sixteen bits"),
    }
}

/// The PAC mode of a chip-neutral CCA mode.
const fn cca_mode_into_pac(mode: Ieee802154CcaMode) -> oer_esp32c5_pac::Ieee802154CcaMode {
    use oer_esp32c5_pac::Ieee802154CcaMode as P;
    match mode {
        Ieee802154CcaMode::Carrier => P::Carrier,
        Ieee802154CcaMode::EnergyDetection => P::EnergyDetection,
        Ieee802154CcaMode::CarrierOrEnergyDetection => P::CarrierOrEnergyDetection,
        Ieee802154CcaMode::CarrierAndEnergyDetection => P::CarrierAndEnergyDetection,
    }
}

/// Every named event, in one order shared by both vocabularies.
const EVENTS: [(Ieee802154Event, PacEvent); 12] = [
    (Ieee802154Event::TxDone, PacEvent::TxDone),
    (Ieee802154Event::RxDone, PacEvent::RxDone),
    (Ieee802154Event::AckTxDone, PacEvent::AckTxDone),
    (Ieee802154Event::AckRxDone, PacEvent::AckRxDone),
    (Ieee802154Event::RxAbort, PacEvent::RxAbort),
    (Ieee802154Event::TxAbort, PacEvent::TxAbort),
    (Ieee802154Event::EdDone, PacEvent::EdDone),
    (Ieee802154Event::Timer0Overflow, PacEvent::Timer0Overflow),
    (Ieee802154Event::Timer1Overflow, PacEvent::Timer1Overflow),
    (Ieee802154Event::ClockCountMatch, PacEvent::ClockCountMatch),
    (Ieee802154Event::TxSfdDone, PacEvent::TxSfdDone),
    (Ieee802154Event::RxSfdDone, PacEvent::RxSfdDone),
];

const fn command_into_pac(command: Ieee802154LlCommand) -> PacMacCommand {
    match command {
        Ieee802154LlCommand::TxStart => PacMacCommand::Transmit,
        Ieee802154LlCommand::RxStart => PacMacCommand::Receive,
        Ieee802154LlCommand::CcaTxStart => PacMacCommand::ClearChannelThenTransmit,
        Ieee802154LlCommand::EdStart => PacMacCommand::EnergyDetection,
        Ieee802154LlCommand::Stop => PacMacCommand::Stop,
    }
}

fn event_into_pac(event: Ieee802154Event) -> PacEvent {
    EVENTS
        .iter()
        .find(|(engine, _)| *engine == event)
        .map(|(_, pac)| *pac)
        .expect("every engine event has a PAC event")
}

fn event_mask_into_pac(mask: Ieee802154EventMask) -> PacEventMask {
    EVENTS
        .iter()
        .filter(|(engine, _)| mask.contains(*engine))
        .fold(PacEventMask::NONE, |pac, (_, event)| {
            pac.union(event.mask())
        })
}

/// The engine observation of a PAC event sample; an unclassified PAC sample
/// stays unclassified.
fn event_observation_from_pac(observation: PacEventObservation) -> Ieee802154EventObservation {
    let events = EVENTS
        .iter()
        .filter(|(_, pac)| observation.contains(*pac))
        .fold(Ieee802154EventMask::NONE, |mask, (engine, _)| {
            mask.with(*engine)
        });
    Ieee802154EventObservation::new(events, observation.classification().is_err())
}

const fn rx_abort_reason_from_pac(
    observation: PacRxAbortReasonObservation,
) -> Ieee802154RxAbortReasonObservation {
    use Ieee802154RxAbortReason as R;
    use PacRxAbortReason as P;
    let reason = match observation {
        PacRxAbortReasonObservation::Unclassified => {
            return Ieee802154RxAbortReasonObservation::Unclassified;
        }
        PacRxAbortReasonObservation::Named(reason) => reason,
    };
    Ieee802154RxAbortReasonObservation::Named(match reason {
        P::RxStop => R::RxStop,
        P::SfdTimeout => R::SfdTimeout,
        P::CrcError => R::CrcError,
        P::InvalidLength => R::InvalidLength,
        P::FilterFail => R::FilterFail,
        P::NoRss => R::NoRss,
        P::CoexistenceBreak => R::CoexistenceBreak,
        P::UnexpectedAck => R::UnexpectedAck,
        P::RxRestart => R::RxRestart,
        P::TxAckTimeout => R::TxAckTimeout,
        P::TxAckStop => R::TxAckStop,
        P::TxAckCoexistenceBreak => R::TxAckCoexistenceBreak,
        P::EnhancedAckSecurityError => R::EnhancedAckSecurityError,
        P::EdAbort => R::EdAbort,
        P::EdStop => R::EdStop,
        P::EdCoexistenceReject => R::EdCoexistenceReject,
    })
}

const fn tx_abort_reason_from_pac(
    observation: PacTxAbortReasonObservation,
) -> Ieee802154TxAbortReasonObservation {
    use Ieee802154TxAbortReason as R;
    use PacTxAbortReason as P;
    let reason = match observation {
        PacTxAbortReasonObservation::Unclassified => {
            return Ieee802154TxAbortReasonObservation::Unclassified;
        }
        PacTxAbortReasonObservation::Named(reason) => reason,
    };
    Ieee802154TxAbortReasonObservation::Named(match reason {
        P::RxAckStop => R::RxAckStop,
        P::RxAckSfdTimeout => R::RxAckSfdTimeout,
        P::RxAckCrcError => R::RxAckCrcError,
        P::RxAckInvalidLength => R::RxAckInvalidLength,
        P::RxAckFilterFail => R::RxAckFilterFail,
        P::RxAckNoRss => R::RxAckNoRss,
        P::RxAckCoexistenceBreak => R::RxAckCoexistenceBreak,
        P::RxAckTypeNotAck => R::RxAckTypeNotAck,
        P::RxAckRestart => R::RxAckRestart,
        P::RxAckTimeout => R::RxAckTimeout,
        P::TxStop => R::TxStop,
        P::TxCoexistenceBreak => R::TxCoexistenceBreak,
        P::TxSecurityError => R::TxSecurityError,
        P::CcaFailed => R::CcaFailed,
        P::CcaBusy => R::CcaBusy,
    })
}

fn rx_status_from_pac(status: PacRxStatus) -> Ieee802154RxStatus {
    Ieee802154RxStatus::new(
        status.filter_fail_reason(),
        rx_abort_reason_from_pac(status.abort_reason()),
        Ieee802154RxStateCode::new(status.state().value())
            .expect("the PAC state field is three bits"),
        status.preamble_match(),
        status.sfd_match(),
    )
}

const fn debug_counter_into_pac(counter: Ieee802154DebugCounter) -> PacDebugCounter {
    use Ieee802154DebugCounter as C;
    use PacDebugCounter as P;
    match counter {
        C::SfdTimeout => P::SfdTimeout,
        C::CrcError => P::CrcError,
        C::EdAbort => P::EdAbort,
        C::CcaFail => P::CcaFail,
        C::RxFilterFail => P::RxFilterFail,
        C::NoRssDetect => P::NoRssDetect,
        C::RxAbortCoex => P::RxAbortCoex,
        C::RxRestart => P::RxRestart,
        C::TxAckAbortCoex => P::TxAckAbortCoex,
        C::EdScanBreakCoex => P::EdScanBreakCoex,
        C::RxAckAbortCoex => P::RxAckAbortCoex,
        C::RxAckTimeout => P::RxAckTimeout,
        C::TxBreakCoex => P::TxBreakCoex,
        C::TxSecurityError => P::TxSecurityError,
        C::CcaBusy => P::CcaBusy,
    }
}

const fn rx_abort_set_into_pac(set: Ieee802154RxAbortEnableSet) -> PacRxAbortEnableSet {
    match set {
        Ieee802154RxAbortEnableSet::RuntimeBaseline => PacRxAbortEnableSet::RuntimeBaseline,
        Ieee802154RxAbortEnableSet::All => PacRxAbortEnableSet::All,
    }
}

const fn tx_abort_set_into_pac(set: Ieee802154TxAbortEnableSet) -> PacTxAbortEnableSet {
    match set {
        Ieee802154TxAbortEnableSet::RuntimeBaseline => PacTxAbortEnableSet::RuntimeBaseline,
        Ieee802154TxAbortEnableSet::All => PacTxAbortEnableSet::All,
    }
}

const fn ed_sample_mode_into_pac(mode: Ieee802154EdSampleMode) -> PacEdSampleMode {
    match mode {
        Ieee802154EdSampleMode::Maximum => PacEdSampleMode::Maximum,
        Ieee802154EdSampleMode::Average => PacEdSampleMode::Average,
    }
}

fn multipan_index_into_pac(index: Ieee802154MultipanIndex) -> PacMultipanIndex {
    PacMultipanIndex::new(index.value()).expect("both vocabularies name four contexts")
}

fn multipan_state_into_pac(state: Ieee802154MultipanEnableState) -> PacMultipanEnableState {
    let [c0, c1, c2, c3] = state.enabled();
    PacMultipanEnableState::new(c0, c1, c2, c3)
}

fn multipan_state_from_pac(state: PacMultipanEnableState) -> Ieee802154MultipanEnableState {
    let enabled = |index| state.contains(PacMultipanIndex::new(index).expect("context index"));
    Ieee802154MultipanEnableState::new(enabled(0), enabled(1), enabled(2), enabled(3))
}

const fn etm_channel_into_pac(channel: Ieee802154EtmChannel) -> PacEtmChannel {
    match channel {
        Ieee802154EtmChannel::Channel0 => PacEtmChannel::Channel0,
        Ieee802154EtmChannel::Channel1 => PacEtmChannel::Channel1,
    }
}

const fn etm_route_into_pac(route: Ieee802154EtmRoute) -> PacEtmRoute {
    match route {
        Ieee802154EtmRoute::Timer0ToTxStart => PacEtmRoute::Timer0ToTxStart,
        Ieee802154EtmRoute::Timer0ToCcaTx => PacEtmRoute::Timer0ToCcaTx,
        Ieee802154EtmRoute::Timer1ToRxStart => PacEtmRoute::Timer1ToRxStart,
    }
}

/// A four-bit shared-table priority in the MAC's four-bit PTI field, written
/// unchanged as libcoexist writes it.
fn mac_pti(pti: CoexPti) -> PacPti {
    PacPti::new(pti.value()).expect("a four-bit priority fits the four-bit field")
}

/// The task owner and its active interrupt owner, held together for the
/// life of an interrupt-driven MAC epoch.
///
/// The public driver runs its operation starts, stops and interrupt handler
/// under one critical section over the complete MAC register block, so this
/// pair is the only implementation of [`Ieee802154LowLevel`] over hardware.
#[must_use = "the MAC owners must be separated and returned to their route"]
pub struct Ieee802154MacOwners {
    task: Ieee802154TaskOwner,
    interrupts: Ieee802154InterruptOwner,
}

impl Ieee802154MacOwners {
    /// Hold the task owner with the interrupt owner it activated.
    pub const fn new(task: Ieee802154TaskOwner, interrupts: Ieee802154InterruptOwner) -> Self {
        Self { task, interrupts }
    }

    /// Separate the owners, for interrupt deactivation.
    pub fn into_parts(self) -> (Ieee802154TaskOwner, Ieee802154InterruptOwner) {
        (self.task, self.interrupts)
    }

    /// Borrow the task owner, for example to prove quiescence while the MAC
    /// is stopped and its interrupt route closed.
    pub fn task_mut(&mut self) -> &mut Ieee802154TaskOwner {
        &mut self.task
    }
}

impl Ieee802154LowLevel for Ieee802154MacOwners {
    fn set_command(&mut self, command: Ieee802154LlCommand) {
        self.task
            .lease()
            .request_mac_command(command_into_pac(command));
    }

    fn events(&mut self) -> Ieee802154EventObservation {
        event_observation_from_pac(self.interrupts.registers().events())
    }

    fn clear_events(&mut self, mask: Ieee802154EventMask) {
        self.interrupts
            .registers_mut()
            .clear_events(event_mask_into_pac(mask));
    }

    fn rx_abort_reason(&mut self) -> Ieee802154RxAbortReasonObservation {
        rx_abort_reason_from_pac(self.interrupts.registers().rx_abort_reason())
    }

    fn tx_abort_reason(&mut self) -> Ieee802154TxAbortReasonObservation {
        tx_abort_reason_from_pac(self.interrupts.registers().tx_abort_reason())
    }

    fn rx_status(&mut self) -> Ieee802154RxStatus {
        rx_status_from_pac(self.task.lease().rx_status())
    }

    fn is_current_rx_frame(&mut self) -> bool {
        self.task.lease().rx_status().frame_in_progress()
    }

    fn set_tx_address(&mut self, address: u32) {
        self.task.lease().publish_transmit_dma_address(address);
    }

    fn set_rx_address(&mut self, address: u32) {
        self.task.lease().publish_receive_dma_address(address);
    }

    fn tx_auto_ack(&mut self) -> bool {
        self.task.lease().tx_auto_ack()
    }

    fn rx_auto_ack(&mut self) -> bool {
        self.task.lease().rx_auto_ack()
    }

    fn tx_enhanced_ack(&mut self) -> bool {
        self.task.lease().enhanced_ack_tx()
    }

    fn pending_mode(&mut self) -> bool {
        self.task.lease().pending_mode()
    }

    fn set_pending_bit(&mut self, pending: bool) {
        self.task.lease().set_frame_pending(pending);
    }

    fn frequency_code(&mut self) -> u8 {
        self.task.lease().frequency_code().value()
    }

    fn ed_rss(&mut self) -> i8 {
        self.interrupts.registers().ed_rss()
    }

    fn cca_busy(&mut self) -> bool {
        self.interrupts.registers().cca_busy()
    }

    fn debug_counter(&mut self, counter: Ieee802154DebugCounter) -> u16 {
        self.task
            .lease()
            .debug_counter(debug_counter_into_pac(counter))
    }

    fn clear_debug_counter(&mut self, counter: Ieee802154DebugCounter) {
        self.task
            .lease()
            .clear_debug_counter(debug_counter_into_pac(counter));
    }

    fn set_ed_duration(&mut self, symbols: u16) {
        self.task
            .lease()
            .set_ed_duration(ed_duration_units(symbols));
    }

    fn notify_enhanced_ack_generated(&mut self) {
        self.task.lease().notify_enhanced_ack_generated();
    }

    fn disable_rx_aborts(&mut self, set: Ieee802154RxAbortEnableSet) {
        self.task
            .lease()
            .disable_rx_aborts(rx_abort_set_into_pac(set));
    }

    fn set_multipan_panid(&mut self, index: Ieee802154MultipanIndex, panid: u16) {
        self.task
            .lease()
            .set_multipan_pan_id(multipan_index_into_pac(index), panid);
    }

    fn multipan_panid(&mut self, index: Ieee802154MultipanIndex) -> u16 {
        self.task
            .lease()
            .multipan_pan_id(multipan_index_into_pac(index))
    }

    fn set_multipan_short_address(&mut self, index: Ieee802154MultipanIndex, address: u16) {
        self.task
            .lease()
            .set_multipan_short_address(multipan_index_into_pac(index), address);
    }

    fn multipan_short_address(&mut self, index: Ieee802154MultipanIndex) -> u16 {
        self.task
            .lease()
            .multipan_short_address(multipan_index_into_pac(index))
    }

    fn set_multipan_extended_address(&mut self, index: Ieee802154MultipanIndex, address: [u8; 8]) {
        self.task
            .lease()
            .set_multipan_extended_address(multipan_index_into_pac(index), address);
    }

    fn multipan_extended_address(&mut self, index: Ieee802154MultipanIndex) -> [u8; 8] {
        self.task
            .lease()
            .multipan_extended_address(multipan_index_into_pac(index))
    }

    fn set_multipan_enable(&mut self, state: Ieee802154MultipanEnableState) {
        self.task
            .lease()
            .set_multipan_enable_state(multipan_state_into_pac(state));
    }

    fn multipan_enable(&mut self) -> Ieee802154MultipanEnableState {
        multipan_state_from_pac(self.task.lease().multipan_enable_state())
    }

    fn set_ack_timeout(&mut self, units: u16) {
        self.task
            .lease()
            .set_ack_timeout(PacAckTimeoutUnits::new(units));
    }

    fn ack_timeout(&mut self) -> u16 {
        self.task.lease().ack_timeout().value()
    }

    fn set_security_address(&mut self, address: &[u8; 8]) {
        self.task.lease().set_security_address(address);
    }

    fn set_security_key(&mut self, key: &[u8; 16]) {
        self.task.lease().set_security_key(key);
    }

    fn set_security_offset(&mut self, offset: u8) {
        let offset = PacSecurityPayloadOffset::new(offset & PacSecurityPayloadOffset::MAX)
            .expect("seven bits fit the payload-offset field");
        self.task.lease().set_security_payload_offset(offset);
    }

    fn enable_all_events(&mut self) {
        self.task.lease().enable_all_events();
    }

    fn enable_event(&mut self, event: Ieee802154Event) {
        self.task.lease().enable_event(event_into_pac(event));
    }

    fn disable_event(&mut self, event: Ieee802154Event) {
        self.task.lease().disable_event(event_into_pac(event));
    }

    fn enable_tx_aborts(&mut self, set: Ieee802154TxAbortEnableSet) {
        self.task
            .lease()
            .enable_tx_aborts(tx_abort_set_into_pac(set));
    }

    fn enable_rx_aborts(&mut self, set: Ieee802154RxAbortEnableSet) {
        self.task
            .lease()
            .enable_rx_aborts(rx_abort_set_into_pac(set));
    }

    fn set_ed_sample_mode(&mut self, mode: Ieee802154EdSampleMode) {
        self.task
            .lease()
            .set_ed_sample_mode(ed_sample_mode_into_pac(mode));
    }

    fn disable_coex(&mut self) {
        let pti = PacPti::new(COEX_DISABLED_PTI).expect("the disabled PTI fits four bits");
        let mut lease = self.task.lease();
        lease.set_txrx_pti(pti);
        lease.set_ack_pti(pti);
    }

    fn set_txrx_pti(&mut self, pti: CoexPti) {
        self.task.lease().set_txrx_pti(mac_pti(pti));
    }

    fn set_ack_pti(&mut self, pti: CoexPti) {
        self.task.lease().set_ack_pti(mac_pti(pti));
    }

    fn set_channel(&mut self, channel: Ieee802154Channel) {
        self.task.lease().set_frequency_code(
            PacFrequencyCode::new(channel.frequency_code())
                .expect("2.4 GHz frequency codes fit the seven-bit field"),
        );
    }

    fn set_tx_power(&mut self, power: &Ieee802154ResolvedTxPower<'_>) {
        let code = PacTxPowerCode::new(u32::from(power.field_code().value()))
            .expect("the ESP32-C5 provider has sixteen levels, which fit the five-bit field");
        self.task.lease().set_tx_power_code(code);
    }

    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.task.lease().set_cca_mode(cca_mode_into_pac(mode));
    }

    fn set_cca_threshold(&mut self, threshold_dbm: i8) {
        self.task.lease().set_cca_threshold_code(threshold_dbm);
    }

    fn set_tx_auto_ack(&mut self, enable: bool) {
        self.task.lease().set_tx_auto_ack(enable);
    }

    fn set_rx_auto_ack(&mut self, enable: bool) {
        self.task.lease().set_rx_auto_ack(enable);
    }

    fn set_tx_enhanced_ack(&mut self, enable: bool) {
        self.task.lease().set_enhanced_ack_tx(enable);
    }

    fn set_coordinator(&mut self, enable: bool) {
        self.task.lease().set_coordinator(enable);
    }

    fn set_promiscuous(&mut self, enable: bool) {
        self.task.lease().set_promiscuous(enable);
    }

    fn set_pending_mode(&mut self, enhanced: bool) {
        self.task.lease().set_pending_mode(enhanced);
    }

    fn set_transmit_security(&mut self, enable: bool) {
        self.task.lease().set_transmit_security(enable);
    }

    fn set_timer_threshold(&mut self, timer: Ieee802154Timer, microseconds: u32) {
        let mut lease = self.task.lease();
        let mut timers = lease.timer_lease();
        match timer {
            Ieee802154Timer::Timer0 => {
                timers.set_timer0_threshold(PacTimer0ThresholdWord::new(microseconds));
            }
            Ieee802154Timer::Timer1 => {
                timers.set_timer1_threshold(PacTimer1ThresholdWord::new(microseconds));
            }
        }
    }

    fn start_timer(&mut self, timer: Ieee802154Timer) {
        let mut lease = self.task.lease();
        let mut timers = lease.timer_lease();
        match timer {
            Ieee802154Timer::Timer0 => timers.start_timer0(),
            Ieee802154Timer::Timer1 => timers.start_timer1(),
        }
    }

    fn stop_timer(&mut self, timer: Ieee802154Timer) {
        let mut lease = self.task.lease();
        let mut timers = lease.timer_lease();
        match timer {
            Ieee802154Timer::Timer0 => timers.stop_timer0(),
            Ieee802154Timer::Timer1 => timers.stop_timer1(),
        }
    }

    fn etm_channel_enabled(&mut self, channel: Ieee802154EtmChannel) -> bool {
        self.task
            .lease()
            .etm_channel_enabled(etm_channel_into_pac(channel))
    }

    fn disable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        self.task
            .lease()
            .disable_etm_channel(etm_channel_into_pac(channel));
    }

    fn enable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        self.task
            .lease()
            .enable_etm_channel(etm_channel_into_pac(channel));
    }

    fn set_etm_route(&mut self, route: Ieee802154EtmRoute) {
        self.task.lease().set_etm_route(etm_route_into_pac(route));
    }
}

#[cfg(test)]
mod tests;
