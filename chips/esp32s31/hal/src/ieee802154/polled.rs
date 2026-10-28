//! Finite MAC transactions over the task/IRQ owners of a silent polling epoch.
//!
//! [`Ieee802154MacOwners`] holds the MAC
//! owners of [`mac`](crate::ieee802154::mac): each method is one accessor of
//! the pinned public LL, and each owner operation is one register
//! transaction. The accessor order is the engine's; nothing here adds,
//! merges or reorders register transactions.

use oer_esp32s31_pac::ieee802154::command::Opcode;
use oer_espressif_coex::CoexPti;
use oer_espressif_ieee802154_mac_values::{
    Ieee802154CcaMode, Ieee802154Channel, Ieee802154DebugCounter, Ieee802154EdSampleMode,
    Ieee802154EtmChannel, Ieee802154EtmRoute, Ieee802154Event, Ieee802154EventMask,
    Ieee802154EventObservation, Ieee802154MacCommand, Ieee802154MultipanEnableState,
    Ieee802154MultipanIndex, Ieee802154ResolvedTxPower, Ieee802154RxAbortEnableSet,
    Ieee802154RxAbortReasonObservation, Ieee802154RxStatus, Ieee802154Timer,
    Ieee802154TxAbortEnableSet, Ieee802154TxAbortReasonObservation,
};
use oer_espressif_interrupt_table::Silent;
use oer_mmio::Placement;

use crate::ieee802154::mac::{
    EtmPlacement, Ieee802154ArmedTaskOwner, Ieee802154InterruptOwner, Ieee802154PowerSequence,
    Ieee802154Pti, Ieee802154TaskOwner, MacPlacement, RxInfoPlacement,
};

/// The command image of an operation command.
const fn opcode(command: Ieee802154MacCommand) -> Opcode {
    match command {
        Ieee802154MacCommand::Transmit => Opcode::TxStart,
        Ieee802154MacCommand::Receive => Opcode::RxStart,
        Ieee802154MacCommand::ClearChannelThenTransmit => Opcode::CcaTxStart,
        Ieee802154MacCommand::EnergyDetection => Opcode::EdStart,
        Ieee802154MacCommand::Stop => Opcode::Stop,
    }
}

/// The command image that starts a timer.
const fn timer_start(timer: Ieee802154Timer) -> Opcode {
    match timer {
        Ieee802154Timer::Timer0 => Opcode::Timer0Start,
        Ieee802154Timer::Timer1 => Opcode::Timer1Start,
    }
}

/// The command image that stops a timer.
const fn timer_stop(timer: Ieee802154Timer) -> Opcode {
    match timer {
        Ieee802154Timer::Timer0 => Opcode::Timer0Stop,
        Ieee802154Timer::Timer1 => Opcode::Timer1Stop,
    }
}

/// The task owner and its armed interrupt owner, held together for the
/// life of a polled MAC epoch.
///
/// The engine reads and acknowledges `EVENT_STATUS` from its operation
/// starts and stops as well as from its event service, as the public driver
/// does under one critical section over the complete MAC register block, so
/// this pair keeps those transactions on one owner. The engine enables the
/// MAC's events and polls them, so the
/// pair holds the router's [`Silent`] proof that no CPU line receives the
/// MAC's source:
///
/// ```no_run
/// use oer_esp32s31_hal::ieee802154::{polled::Ieee802154MacOwners, mac::*};
/// use oer_esp32s31_hal::platform::InterruptController;
/// use oer_espressif_interrupt_table::Router;
/// fn poll(
///     router: &Router<InterruptController>,
///     task: Ieee802154TaskOwner,
///     setup: Ieee802154InterruptSetupOwner,
/// ) -> Option<Ieee802154MacOwners> {
///     // A refused route has not accessed or started the MAC.
///     let Ok(silent) = router.silent::<Ieee802154InterruptOwner>() else { return None; };
///     let (startup, interrupts) = setup.arm(task);
///     Some(Ieee802154MacOwners::new(startup, interrupts, silent))
/// }
/// # fn main() {}
/// ```
///
/// ```compile_fail,E0061
/// # use oer_esp32s31_hal::ieee802154::{polled::Ieee802154MacOwners, mac::*};
/// fn poll(mut task: Ieee802154TaskOwner, setup: Ieee802154InterruptSetupOwner) {
///     let (startup, interrupts) = setup.arm(task);
///     // Enabling the events by polling takes the silent route's proof.
///     let _mac = Ieee802154MacOwners::new(startup, interrupts);
/// }
/// ```
#[must_use = "the MAC owners must be separated and returned to their route"]
pub struct Ieee802154MacOwners<
    P: Placement = MacPlacement,
    E: Placement = EtmPlacement,
    B: Placement = RxInfoPlacement,
> {
    task: Ieee802154TaskOwner<P, E, B>,
    interrupts: Ieee802154InterruptOwner<P>,
    silent: Silent<Ieee802154InterruptOwner<P>>,
}

/// The task owner, the armed interrupt owner and the router's proof that
/// the pair polls the MAC's source, as [`Ieee802154MacOwners::into_parts`]
/// separates them.
pub type Ieee802154MacParts<P = MacPlacement, E = EtmPlacement, B = RxInfoPlacement> = (
    Ieee802154TaskOwner<P, E, B>,
    Ieee802154InterruptOwner<P>,
    Silent<Ieee802154InterruptOwner<P>>,
);

impl<P: Placement, E: Placement, B: Placement> Ieee802154MacOwners<P, E, B> {
    /// Hold the task owner with the interrupt owner it armed and the proof
    /// that no line receives the MAC's source
    /// (`oer_espressif_interrupt_table::Router::silent`).
    ///
    /// Startup discard completed before the IRQ owner became bindable.
    pub fn new(
        startup: Ieee802154ArmedTaskOwner<P, E, B>,
        interrupts: Ieee802154InterruptOwner<P>,
        silent: Silent<Ieee802154InterruptOwner<P>>,
    ) -> Self {
        let (task, interrupts, silent) = startup.into_polled(interrupts, silent);
        Self {
            task,
            interrupts,
            silent,
        }
    }

    /// Separate the owners and the proof, for the interrupt owner's
    /// deactivation.
    pub fn into_parts(self) -> Ieee802154MacParts<P, E, B> {
        (self.task, self.interrupts, self.silent)
    }

    /// Read the MAC power-sequencing words from `PAON_DELAY` to `DCDC_CTRL`,
    /// for comparison with an ESP-IDF image on hardware.
    pub fn power_sequence(&mut self) -> Ieee802154PowerSequence {
        self.task.power_sequence()
    }
}

impl<P: Placement, E: Placement, B: Placement> Ieee802154MacOwners<P, E, B> {
    /// The low byte of the shared baseband's current receive information,
    /// read live as the vendor does: it reflects the most recent baseband
    /// reception, which may be Bluetooth's while Bluetooth shares the
    /// baseband. Read side effects are unproven.
    pub fn recent_rssi(&mut self) -> i8 {
        self.task.recent_rssi()
    }
}

impl<P: Placement, E: Placement, B: Placement> Ieee802154MacOwners<P, E, B> {
    pub fn set_command(&mut self, command: Ieee802154MacCommand) {
        self.task.issue(opcode(command));
    }

    pub fn events(&mut self) -> Ieee802154EventObservation {
        self.interrupts.events().observation()
    }

    pub fn clear_events(&mut self, mask: Ieee802154EventMask) {
        let snapshot = self.interrupts.events();
        self.interrupts.clear_events(snapshot, mask);
    }

    pub fn rx_abort_reason(&mut self) -> Ieee802154RxAbortReasonObservation {
        self.interrupts.rx_abort_reason()
    }

    pub fn tx_abort_reason(&mut self) -> Ieee802154TxAbortReasonObservation {
        self.interrupts.tx_abort_reason()
    }

    pub fn rx_status(&mut self) -> Ieee802154RxStatus {
        self.task.rx_status()
    }

    pub fn is_current_rx_frame(&mut self) -> bool {
        self.task.rx_status().frame_in_progress()
    }

    pub fn set_tx_address(&mut self, address: crate::layouts::DmaWindowAddress) {
        self.task.set_tx_address(address);
    }

    pub fn set_rx_address(&mut self, address: crate::layouts::DmaWindowAddress) {
        self.task.set_rx_address(address);
    }

    pub fn tx_auto_ack(&mut self) -> bool {
        self.task.tx_auto_ack()
    }

    pub fn rx_auto_ack(&mut self) -> bool {
        self.task.rx_auto_ack()
    }

    pub fn tx_enhanced_ack(&mut self) -> bool {
        self.task.tx_enhanced_ack()
    }

    pub fn pending_mode(&mut self) -> bool {
        self.task.pending_mode()
    }

    pub fn set_pending_bit(&mut self, pending: bool) {
        self.task.set_frame_pending(pending);
    }

    pub fn frequency_code(&mut self) -> u8 {
        self.task.frequency_code()
    }

    pub fn ed_rss(&mut self) -> i8 {
        self.interrupts.ed_rss()
    }

    pub fn cca_busy(&mut self) -> bool {
        self.interrupts.cca_busy()
    }

    pub fn debug_counter(&mut self, counter: Ieee802154DebugCounter) -> u16 {
        self.task.debug_counter(counter)
    }

    pub fn clear_debug_counter(&mut self, counter: Ieee802154DebugCounter) {
        self.task.clear_debug_counter(counter);
    }

    pub fn set_ed_duration(&mut self, symbols: u16) {
        self.task.set_ed_duration(symbols);
    }

    pub fn notify_enhanced_ack_generated(&mut self) {
        self.task.notify_enhanced_ack_generated();
    }

    pub fn disable_rx_aborts(&mut self, set: Ieee802154RxAbortEnableSet) {
        self.task.set_rx_aborts(set, false);
    }

    pub fn set_multipan_panid(&mut self, index: Ieee802154MultipanIndex, panid: u16) {
        self.task.set_multipan_pan_id(index, panid);
    }

    pub fn multipan_panid(&mut self, index: Ieee802154MultipanIndex) -> u16 {
        self.task.multipan_pan_id(index)
    }

    pub fn set_multipan_short_address(&mut self, index: Ieee802154MultipanIndex, address: u16) {
        self.task.set_multipan_short_address(index, address);
    }

    pub fn multipan_short_address(&mut self, index: Ieee802154MultipanIndex) -> u16 {
        self.task.multipan_short_address(index)
    }

    pub fn set_multipan_extended_address(
        &mut self,
        index: Ieee802154MultipanIndex,
        address: [u8; 8],
    ) {
        self.task.set_multipan_extended_address(index, address);
    }

    pub fn multipan_extended_address(&mut self, index: Ieee802154MultipanIndex) -> [u8; 8] {
        self.task.multipan_extended_address(index)
    }

    pub fn set_multipan_enable(&mut self, state: Ieee802154MultipanEnableState) {
        self.task.set_multipan_enable(state);
    }

    pub fn multipan_enable(&mut self) -> Ieee802154MultipanEnableState {
        self.task.multipan_enable()
    }

    pub fn set_ack_timeout(&mut self, units: u16) {
        self.task.set_ack_timeout(units);
    }

    pub fn ack_timeout(&mut self) -> u16 {
        self.task.ack_timeout()
    }

    pub fn set_security_address(&mut self, address: &[u8; 8]) {
        self.task.set_security_address(address);
    }

    pub fn set_security_key(&mut self, key: &[u8; 16]) {
        self.task.set_security_key(key);
    }

    pub fn set_security_offset(&mut self, offset: u8) {
        self.task.set_security_offset(offset);
    }

    pub fn enable_all_events(&mut self) {
        self.task.enable_all_events();
    }

    pub fn enable_event(&mut self, event: Ieee802154Event) {
        self.task.set_event_enabled(event, true);
    }

    pub fn disable_event(&mut self, event: Ieee802154Event) {
        self.task.set_event_enabled(event, false);
    }

    pub fn enable_tx_aborts(&mut self, set: Ieee802154TxAbortEnableSet) {
        self.task.set_tx_aborts(set, true);
    }

    pub fn enable_rx_aborts(&mut self, set: Ieee802154RxAbortEnableSet) {
        self.task.set_rx_aborts(set, true);
    }

    pub fn set_ed_sample_mode(&mut self, mode: Ieee802154EdSampleMode) {
        self.task.set_ed_sample_mode(mode);
    }

    pub fn disable_coex(&mut self) {
        self.task.set_txrx_pti(Ieee802154Pti::COEX_DISABLED);
        self.task.set_ack_pti(Ieee802154Pti::COEX_DISABLED);
    }

    pub fn set_txrx_pti(&mut self, pti: CoexPti) {
        self.task.set_txrx_pti(Ieee802154Pti::from_coex(pti));
    }

    pub fn set_ack_pti(&mut self, pti: CoexPti) {
        self.task.set_ack_pti(Ieee802154Pti::from_coex(pti));
    }

    pub fn set_channel(&mut self, channel: Ieee802154Channel) {
        self.task.set_channel(channel);
    }

    pub fn set_tx_power(&mut self, power: &Ieee802154ResolvedTxPower<'_>) {
        self.task.set_tx_power_code(power.field_code().value());
    }

    pub fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.task.set_cca_mode(mode);
    }

    pub fn set_cca_threshold(&mut self, threshold_dbm: i8) {
        self.task.set_cca_threshold_code(threshold_dbm);
    }

    pub fn set_tx_auto_ack(&mut self, enable: bool) {
        self.task.set_tx_auto_ack(enable);
    }

    pub fn set_rx_auto_ack(&mut self, enable: bool) {
        self.task.set_rx_auto_ack(enable);
    }

    pub fn set_tx_enhanced_ack(&mut self, enable: bool) {
        self.task.set_tx_enhanced_ack(enable);
    }

    pub fn set_coordinator(&mut self, enable: bool) {
        self.task.set_coordinator(enable);
    }

    pub fn set_promiscuous(&mut self, enable: bool) {
        self.task.set_promiscuous(enable);
    }

    pub fn set_pending_mode(&mut self, enhanced: bool) {
        self.task.set_pending_mode(enhanced);
    }

    pub fn set_transmit_security(&mut self, enable: bool) {
        self.task.set_transmit_security(enable);
    }

    pub fn set_timer_threshold(&mut self, timer: Ieee802154Timer, microseconds: u32) {
        match timer {
            Ieee802154Timer::Timer0 => self.task.set_timer0_threshold(microseconds),
            Ieee802154Timer::Timer1 => self.task.set_timer1_threshold(microseconds),
        }
    }

    pub fn start_timer(&mut self, timer: Ieee802154Timer) {
        self.task.issue(timer_start(timer));
    }

    pub fn stop_timer(&mut self, timer: Ieee802154Timer) {
        self.task.issue(timer_stop(timer));
    }

    pub fn etm_channel_enabled(&mut self, channel: Ieee802154EtmChannel) -> bool {
        self.task.etm_channel_enabled(channel)
    }

    pub fn disable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        self.task.disable_etm_channel(channel);
    }

    pub fn enable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        self.task.enable_etm_channel(channel);
    }

    pub fn set_etm_route(&mut self, route: Ieee802154EtmRoute) {
        self.task.set_etm_route(route);
    }
}

#[cfg(test)]
mod tests;
