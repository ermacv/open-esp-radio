//! A register model of [`Ieee802154LowLevel`] for host tests.
//!
//! Getters return the value their setter last wrote; the test sets the
//! event image, abort reasons and measurement results the MAC would latch.
//! The model knows nothing else about the hardware.

use super::{
    COEX_DISABLED_PTI, Ieee802154LlCommand, Ieee802154LowLevel, Ieee802154RecentRssi,
    Ieee802154Timer,
};
use crate::{channel::*, coex::*, tx_power::*, types::*};

/// The modelled MAC state; every field is public for the test to set or
/// inspect.
#[derive(Clone, Debug)]
pub struct Ieee802154LlModel {
    /// Latched events; `clear_events` clears them.
    pub events: Ieee802154EventMask,
    /// Latched receive-abort reason.
    pub rx_abort: Ieee802154RxAbortReasonObservation,
    /// Latched transmit-abort reason.
    pub tx_abort: Ieee802154TxAbortReasonObservation,
    /// Whether a frame is past its SFD.
    pub current_rx_frame: bool,
    /// Energy-detection result.
    pub ed_rss: i8,
    /// The live RSSI [`Ieee802154RecentRssi::recent_rssi`] reads.
    pub recent_rssi: i8,
    /// CCA result.
    pub cca_busy: bool,
    /// The last operation command.
    pub command: Option<Ieee802154LlCommand>,
    /// The last published receive buffer.
    pub rx_address: Option<u32>,
    /// The last published transmit buffer.
    pub tx_address: Option<u32>,
    /// The published channel's frequency code.
    pub frequency_code: u8,
    /// Hardware ACK transmission.
    pub tx_auto_ack: bool,
    /// ACK reception after transmission.
    pub rx_auto_ack: bool,
    /// Enhanced-ACK transmission.
    pub tx_enhanced_ack: bool,
    /// Enhanced pending lookup.
    pub pending_mode: bool,
    /// The last pending bit.
    pub pending_bit: bool,
    /// Promiscuous receive.
    pub promiscuous: bool,
    /// Transmit security enable.
    pub transmit_security: bool,
    /// The transmit security nonce address.
    pub security_address: [u8; 8],
    /// The transmit security key.
    pub security_key: [u8; 16],
    /// PAN ID of each context.
    pub panid: [u16; 4],
    /// Short address of each context.
    pub short_address: [u16; 4],
    /// Extended address of each context.
    pub extended_address: [[u8; 8]; 4],
    /// ACK timeout in 16-microsecond units.
    pub ack_timeout: u16,
    /// ETM channel enables.
    pub etm_enabled: [bool; 2],
    /// Multi-PAN context enables.
    pub multipan_enable: Ieee802154MultipanEnableState,
    /// TX/RX PTI field value.
    pub txrx_pti: u8,
    /// ACK PTI field value.
    pub ack_pti: u8,
    /// The MAC diagnostic counters, in `Ieee802154DebugCounter` order.
    pub debug_counters: [u16; DEBUG_COUNTERS],
}

impl Default for Ieee802154LlModel {
    fn default() -> Self {
        Self {
            events: Ieee802154EventMask::NONE,
            rx_abort: Ieee802154RxAbortReasonObservation::Unclassified,
            tx_abort: Ieee802154TxAbortReasonObservation::Unclassified,
            current_rx_frame: false,
            ed_rss: 0,
            recent_rssi: 0,
            cca_busy: false,
            command: None,
            rx_address: None,
            tx_address: None,
            frequency_code: 0,
            tx_auto_ack: false,
            rx_auto_ack: false,
            tx_enhanced_ack: false,
            pending_mode: false,
            pending_bit: false,
            promiscuous: false,
            transmit_security: false,
            security_address: [0; 8],
            security_key: [0; 16],
            panid: [0; 4],
            short_address: [0; 4],
            extended_address: [[0; 8]; 4],
            ack_timeout: 0,
            etm_enabled: [false; 2],
            multipan_enable: Ieee802154MultipanEnableState::NONE,
            txrx_pti: 0,
            ack_pti: 0,
            debug_counters: [0; DEBUG_COUNTERS],
        }
    }
}

/// The common LL's diagnostic counters.
pub const DEBUG_COUNTERS: usize = 15;

/// The model slot of a diagnostic counter in `debug_counters`.
pub const fn debug_slot(counter: Ieee802154DebugCounter) -> usize {
    match counter {
        Ieee802154DebugCounter::SfdTimeout => 0,
        Ieee802154DebugCounter::CrcError => 1,
        Ieee802154DebugCounter::EdAbort => 2,
        Ieee802154DebugCounter::CcaFail => 3,
        Ieee802154DebugCounter::RxFilterFail => 4,
        Ieee802154DebugCounter::NoRssDetect => 5,
        Ieee802154DebugCounter::RxAbortCoex => 6,
        Ieee802154DebugCounter::RxRestart => 7,
        Ieee802154DebugCounter::TxAckAbortCoex => 8,
        Ieee802154DebugCounter::EdScanBreakCoex => 9,
        Ieee802154DebugCounter::RxAckAbortCoex => 10,
        Ieee802154DebugCounter::RxAckTimeout => 11,
        Ieee802154DebugCounter::TxBreakCoex => 12,
        Ieee802154DebugCounter::TxSecurityError => 13,
        Ieee802154DebugCounter::CcaBusy => 14,
    }
}

impl Ieee802154LlModel {
    /// Latch `events` for the next interrupt.
    pub fn raise(&mut self, events: &[Ieee802154Event]) {
        self.events = events
            .iter()
            .fold(self.events, |mask, event| mask.union(event.mask()));
    }

    /// The published channel.
    pub fn channel(&self) -> Option<Ieee802154Channel> {
        Ieee802154Channel::from_frequency_code(self.frequency_code)
    }
}

const fn etm_index(channel: Ieee802154EtmChannel) -> usize {
    match channel {
        Ieee802154EtmChannel::Channel0 => 0,
        Ieee802154EtmChannel::Channel1 => 1,
    }
}

impl Ieee802154LowLevel for Ieee802154LlModel {
    fn set_command(&mut self, command: Ieee802154LlCommand) {
        self.command = Some(command);
    }
    fn events(&mut self) -> Ieee802154EventObservation {
        Ieee802154EventObservation::from_named(self.events)
    }
    fn clear_events(&mut self, mask: Ieee802154EventMask) {
        self.events = self.events.difference(mask);
    }
    fn rx_abort_reason(&mut self) -> Ieee802154RxAbortReasonObservation {
        self.rx_abort
    }
    fn tx_abort_reason(&mut self) -> Ieee802154TxAbortReasonObservation {
        self.tx_abort
    }
    fn rx_status(&mut self) -> Ieee802154RxStatus {
        Ieee802154RxStatus::new(
            0,
            self.rx_abort,
            super::Ieee802154RxStateCode::new(0).expect("zero is a state code"),
            false,
            false,
        )
    }
    fn is_current_rx_frame(&mut self) -> bool {
        self.current_rx_frame
    }
    fn set_tx_address(&mut self, address: u32) {
        self.tx_address = Some(address);
    }
    fn set_rx_address(&mut self, address: u32) {
        self.rx_address = Some(address);
    }
    fn tx_auto_ack(&mut self) -> bool {
        self.tx_auto_ack
    }
    fn rx_auto_ack(&mut self) -> bool {
        self.rx_auto_ack
    }
    fn tx_enhanced_ack(&mut self) -> bool {
        self.tx_enhanced_ack
    }
    fn pending_mode(&mut self) -> bool {
        self.pending_mode
    }
    fn set_pending_bit(&mut self, pending: bool) {
        self.pending_bit = pending;
    }
    fn frequency_code(&mut self) -> u8 {
        self.frequency_code
    }
    fn ed_rss(&mut self) -> i8 {
        self.ed_rss
    }
    fn cca_busy(&mut self) -> bool {
        self.cca_busy
    }
    fn debug_counter(&mut self, counter: Ieee802154DebugCounter) -> u16 {
        self.debug_counters[debug_slot(counter)]
    }
    fn clear_debug_counter(&mut self, counter: Ieee802154DebugCounter) {
        self.debug_counters[debug_slot(counter)] = 0;
    }
    fn set_ed_duration(&mut self, _symbols: u16) {}
    fn notify_enhanced_ack_generated(&mut self) {}
    fn disable_rx_aborts(&mut self, _set: Ieee802154RxAbortEnableSet) {}
    fn set_multipan_panid(&mut self, index: Ieee802154MultipanIndex, panid: u16) {
        self.multipan_enable = self.multipan_enable.with(index);
        self.panid[usize::from(index.value())] = panid;
    }
    fn multipan_panid(&mut self, index: Ieee802154MultipanIndex) -> u16 {
        self.panid[usize::from(index.value())]
    }
    fn set_multipan_short_address(&mut self, index: Ieee802154MultipanIndex, address: u16) {
        self.multipan_enable = self.multipan_enable.with(index);
        self.short_address[usize::from(index.value())] = address;
    }
    fn multipan_short_address(&mut self, index: Ieee802154MultipanIndex) -> u16 {
        self.short_address[usize::from(index.value())]
    }
    fn set_multipan_extended_address(&mut self, index: Ieee802154MultipanIndex, address: [u8; 8]) {
        self.multipan_enable = self.multipan_enable.with(index);
        self.extended_address[usize::from(index.value())] = address;
    }
    fn multipan_extended_address(&mut self, index: Ieee802154MultipanIndex) -> [u8; 8] {
        self.extended_address[usize::from(index.value())]
    }
    fn set_multipan_enable(&mut self, state: Ieee802154MultipanEnableState) {
        self.multipan_enable = state;
    }
    fn multipan_enable(&mut self) -> Ieee802154MultipanEnableState {
        self.multipan_enable
    }
    fn set_ack_timeout(&mut self, units: u16) {
        self.ack_timeout = units;
    }
    fn ack_timeout(&mut self) -> u16 {
        self.ack_timeout
    }
    fn set_security_address(&mut self, address: &[u8; 8]) {
        self.security_address = *address;
    }
    fn set_security_key(&mut self, key: &[u8; 16]) {
        self.security_key = *key;
    }
    fn set_security_offset(&mut self, _offset: u8) {}
    fn enable_all_events(&mut self) {}
    fn enable_event(&mut self, _event: Ieee802154Event) {}
    fn disable_event(&mut self, _event: Ieee802154Event) {}
    fn enable_tx_aborts(&mut self, _set: Ieee802154TxAbortEnableSet) {}
    fn enable_rx_aborts(&mut self, _set: Ieee802154RxAbortEnableSet) {}
    fn set_ed_sample_mode(&mut self, _mode: Ieee802154EdSampleMode) {}
    fn disable_coex(&mut self) {
        self.txrx_pti = COEX_DISABLED_PTI;
        self.ack_pti = COEX_DISABLED_PTI;
    }
    fn set_txrx_pti(&mut self, pti: CoexPti) {
        self.txrx_pti = pti.value();
    }
    fn set_ack_pti(&mut self, pti: CoexPti) {
        self.ack_pti = pti.value();
    }
    fn set_channel(&mut self, channel: Ieee802154Channel) {
        self.frequency_code = channel.frequency_code();
    }
    fn set_tx_power(&mut self, _power: &Ieee802154ResolvedTxPower<'_>) {}
    fn set_cca_mode(&mut self, _mode: Ieee802154CcaMode) {}
    fn set_cca_threshold(&mut self, _threshold_dbm: i8) {}
    fn set_tx_auto_ack(&mut self, enable: bool) {
        self.tx_auto_ack = enable;
    }
    fn set_rx_auto_ack(&mut self, enable: bool) {
        self.rx_auto_ack = enable;
    }
    fn set_tx_enhanced_ack(&mut self, enable: bool) {
        self.tx_enhanced_ack = enable;
    }
    fn set_coordinator(&mut self, _enable: bool) {}
    fn set_promiscuous(&mut self, enable: bool) {
        self.promiscuous = enable;
    }
    fn set_pending_mode(&mut self, enhanced: bool) {
        self.pending_mode = enhanced;
    }
    fn set_transmit_security(&mut self, enable: bool) {
        self.transmit_security = enable;
    }
    fn set_timer_threshold(&mut self, _timer: Ieee802154Timer, _microseconds: u32) {}
    fn start_timer(&mut self, _timer: Ieee802154Timer) {}
    fn stop_timer(&mut self, _timer: Ieee802154Timer) {}
    fn etm_channel_enabled(&mut self, channel: Ieee802154EtmChannel) -> bool {
        self.etm_enabled[etm_index(channel)]
    }
    fn disable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        self.etm_enabled[etm_index(channel)] = false;
    }
    fn enable_etm_channel(&mut self, channel: Ieee802154EtmChannel) {
        self.etm_enabled[etm_index(channel)] = true;
    }
    fn set_etm_route(&mut self, _route: Ieee802154EtmRoute) {}
}

impl Ieee802154RecentRssi for Ieee802154LlModel {
    fn recent_rssi(&mut self) -> i8 {
        self.recent_rssi
    }
}
