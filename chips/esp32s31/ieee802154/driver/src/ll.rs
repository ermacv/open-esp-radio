//! The engine's LL contract implemented over HAL-owned finite transactions.

use oer_esp32s31_hal::{
    ieee802154::{
        mac::{EtmPlacement, Ieee802154PowerSequence, MacPlacement, RxInfoPlacement},
        polled::Ieee802154MacOwners,
    },
    layouts::DmaWindowAddress,
};
use oer_espressif_coex::CoexPti;
use oer_espressif_ieee802154_engine::ll::{Ieee802154LowLevel, Ieee802154RecentRssi};
use oer_espressif_ieee802154_mac_values::{
    Ieee802154CcaMode, Ieee802154Channel, Ieee802154DebugCounter, Ieee802154EdSampleMode,
    Ieee802154EtmChannel, Ieee802154EtmRoute, Ieee802154Event, Ieee802154EventMask,
    Ieee802154EventObservation, Ieee802154MacCommand, Ieee802154MultipanEnableState,
    Ieee802154MultipanIndex, Ieee802154ResolvedTxPower, Ieee802154RxAbortEnableSet,
    Ieee802154RxAbortReasonObservation, Ieee802154RxStatus, Ieee802154Timer,
    Ieee802154TxAbortEnableSet, Ieee802154TxAbortReasonObservation,
};
use oer_mmio::Placement;

/// Own the HAL's task/IRQ/Silent pair for one engine epoch.
#[must_use = "retain the MAC adapter until the engine returns its hardware owners"]
pub struct S31Mac<
    P: Placement = MacPlacement,
    E: Placement = EtmPlacement,
    B: Placement = RxInfoPlacement,
> {
    inner: Ieee802154MacOwners<P, E, B>,
}

impl<P: Placement, E: Placement, B: Placement> S31Mac<P, E, B> {
    /// Move the silent polling pair into the engine's LL adapter.
    pub fn new(inner: Ieee802154MacOwners<P, E, B>) -> Self {
        Self { inner }
    }

    /// Return the same polling pair after the engine relinquishes it.
    pub fn into_inner(self) -> Ieee802154MacOwners<P, E, B> {
        self.inner
    }

    /// Read the owned MAC's power-sequencing words for hardware comparison.
    pub fn power_sequence(&mut self) -> Ieee802154PowerSequence {
        self.inner.power_sequence()
    }
}

impl<P: Placement, E: Placement, B: Placement> Ieee802154RecentRssi for S31Mac<P, E, B> {
    /// The low byte of the shared baseband's current receive information,
    /// read live as the vendor does: it reflects the most recent baseband
    /// reception, which may be Bluetooth's while Bluetooth shares the
    /// baseband. Read side effects are unproven.
    fn recent_rssi(&mut self) -> i8 {
        self.inner.recent_rssi()
    }
}

impl<P: Placement, E: Placement, B: Placement> Ieee802154LowLevel for S31Mac<P, E, B> {
    type Address = DmaWindowAddress;

    fn set_command(&mut self, command: Ieee802154MacCommand) {
        self.inner.set_command(command)
    }

    fn events(&mut self) -> Ieee802154EventObservation {
        self.inner.events()
    }

    fn clear_events(&mut self, mask: Ieee802154EventMask) {
        self.inner.clear_events(mask)
    }

    fn rx_abort_reason(&mut self) -> Ieee802154RxAbortReasonObservation {
        self.inner.rx_abort_reason()
    }

    fn tx_abort_reason(&mut self) -> Ieee802154TxAbortReasonObservation {
        self.inner.tx_abort_reason()
    }

    fn rx_status(&mut self) -> Ieee802154RxStatus {
        self.inner.rx_status()
    }

    fn is_current_rx_frame(&mut self) -> bool {
        self.inner.is_current_rx_frame()
    }

    fn set_tx_address(&mut self, address: Self::Address) {
        self.inner.set_tx_address(address)
    }

    fn set_rx_address(&mut self, address: Self::Address) {
        self.inner.set_rx_address(address)
    }

    fn tx_auto_ack(&mut self) -> bool {
        self.inner.tx_auto_ack()
    }

    fn rx_auto_ack(&mut self) -> bool {
        self.inner.rx_auto_ack()
    }

    fn tx_enhanced_ack(&mut self) -> bool {
        self.inner.tx_enhanced_ack()
    }

    fn pending_mode(&mut self) -> bool {
        self.inner.pending_mode()
    }

    fn set_pending_bit(&mut self, pending: bool) {
        self.inner.set_pending_bit(pending)
    }

    fn frequency_code(&mut self) -> u8 {
        self.inner.frequency_code()
    }

    fn ed_rss(&mut self) -> i8 {
        self.inner.ed_rss()
    }

    fn rssi_compensation_dbm(&self) -> i8 {
        oer_esp32s31_knowledge::ieee802154::RSSI_COMPENSATION_DBM
    }

    fn cca_busy(&mut self) -> bool {
        self.inner.cca_busy()
    }

    fn debug_counter(&mut self, counter: Ieee802154DebugCounter) -> u16 {
        self.inner.debug_counter(counter)
    }

    fn clear_debug_counter(&mut self, counter: Ieee802154DebugCounter) {
        self.inner.clear_debug_counter(counter)
    }

    fn set_ed_duration(&mut self, symbols: u16) {
        self.inner.set_ed_duration(symbols)
    }

    fn notify_enhanced_ack_generated(&mut self) {
        self.inner.notify_enhanced_ack_generated()
    }

    fn disable_rx_aborts(&mut self, set: Ieee802154RxAbortEnableSet) {
        self.inner.disable_rx_aborts(set)
    }

    fn set_multipan_panid(&mut self, index: Ieee802154MultipanIndex, panid: u16) {
        self.inner.set_multipan_panid(index, panid)
    }

    fn multipan_panid(&mut self, index: Ieee802154MultipanIndex) -> u16 {
        self.inner.multipan_panid(index)
    }

    fn set_multipan_short_address(&mut self, index: Ieee802154MultipanIndex, address: u16) {
        self.inner.set_multipan_short_address(index, address)
    }

    fn multipan_short_address(&mut self, index: Ieee802154MultipanIndex) -> u16 {
        self.inner.multipan_short_address(index)
    }

    fn set_multipan_extended_address(&mut self, index: Ieee802154MultipanIndex, address: [u8; 8]) {
        self.inner.set_multipan_extended_address(index, address)
    }

    fn multipan_extended_address(&mut self, index: Ieee802154MultipanIndex) -> [u8; 8] {
        self.inner.multipan_extended_address(index)
    }

    fn set_multipan_enable(&mut self, state: Ieee802154MultipanEnableState) {
        self.inner.set_multipan_enable(state)
    }

    fn multipan_enable(&mut self) -> Ieee802154MultipanEnableState {
        self.inner.multipan_enable()
    }

    fn set_ack_timeout(&mut self, units: u16) {
        self.inner.set_ack_timeout(units)
    }

    fn ack_timeout(&mut self) -> u16 {
        self.inner.ack_timeout()
    }

    fn set_security_address(&mut self, address: &[u8; 8]) {
        self.inner.set_security_address(address)
    }

    fn set_security_key(&mut self, key: &[u8; 16]) {
        self.inner.set_security_key(key)
    }

    fn set_security_offset(&mut self, offset: u8) {
        self.inner.set_security_offset(offset)
    }

    fn enable_all_events(&mut self) {
        self.inner.enable_all_events()
    }

    fn enable_event(&mut self, event: Ieee802154Event) {
        self.inner.enable_event(event)
    }

    fn disable_event(&mut self, event: Ieee802154Event) {
        self.inner.disable_event(event)
    }

    fn enable_tx_aborts(&mut self, set: Ieee802154TxAbortEnableSet) {
        self.inner.enable_tx_aborts(set)
    }

    fn enable_rx_aborts(&mut self, set: Ieee802154RxAbortEnableSet) {
        self.inner.enable_rx_aborts(set)
    }

    fn set_ed_sample_mode(&mut self, mode: Ieee802154EdSampleMode) {
        self.inner.set_ed_sample_mode(mode)
    }

    fn disable_coex(&mut self) {
        self.inner.disable_txrx_coex();
        self.inner.disable_ack_coex()
    }

    fn set_txrx_pti(&mut self, pti: CoexPti) {
        self.inner.set_txrx_pti(pti)
    }

    fn set_ack_pti(&mut self, pti: CoexPti) {
        self.inner.set_ack_pti(pti)
    }

    fn set_channel(&mut self, channel: Ieee802154Channel) {
        self.inner.set_channel(channel)
    }

    fn set_tx_power(&mut self, power: &Ieee802154ResolvedTxPower<'_>) {
        self.inner.set_tx_power(power)
    }

    fn set_cca_mode(&mut self, mode: Ieee802154CcaMode) {
        self.inner.set_cca_mode(mode)
    }

    fn set_cca_threshold(&mut self, threshold_dbm: i8) {
        self.inner.set_cca_threshold(threshold_dbm)
    }

    fn set_tx_auto_ack(&mut self, enable: bool) {
        self.inner.set_tx_auto_ack(enable)
    }

    fn set_rx_auto_ack(&mut self, enable: bool) {
        self.inner.set_rx_auto_ack(enable)
    }

    fn set_tx_enhanced_ack(&mut self, enable: bool) {
        self.inner.set_tx_enhanced_ack(enable)
    }

    fn set_coordinator(&mut self, enable: bool) {
        self.inner.set_coordinator(enable)
    }

    fn set_promiscuous(&mut self, enable: bool) {
        self.inner.set_promiscuous(enable)
    }

    fn set_pending_mode(&mut self, enhanced: bool) {
        self.inner.set_pending_mode(enhanced)
    }

    fn set_transmit_security(&mut self, enable: bool) {
        self.inner.set_transmit_security(enable)
    }

    fn set_timer_threshold(&mut self, timer: Ieee802154Timer, microseconds: u32) {
        self.inner.set_timer_threshold(timer, microseconds)
    }

    fn start_timer(&mut self, timer: Ieee802154Timer) {
        self.inner.start_timer(timer)
    }

    fn stop_timer(&mut self, timer: Ieee802154Timer) {
        self.inner.stop_timer(timer)
    }

    fn etm_channel_enabled(&mut self, channel: Ieee802154EtmChannel) -> bool {
        self.inner.etm_channel_enabled(channel)
    }

    fn disable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        self.inner.disable_etm_channel(channel)
    }

    fn enable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        self.inner.enable_etm_channel(channel)
    }

    fn set_etm_route(&mut self, route: Ieee802154EtmRoute) {
        self.inner.set_etm_route(route)
    }
}
