//! Typed task-side transactions, one per public ESP-IDF common-LL accessor.
//!
//! The composite configuration methods of [`Ieee802154RegisterLease`] keep
//! their reviewed write order; these methods expose the same single
//! transactions individually so a port of the public driver can issue exactly
//! the accessor sequence the vendor driver issues.

use super::{
    Ieee802154AckTimeoutUnits, Ieee802154EdSampleMode, Ieee802154Event, Ieee802154FrequencyCode,
    Ieee802154MultipanIndex, Ieee802154RegisterLease, Ieee802154RxAbortEnableSet,
    Ieee802154RxAbortReasonObservation, Ieee802154RxStateCode, Ieee802154SecurityPayloadOffset,
    Ieee802154TxAbortEnableSet, Ieee802154TxAbortReasonObservation,
    Ieee802154TxSecurityErrorObservation, Ieee802154TxStateCode, Ieee802154TxStatus,
};
use crate::ieee802154::ownership::{RawDebugCounter, RawEvent};

/// Complete `RX_STATUS` observation (`ieee802154_ll_get_rx_status`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154RxStatus {
    filter_fail_reason: u8,
    abort_reason: Ieee802154RxAbortReasonObservation,
    state: Ieee802154RxStateCode,
    preamble_match: bool,
    sfd_match: bool,
}

impl Ieee802154RxStatus {
    /// One observation, as a register model reports it. Observations grant
    /// no write authority.
    pub const fn new(
        filter_fail_reason: u8,
        abort_reason: Ieee802154RxAbortReasonObservation,
        state: Ieee802154RxStateCode,
        preamble_match: bool,
        sfd_match: bool,
    ) -> Self {
        Self {
            filter_fail_reason,
            abort_reason,
            state,
            preamble_match,
            sfd_match,
        }
    }

    /// Raw four-bit filter-failure reason; no values are classified.
    pub const fn filter_fail_reason(&self) -> u8 {
        self.filter_fail_reason
    }

    /// Receive-abort reason of the last abort.
    pub const fn abort_reason(&self) -> Ieee802154RxAbortReasonObservation {
        self.abort_reason
    }

    /// Receiver state code.
    pub const fn state(&self) -> Ieee802154RxStateCode {
        self.state
    }

    /// `ieee802154_ll_is_current_rx_frame`: the receiver has passed the SFD
    /// of a frame.
    pub const fn frame_in_progress(&self) -> bool {
        self.state.is_after_receive_sfd()
    }

    /// Whether the preamble matched.
    pub const fn preamble_match(&self) -> bool {
        self.preamble_match
    }

    /// Whether the SFD matched.
    pub const fn sfd_match(&self) -> bool {
        self.sfd_match
    }
}

/// One MAC diagnostic counter (`ieee802154_ll_get_*_cnt`).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Ieee802154DebugCounter {
    SfdTimeout,
    CrcError,
    EdAbort,
    CcaFail,
    RxFilterFail,
    NoRssDetect,
    RxAbortCoex,
    RxRestart,
    TxAckAbortCoex,
    EdScanBreakCoex,
    RxAckAbortCoex,
    RxAckTimeout,
    TxBreakCoex,
    TxSecurityError,
    CcaBusy,
}

impl Ieee802154DebugCounter {
    const fn raw(self) -> RawDebugCounter {
        match self {
            Self::SfdTimeout => RawDebugCounter::SfdTimeout,
            Self::CrcError => RawDebugCounter::CrcError,
            Self::EdAbort => RawDebugCounter::EdAbort,
            Self::CcaFail => RawDebugCounter::CcaFail,
            Self::RxFilterFail => RawDebugCounter::RxFilterFail,
            Self::NoRssDetect => RawDebugCounter::NoRssDetect,
            Self::RxAbortCoex => RawDebugCounter::RxAbortCoex,
            Self::RxRestart => RawDebugCounter::RxRestart,
            Self::TxAckAbortCoex => RawDebugCounter::TxAckAbortCoex,
            Self::EdScanBreakCoex => RawDebugCounter::EdScanBreakCoex,
            Self::RxAckAbortCoex => RawDebugCounter::RxAckAbortCoex,
            Self::RxAckTimeout => RawDebugCounter::RxAckTimeout,
            Self::TxBreakCoex => RawDebugCounter::TxBreakCoex,
            Self::TxSecurityError => RawDebugCounter::TxSecurityError,
            Self::CcaBusy => RawDebugCounter::CcaBusy,
        }
    }
}

const fn raw_event(event: Ieee802154Event) -> RawEvent {
    match event {
        Ieee802154Event::TxDone => RawEvent::TxDone,
        Ieee802154Event::RxDone => RawEvent::RxDone,
        Ieee802154Event::AckTxDone => RawEvent::AckTxDone,
        Ieee802154Event::AckRxDone => RawEvent::AckRxDone,
        Ieee802154Event::RxAbort => RawEvent::RxAbort,
        Ieee802154Event::TxAbort => RawEvent::TxAbort,
        Ieee802154Event::EdDone => RawEvent::EdDone,
        Ieee802154Event::Timer0Overflow => RawEvent::Timer0Overflow,
        Ieee802154Event::Timer1Overflow => RawEvent::Timer1Overflow,
        Ieee802154Event::ClockCountMatch => RawEvent::ClockCountMatch,
        Ieee802154Event::TxSfdDone => RawEvent::TxSfdDone,
        Ieee802154Event::RxSfdDone => RawEvent::RxSfdDone,
    }
}

impl Ieee802154RegisterLease<'_> {
    /// `ieee802154_ll_set_tx_auto_ack`.
    pub fn set_tx_auto_ack(&mut self, enable: bool) {
        self.registers.set_auto_ack_tx(enable);
    }

    /// `ieee802154_ll_get_tx_auto_ack`.
    pub fn tx_auto_ack(&self) -> bool {
        self.registers.auto_ack_tx()
    }

    /// `ieee802154_ll_set_rx_auto_ack`.
    pub fn set_rx_auto_ack(&mut self, enable: bool) {
        self.registers.set_auto_ack_rx(enable);
    }

    /// `ieee802154_ll_get_rx_auto_ack`.
    pub fn rx_auto_ack(&self) -> bool {
        self.registers.auto_ack_rx()
    }

    /// `ieee802154_ll_set_tx_enhance_ack`.
    pub fn set_enhanced_ack_tx(&mut self, enable: bool) {
        self.registers.set_enhanced_ack_tx(enable);
    }

    /// `ieee802154_ll_get_tx_enhance_ack`.
    pub fn enhanced_ack_tx(&self) -> bool {
        self.registers.enhanced_ack_tx()
    }

    /// `ieee802154_ll_set_coordinator`.
    pub fn set_coordinator(&mut self, enable: bool) {
        self.registers.set_coordinator(enable);
    }

    /// `ieee802154_ll_set_promiscuous`.
    pub fn set_promiscuous(&mut self, enable: bool) {
        self.registers.set_promiscuous(enable);
    }

    /// `ieee802154_ll_set_pending_mode`: the one-bit enhanced/Zigbee pending
    /// selector.
    pub fn set_pending_mode(&mut self, enhanced: bool) {
        self.registers.set_pending_enhanced(enhanced);
    }

    /// `ieee802154_ll_get_pending_mode`.
    pub fn pending_mode(&self) -> bool {
        self.registers.pending_enhanced()
    }

    /// `ieee802154_ll_get_freq`.
    pub fn frequency_code(&self) -> Ieee802154FrequencyCode {
        Ieee802154FrequencyCode(self.registers.frequency_code())
    }

    /// `ieee802154_ll_get_ack_timeout`.
    pub fn ack_timeout(&self) -> Ieee802154AckTimeoutUnits {
        Ieee802154AckTimeoutUnits::new(self.registers.ack_timeout())
    }

    /// `ieee802154_ll_set_multipan_panid`.
    pub fn set_multipan_pan_id(&mut self, index: Ieee802154MultipanIndex, pan_id: u16) {
        self.registers.set_multipan_pan_id(index.as_usize(), pan_id);
    }

    /// `ieee802154_ll_get_multipan_panid`.
    pub fn multipan_pan_id(&self, index: Ieee802154MultipanIndex) -> u16 {
        self.registers.multipan_pan_id(index.as_usize())
    }

    /// `ieee802154_ll_set_multipan_short_addr`.
    pub fn set_multipan_short_address(&mut self, index: Ieee802154MultipanIndex, address: u16) {
        self.registers
            .set_multipan_short_address(index.as_usize(), address);
    }

    /// `ieee802154_ll_get_multipan_short_addr`.
    pub fn multipan_short_address(&self, index: Ieee802154MultipanIndex) -> u16 {
        self.registers.multipan_short_address(index.as_usize())
    }

    /// `ieee802154_ll_set_multipan_ext_addr`.
    pub fn set_multipan_extended_address(
        &mut self,
        index: Ieee802154MultipanIndex,
        address: [u8; 8],
    ) {
        self.registers
            .set_multipan_extended_address(index.as_usize(), address);
    }

    /// `ieee802154_ll_get_multipan_ext_addr`.
    pub fn multipan_extended_address(&self, index: Ieee802154MultipanIndex) -> [u8; 8] {
        self.registers.multipan_extended_address(index.as_usize())
    }

    /// `ieee802154_ll_set_ed_sample_mode`.
    pub fn set_ed_sample_mode(&mut self, mode: Ieee802154EdSampleMode) {
        self.registers
            .set_ed_sample_mode(matches!(mode, Ieee802154EdSampleMode::Average));
    }

    /// `ieee802154_ll_enable_events(IEEE802154_EVENT_MASK)`.
    pub fn enable_all_events(&mut self) {
        self.registers.enable_all_events();
    }

    /// `ieee802154_ll_enable_events` of one event.
    pub fn enable_event(&mut self, event: Ieee802154Event) {
        self.registers.set_event_enabled(raw_event(event), true);
    }

    /// `ieee802154_ll_disable_events` of one event.
    pub fn disable_event(&mut self, event: Ieee802154Event) {
        self.registers.set_event_enabled(raw_event(event), false);
    }

    /// `ieee802154_ll_enable_rx_abort_events`.
    pub fn enable_rx_aborts(&mut self, set: Ieee802154RxAbortEnableSet) {
        self.registers.update_rx_abort_enable(set.mask(), true);
    }

    /// `ieee802154_ll_disable_rx_abort_events`.
    pub fn disable_rx_aborts(&mut self, set: Ieee802154RxAbortEnableSet) {
        self.registers.update_rx_abort_enable(set.mask(), false);
    }

    /// `ieee802154_ll_enable_tx_abort_events`.
    pub fn enable_tx_aborts(&mut self, set: Ieee802154TxAbortEnableSet) {
        self.registers.update_tx_abort_enable(set.mask(), true);
    }

    /// `ieee802154_ll_disable_tx_abort_events`.
    pub fn disable_tx_aborts(&mut self, set: Ieee802154TxAbortEnableSet) {
        self.registers.update_tx_abort_enable(set.mask(), false);
    }

    /// `ieee802154_ll_get_rx_status`.
    pub fn rx_status(&self) -> Ieee802154RxStatus {
        let status = self.registers.rx_status();
        Ieee802154RxStatus {
            filter_fail_reason: status.filter_fail_reason,
            abort_reason: Ieee802154RxAbortReasonObservation::from_field(status.abort_reason),
            state: Ieee802154RxStateCode::from_field(status.state),
            preamble_match: status.preamble_match,
            sfd_match: status.sfd_match,
        }
    }

    /// `ieee802154_ll_get_tx_status`.
    pub fn tx_status(&self) -> Ieee802154TxStatus {
        let status = self.registers.tx_status();
        Ieee802154TxStatus::new(
            Ieee802154TxStateCode::from_field(status.state),
            Ieee802154TxAbortReasonObservation::from_field(status.abort_reason),
            Ieee802154TxSecurityErrorObservation::from_field(status.security_error),
        )
    }

    /// `ieee802154_ll_set_transmit_security`.
    pub fn set_transmit_security(&mut self, enable: bool) {
        self.registers.set_transmit_security(enable);
    }

    /// `ieee802154_ll_set_security_offset`.
    pub fn set_security_payload_offset(&mut self, offset: Ieee802154SecurityPayloadOffset) {
        self.registers.set_security_payload_offset(offset.value());
    }

    /// `ieee802154_ll_get_security_offset`.
    pub fn security_payload_offset(&self) -> Ieee802154SecurityPayloadOffset {
        Ieee802154SecurityPayloadOffset::from_field(self.registers.security_payload_offset())
    }

    /// `ieee802154_ll_set_security_addr`.
    pub fn set_security_address(&mut self, address: &[u8; 8]) {
        self.registers.set_security_address(address);
    }

    /// `ieee802154_ll_set_security_key`.
    pub fn set_security_key(&mut self, key: &[u8; 16]) {
        self.registers.set_security_key(key);
    }

    /// `ieee802154_ll_get_*_cnt` as the reviewed sixteen-bit field.
    pub fn debug_counter(&self, counter: Ieee802154DebugCounter) -> u16 {
        self.registers.debug_counter(counter.raw())
    }

    /// `ieee802154_ll_clear_debug_cnt` of one counter.
    pub fn clear_debug_counter(&mut self, counter: Ieee802154DebugCounter) {
        self.registers.clear_debug_counter(counter.raw());
    }
}

#[cfg(test)]
mod tests;
