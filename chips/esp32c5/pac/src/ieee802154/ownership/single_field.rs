//! Single-field transactions matching the public ESP-IDF common LL one to one.
//!
//! Each method is exactly one `ieee802154_ll_*` accessor of the pinned
//! `ieee802154_common_ll.h`: one field read, one read-modify-write of one
//! field, or one register write. Composite configuration methods elsewhere in
//! this module are built from the same port steps, so both paths keep the
//! vendor write order.

use super::{
    KEY_WORDS, MultipanIdentityProgrammingPort, MultipanIndex, TaskRegisters,
    TransmitSecurityProgrammingPort, mac,
};

/// Complete `RX_STATUS` observation (`ieee802154_ll_get_rx_status`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawRxStatus {
    pub filter_fail_reason: u8,
    pub abort_reason: u8,
    pub state: u8,
    pub preamble_match: bool,
    pub sfd_match: bool,
}

/// Complete `TX_STATUS` observation (`ieee802154_ll_get_tx_status`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawTxStatus {
    pub state: u8,
    pub abort_reason: u8,
    pub security_error: u8,
}

/// One diagnostic counter field (`ieee802154_ll_get_*_cnt`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RawDebugCounter {
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

/// Named event-enable bits (`ieee802154_ll_enable_events`/`disable_events`
/// with one event).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RawEvent {
    TxDone,
    RxDone,
    AckTxDone,
    AckRxDone,
    RxAbort,
    TxAbort,
    EdDone,
    Timer0Overflow,
    Timer1Overflow,
    ClockCountMatch,
    TxSfdDone,
    RxSfdDone,
}

impl TaskRegisters {
    /// `ieee802154_ll_set_tx_auto_ack`.
    #[doc(hidden)]
    pub fn set_auto_ack_tx(&mut self, enable: bool) {
        self.registers.modify_control(|writer| {
            writer.set_auto_ack_tx(enable);
        });
    }

    /// `ieee802154_ll_get_tx_auto_ack`.
    #[doc(hidden)]
    pub fn auto_ack_tx(&self) -> bool {
        self.registers.read_control().auto_ack_tx()
    }

    /// `ieee802154_ll_set_rx_auto_ack`.
    #[doc(hidden)]
    pub fn set_auto_ack_rx(&mut self, enable: bool) {
        self.registers.modify_control(|writer| {
            writer.set_auto_ack_rx(enable);
        });
    }

    /// `ieee802154_ll_get_rx_auto_ack`.
    #[doc(hidden)]
    pub fn auto_ack_rx(&self) -> bool {
        self.registers.read_control().auto_ack_rx()
    }

    /// `ieee802154_ll_set_tx_enhance_ack`.
    #[doc(hidden)]
    pub fn set_enhanced_ack_tx(&mut self, enable: bool) {
        self.registers.modify_control(|writer| {
            writer.set_enhanced_ack_tx(enable);
        });
    }

    /// `ieee802154_ll_get_tx_enhance_ack`.
    #[doc(hidden)]
    pub fn enhanced_ack_tx(&self) -> bool {
        self.registers.read_control().enhanced_ack_tx()
    }

    /// `ieee802154_ll_set_coordinator`.
    #[doc(hidden)]
    pub fn set_coordinator(&mut self, enable: bool) {
        self.registers.modify_control(|writer| {
            writer.set_coordinator(enable);
        });
    }

    /// `ieee802154_ll_set_promiscuous`.
    #[doc(hidden)]
    pub fn set_promiscuous(&mut self, enable: bool) {
        self.registers.modify_control(|writer| {
            writer.set_promiscuous(enable);
        });
    }

    /// `ieee802154_ll_set_pending_mode`.
    #[doc(hidden)]
    pub fn set_pending_enhanced(&mut self, enable: bool) {
        self.registers.modify_control(|writer| {
            writer.set_pending_enhanced(enable);
        });
    }

    /// `ieee802154_ll_get_pending_mode`.
    #[doc(hidden)]
    pub fn pending_enhanced(&self) -> bool {
        self.registers.read_control().pending_enhanced()
    }

    /// `ieee802154_ll_get_freq`.
    #[doc(hidden)]
    pub fn frequency_code(&self) -> u8 {
        self.registers.read_channel().frequency_code()
    }

    /// `ieee802154_ll_get_ack_timeout`.
    #[doc(hidden)]
    pub fn ack_timeout(&self) -> u16 {
        self.registers.read_ack_timeout().timeout()
    }

    /// `ieee802154_ll_set_multipan_panid`: enable the context, then write.
    #[doc(hidden)]
    pub fn set_multipan_pan_id(&mut self, index: MultipanIndex, pan_id: u16) {
        self.registers.enable_context(index);
        self.registers.write_pan_id(index, pan_id);
    }

    /// `ieee802154_ll_get_multipan_panid`.
    #[doc(hidden)]
    pub fn multipan_pan_id(&self, index: MultipanIndex) -> u16 {
        self.registers.read_multipan_pan_id(index).pan_id()
    }

    /// `ieee802154_ll_set_multipan_short_addr`: enable the context, then write.
    #[doc(hidden)]
    pub fn set_multipan_short_address(&mut self, index: MultipanIndex, short_address: u16) {
        self.registers.enable_context(index);
        self.registers.write_short_address(index, short_address);
    }

    /// `ieee802154_ll_get_multipan_short_addr`.
    #[doc(hidden)]
    pub fn multipan_short_address(&self, index: MultipanIndex) -> u16 {
        self.registers.read_multipan_short_address(index).address()
    }

    /// `ieee802154_ll_set_multipan_ext_addr`: enable the context, then write
    /// the low and high little-endian words.
    #[doc(hidden)]
    pub fn set_multipan_extended_address(&mut self, index: MultipanIndex, address: [u8; 8]) {
        self.registers.enable_context(index);
        let [a0, a1, a2, a3, a4, a5, a6, a7] = address;
        self.registers
            .write_extended_address_low(index, u32::from_le_bytes([a0, a1, a2, a3]));
        self.registers
            .write_extended_address_high(index, u32::from_le_bytes([a4, a5, a6, a7]));
    }

    /// `ieee802154_ll_get_multipan_ext_addr`.
    #[doc(hidden)]
    pub fn multipan_extended_address(&self, index: MultipanIndex) -> [u8; 8] {
        self.multipan_identity(index).extended_address
    }

    /// `ieee802154_ll_set_ed_sample_mode`.
    #[doc(hidden)]
    pub fn set_ed_sample_mode(&mut self, average: bool) {
        let mode = if average {
            mac::ed_config::EdSampleMode::Average
        } else {
            mac::ed_config::EdSampleMode::Maximum
        };
        self.registers.modify_ed_config(|writer| {
            writer.set_ed_sample_mode(mode);
        });
    }

    /// `ieee802154_ll_enable_events(IEEE802154_EVENT_MASK)`: set all thirteen
    /// enable bits, including the two unclassified ones the public mask
    /// covers.
    #[doc(hidden)]
    pub fn enable_all_events(&mut self) {
        self.registers.modify_event_enable(|writer| {
            writer.set_tx_done(true);
            writer.set_rx_done(true);
            writer.set_ack_tx_done(true);
            writer.set_ack_rx_done(true);
            writer.set_rx_abort(true);
            writer.set_tx_abort(true);
            writer.set_ed_done(true);
            writer.set_events_7(true);
            writer.set_timer0_overflow(true);
            writer.set_timer1_overflow(true);
            writer.set_clock_count_match(true);
            writer.set_tx_sfd_done(true);
            writer.set_rx_sfd_done(true);
        });
    }

    /// `ieee802154_ll_enable_events`/`disable_events` of one named event.
    #[doc(hidden)]
    pub fn set_event_enabled(&mut self, event: RawEvent, enable: bool) {
        self.registers.modify_event_enable(|writer| {
            match event {
                RawEvent::TxDone => writer.set_tx_done(enable),
                RawEvent::RxDone => writer.set_rx_done(enable),
                RawEvent::AckTxDone => writer.set_ack_tx_done(enable),
                RawEvent::AckRxDone => writer.set_ack_rx_done(enable),
                RawEvent::RxAbort => writer.set_rx_abort(enable),
                RawEvent::TxAbort => writer.set_tx_abort(enable),
                RawEvent::EdDone => writer.set_ed_done(enable),
                RawEvent::Timer0Overflow => writer.set_timer0_overflow(enable),
                RawEvent::Timer1Overflow => writer.set_timer1_overflow(enable),
                RawEvent::ClockCountMatch => writer.set_clock_count_match(enable),
                RawEvent::TxSfdDone => writer.set_tx_sfd_done(enable),
                RawEvent::RxSfdDone => writer.set_rx_sfd_done(enable),
            };
        });
    }

    /// The LL's `|=` of the rx-abort baseline set.
    #[doc(hidden)]
    pub fn enable_rx_abort_baseline(&mut self) {
        self.registers.enable_ieee802154_rx_abort_baseline();
    }

    /// The LL's `|=` of the rx-abort all set.
    #[doc(hidden)]
    pub fn enable_rx_abort_all(&mut self) {
        self.registers.enable_ieee802154_rx_abort_all();
    }

    /// The LL's `|=` of the tx-abort baseline set.
    #[doc(hidden)]
    pub fn enable_tx_abort_baseline(&mut self) {
        self.registers.enable_ieee802154_tx_abort_baseline();
    }

    /// The LL's `|=` of the tx-abort all set.
    #[doc(hidden)]
    pub fn enable_tx_abort_all(&mut self) {
        self.registers.enable_ieee802154_tx_abort_all();
    }

    /// The LL's `&= !` of the rx-abort baseline set.
    #[doc(hidden)]
    pub fn disable_rx_abort_baseline(&mut self) {
        self.registers.disable_ieee802154_rx_abort_baseline();
    }

    /// The LL's `&= !` of the rx-abort all set.
    #[doc(hidden)]
    pub fn disable_rx_abort_all(&mut self) {
        self.registers.disable_ieee802154_rx_abort_all();
    }

    /// The LL's `&= !` of the tx-abort baseline set.
    #[doc(hidden)]
    pub fn disable_tx_abort_baseline(&mut self) {
        self.registers.disable_ieee802154_tx_abort_baseline();
    }

    /// The LL's `&= !` of the tx-abort all set.
    #[doc(hidden)]
    pub fn disable_tx_abort_all(&mut self) {
        self.registers.disable_ieee802154_tx_abort_all();
    }

    /// `ieee802154_ll_get_rx_status`, decoded field by field.
    #[doc(hidden)]
    pub fn rx_status(&self) -> RawRxStatus {
        let status = self.registers.read_rx_status();
        RawRxStatus {
            filter_fail_reason: status.filter_fail_reason(),
            abort_reason: status.abort_reason(),
            state: status.state(),
            preamble_match: status.preamble_match(),
            sfd_match: status.sfd_match(),
        }
    }

    /// `ieee802154_ll_get_tx_status`, decoded field by field.
    #[doc(hidden)]
    pub fn tx_status(&self) -> RawTxStatus {
        let status = self.registers.read_tx_status();
        RawTxStatus {
            state: status.state(),
            abort_reason: status.abort_reason(),
            security_error: status.security_error(),
        }
    }

    /// `ieee802154_ll_set_transmit_security`.
    #[doc(hidden)]
    pub fn set_transmit_security(&mut self, enable: bool) {
        self.registers.set_enabled(enable);
    }

    /// `ieee802154_ll_set_security_offset`.
    #[doc(hidden)]
    pub fn set_security_payload_offset(&mut self, offset: u8) {
        self.registers.write_payload_offset(offset);
    }

    /// `ieee802154_ll_get_security_offset`.
    #[doc(hidden)]
    pub fn security_payload_offset(&self) -> u8 {
        self.registers.read_security_control().payload_offset()
    }

    /// `ieee802154_ll_set_security_addr`: low then high little-endian word.
    #[doc(hidden)]
    pub fn set_security_address(&mut self, address: &[u8; 8]) {
        let [a0, a1, a2, a3, a4, a5, a6, a7] = *address;
        self.registers
            .write_address_low(u32::from_le_bytes([a0, a1, a2, a3]));
        self.registers
            .write_address_high(u32::from_le_bytes([a4, a5, a6, a7]));
    }

    /// `ieee802154_ll_set_security_key`: four little-endian words in order.
    #[doc(hidden)]
    pub fn set_security_key(&mut self, key: &[u8; 16]) {
        let (words, _) = key.as_chunks::<4>();
        for (index, word) in KEY_WORDS.into_iter().zip(words) {
            self.registers
                .write_key_word(index, u32::from_le_bytes(*word));
        }
    }

    /// `ieee802154_ll_get_*_cnt`. The reviewed register model publishes each
    /// counter as a sixteen-bit field, following `ieee802154_reg.h`; the
    /// public struct reads the complete word for the single-counter
    /// registers.
    #[doc(hidden)]
    pub fn debug_counter(&self, counter: RawDebugCounter) -> u16 {
        let registers = &self.registers;
        match counter {
            RawDebugCounter::SfdTimeout => registers.read_sfd_timeout_counter().sfd_timeout_count(),
            RawDebugCounter::CrcError => registers.read_crc_error_counter().crc_error_count(),
            RawDebugCounter::EdAbort => registers.read_ed_abort_counter().count(),
            RawDebugCounter::CcaFail => registers.read_cca_fail_counter().count(),
            RawDebugCounter::RxFilterFail => registers.read_rx_filter_fail_counter().count(),
            RawDebugCounter::NoRssDetect => registers.read_no_rss_detect_counter().count(),
            RawDebugCounter::RxAbortCoex => registers.read_rx_abort_coex_counter().count(),
            RawDebugCounter::RxRestart => registers.read_rx_restart_counter().count(),
            RawDebugCounter::TxAckAbortCoex => registers.read_tx_ack_abort_coex_counter().count(),
            RawDebugCounter::EdScanBreakCoex => registers.read_ed_scan_break_coex_counter().count(),
            RawDebugCounter::RxAckAbortCoex => registers.read_rx_ack_abort_coex_counter().count(),
            RawDebugCounter::RxAckTimeout => registers.read_rx_ack_timeout_counter().count(),
            RawDebugCounter::TxBreakCoex => registers.read_tx_break_coex_counter().count(),
            RawDebugCounter::TxSecurityError => registers.read_tx_security_error_counter().count(),
            RawDebugCounter::CcaBusy => registers.read_cca_busy_counter().count(),
        }
    }

    /// `ieee802154_ll_clear_debug_cnt` of one counter: a complete write of
    /// the clear register with only that counter's bit set, as the public LL
    /// does.
    #[doc(hidden)]
    pub fn clear_debug_counter(&mut self, counter: RawDebugCounter) {
        self.registers
            .write_diagnostic_counter_clear(|writer| match counter {
                RawDebugCounter::SfdTimeout => {
                    writer.set_sfd_timeout(true);
                }
                RawDebugCounter::CrcError => {
                    writer.set_crc_error(true);
                }
                RawDebugCounter::EdAbort => {
                    writer.set_ed_abort(true);
                }
                RawDebugCounter::CcaFail => {
                    writer.set_cca_fail(true);
                }
                RawDebugCounter::RxFilterFail => {
                    writer.set_rx_filter_fail(true);
                }
                RawDebugCounter::NoRssDetect => {
                    writer.set_no_rss_detect(true);
                }
                RawDebugCounter::RxAbortCoex => {
                    writer.set_rx_abort_coex(true);
                }
                RawDebugCounter::RxRestart => {
                    writer.set_rx_restart(true);
                }
                RawDebugCounter::TxAckAbortCoex => {
                    writer.set_tx_ack_abort_coex(true);
                }
                RawDebugCounter::EdScanBreakCoex => {
                    writer.set_ed_scan_break_coex(true);
                }
                RawDebugCounter::RxAckAbortCoex => {
                    writer.set_rx_ack_abort_coex(true);
                }
                RawDebugCounter::RxAckTimeout => {
                    writer.set_rx_ack_timeout(true);
                }
                RawDebugCounter::TxBreakCoex => {
                    writer.set_tx_break_coex(true);
                }
                RawDebugCounter::TxSecurityError => {
                    writer.set_tx_security_error(true);
                }
                RawDebugCounter::CcaBusy => {
                    writer.set_cca_busy(true);
                }
            });
    }
}
