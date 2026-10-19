//! Single-field transactions matching the public ESP-IDF common LL one to one.
//!
//! Each method is exactly one `ieee802154_ll_*` accessor of the pinned
//! `ieee802154_common_ll.h`: one field read, one read-modify-write of one
//! field, or one register write. Composite configuration methods elsewhere in
//! this module are built from the same port steps, so both paths keep the
//! vendor write order.

use super::{MultipanIdentityProgrammingPort, TaskRegisters, TransmitSecurityProgrammingPort};

/// Complete `RX_STATUS` observation (`ieee802154_ll_get_rx_status`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawRxStatus {
    pub filter_fail_reason: u8,
    pub abort_reason: u8,
    pub state: u8,
    pub current_channel_index: bool,
    pub preamble_match: bool,
    pub sfd_match: bool,
}

/// The MAC power-sequencing words from `PAON_DELAY` to `DCDC_CTRL`, field by
/// field; `reserved` holds each word's vendor-reserved remainder in address
/// order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RawPowerSequence {
    pub paon_delay: u16,
    pub txon_delay: u16,
    pub txen_stop_delay: u8,
    pub txoff_delay: u8,
    pub rxon_delay: u16,
    pub txrx_switch_delay: u16,
    pub cont_rx_delay: u8,
    pub dcdc_pre_up_delay: u8,
    pub dcdc_down_delay: u8,
    pub dcdc_ctrl_enabled: bool,
    pub tx_dcdc_up: bool,
    pub reserved: [u32; 8],
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
    RxFilterNotWork,
    CrcError,
    RxPreambleDetectError,
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
        self.registers
            .control()
            .modify(|_, writer| writer.auto_ack_tx().bit(enable));
    }

    /// `ieee802154_ll_get_tx_auto_ack`.
    #[doc(hidden)]
    pub fn auto_ack_tx(&self) -> bool {
        self.registers.control().read().auto_ack_tx().bit_is_set()
    }

    /// `ieee802154_ll_set_rx_auto_ack`.
    #[doc(hidden)]
    pub fn set_auto_ack_rx(&mut self, enable: bool) {
        self.registers
            .control()
            .modify(|_, writer| writer.auto_ack_rx().bit(enable));
    }

    /// `ieee802154_ll_get_rx_auto_ack`.
    #[doc(hidden)]
    pub fn auto_ack_rx(&self) -> bool {
        self.registers.control().read().auto_ack_rx().bit_is_set()
    }

    /// `ieee802154_ll_set_tx_enhance_ack`.
    #[doc(hidden)]
    pub fn set_enhanced_ack_tx(&mut self, enable: bool) {
        self.registers
            .control()
            .modify(|_, writer| writer.enhanced_ack_tx().bit(enable));
    }

    /// `ieee802154_ll_get_tx_enhance_ack`.
    #[doc(hidden)]
    pub fn enhanced_ack_tx(&self) -> bool {
        self.registers
            .control()
            .read()
            .enhanced_ack_tx()
            .bit_is_set()
    }

    /// `ieee802154_ll_set_coordinator`.
    #[doc(hidden)]
    pub fn set_coordinator(&mut self, enable: bool) {
        self.registers
            .control()
            .modify(|_, writer| writer.coordinator().bit(enable));
    }

    /// `ieee802154_ll_set_promiscuous`.
    #[doc(hidden)]
    pub fn set_promiscuous(&mut self, enable: bool) {
        self.registers
            .control()
            .modify(|_, writer| writer.promiscuous().bit(enable));
    }

    /// `ieee802154_ll_set_pending_mode`.
    #[doc(hidden)]
    pub fn set_pending_enhanced(&mut self, enable: bool) {
        self.registers
            .control()
            .modify(|_, writer| writer.pending_enhanced().bit(enable));
    }

    /// `ieee802154_ll_get_pending_mode`.
    #[doc(hidden)]
    pub fn pending_enhanced(&self) -> bool {
        self.registers
            .control()
            .read()
            .pending_enhanced()
            .bit_is_set()
    }

    /// `ieee802154_ll_get_freq`.
    #[doc(hidden)]
    pub fn frequency_code(&self) -> u8 {
        self.registers.channel().read().frequency_code().bits()
    }

    /// `ieee802154_ll_get_ack_timeout`.
    #[doc(hidden)]
    pub fn ack_timeout(&self) -> u16 {
        self.registers.ack_timeout().read().timeout().bits()
    }

    /// `ieee802154_ll_set_multipan_panid`: enable the context, then write.
    #[doc(hidden)]
    pub fn set_multipan_pan_id(&mut self, index: usize, pan_id: u16) {
        assert!(index < 4, "multipan index exceeds four contexts");
        self.registers.enable_context(index);
        self.registers.write_pan_id(index, pan_id);
    }

    /// `ieee802154_ll_get_multipan_panid`.
    #[doc(hidden)]
    pub fn multipan_pan_id(&self, index: usize) -> u16 {
        assert!(index < 4, "multipan index exceeds four contexts");
        self.registers.multipan_pan_id(index).read().pan_id().bits()
    }

    /// `ieee802154_ll_set_multipan_short_addr`: enable the context, then write.
    #[doc(hidden)]
    pub fn set_multipan_short_address(&mut self, index: usize, short_address: u16) {
        assert!(index < 4, "multipan index exceeds four contexts");
        self.registers.enable_context(index);
        self.registers.write_short_address(index, short_address);
    }

    /// `ieee802154_ll_get_multipan_short_addr`.
    #[doc(hidden)]
    pub fn multipan_short_address(&self, index: usize) -> u16 {
        assert!(index < 4, "multipan index exceeds four contexts");
        self.registers
            .multipan_short_address(index)
            .read()
            .address()
            .bits()
    }

    /// `ieee802154_ll_set_multipan_ext_addr`: enable the context, then write
    /// the low and high little-endian words.
    #[doc(hidden)]
    pub fn set_multipan_extended_address(&mut self, index: usize, address: [u8; 8]) {
        assert!(index < 4, "multipan index exceeds four contexts");
        self.registers.enable_context(index);
        let [a0, a1, a2, a3, a4, a5, a6, a7] = address;
        self.registers
            .write_extended_address_low(index, u32::from_le_bytes([a0, a1, a2, a3]));
        self.registers
            .write_extended_address_high(index, u32::from_le_bytes([a4, a5, a6, a7]));
    }

    /// `ieee802154_ll_get_multipan_ext_addr`.
    #[doc(hidden)]
    pub fn multipan_extended_address(&self, index: usize) -> [u8; 8] {
        self.multipan_identity(index).extended_address
    }

    /// `ieee802154_ll_set_ed_sample_mode`.
    #[doc(hidden)]
    pub fn set_ed_sample_mode(&mut self, average: bool) {
        self.registers.ed_config().modify(|_, writer| {
            if average {
                writer.ed_sample_mode().average()
            } else {
                writer.ed_sample_mode().maximum()
            }
        });
    }

    /// `ieee802154_ll_enable_events(IEEE802154_EVENT_MASK)`: set all fourteen
    /// enable bits, including the two unclassified ones the public mask
    /// covers.
    #[doc(hidden)]
    pub fn enable_all_events(&mut self) {
        self.registers.event_enable().modify(|_, writer| {
            writer.tx_done().set_bit();
            writer.rx_done().set_bit();
            writer.ack_tx_done().set_bit();
            writer.ack_rx_done().set_bit();
            writer.rx_abort().set_bit();
            writer.tx_abort().set_bit();
            writer.ed_done().set_bit();
            writer.bit_7_opaque().set_bit();
            writer.timer0_overflow().set_bit();
            writer.timer1_overflow().set_bit();
            writer.clock_count_match().set_bit();
            writer.tx_sfd_done().set_bit();
            writer.rx_sfd_done().set_bit();
            writer.bit_13_opaque().set_bit()
        });
    }

    /// `ieee802154_ll_enable_events`/`disable_events` of one named event.
    #[doc(hidden)]
    pub fn set_event_enabled(&mut self, event: RawEvent, enable: bool) {
        self.registers
            .event_enable()
            .modify(|_, writer| match event {
                RawEvent::TxDone => writer.tx_done().bit(enable),
                RawEvent::RxDone => writer.rx_done().bit(enable),
                RawEvent::AckTxDone => writer.ack_tx_done().bit(enable),
                RawEvent::AckRxDone => writer.ack_rx_done().bit(enable),
                RawEvent::RxAbort => writer.rx_abort().bit(enable),
                RawEvent::TxAbort => writer.tx_abort().bit(enable),
                RawEvent::EdDone => writer.ed_done().bit(enable),
                RawEvent::Timer0Overflow => writer.timer0_overflow().bit(enable),
                RawEvent::Timer1Overflow => writer.timer1_overflow().bit(enable),
                RawEvent::ClockCountMatch => writer.clock_count_match().bit(enable),
                RawEvent::TxSfdDone => writer.tx_sfd_done().bit(enable),
                RawEvent::RxSfdDone => writer.rx_sfd_done().bit(enable),
            });
    }

    /// `ieee802154_ll_enable_rx_abort_events` (`|=`) and
    /// `ieee802154_ll_disable_rx_abort_events` (`&= ~`) of one reviewed mask.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the thirty-one-bit mask is one of the reviewed public LL masks"
    )]
    pub fn update_rx_abort_enable(&mut self, mask: u32, enable: bool) {
        assert!(
            mask <= 0x7fff_ffff,
            "receive-abort mask exceeds thirty-one bits"
        );
        self.registers.rx_abort_enable().modify(|read, writer| {
            let current = read.events().bits();
            let next = if enable {
                current | mask
            } else {
                current & !mask
            };
            // SAFETY: `next` stays within the thirty-one-bit field and is
            // composed only from the current image and a reviewed LL mask.
            unsafe { writer.events().bits(next) }
        });
    }

    /// `ieee802154_ll_enable_tx_abort_events` (`|=`) and
    /// `ieee802154_ll_disable_tx_abort_events` (`&= ~`) of one reviewed mask.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the thirty-one-bit mask is one of the reviewed public LL masks"
    )]
    pub fn update_tx_abort_enable(&mut self, mask: u32, enable: bool) {
        assert!(
            mask <= 0x7fff_ffff,
            "transmit-abort mask exceeds thirty-one bits"
        );
        self.registers.tx_abort_enable().modify(|read, writer| {
            let current = read.events().bits();
            let next = if enable {
                current | mask
            } else {
                current & !mask
            };
            // SAFETY: `next` stays within the thirty-one-bit field and is
            // composed only from the current image and a reviewed LL mask.
            unsafe { writer.events().bits(next) }
        });
    }

    /// `ieee802154_ll_get_rx_status`, decoded field by field.
    #[doc(hidden)]
    pub fn rx_status(&self) -> RawRxStatus {
        let status = self.registers.rx_status().read();
        RawRxStatus {
            filter_fail_reason: status.filter_fail_reason().bits(),
            abort_reason: status.abort_reason().bits(),
            state: status.state().bits(),
            current_channel_index: status.current_channel_index().bit_is_set(),
            preamble_match: status.preamble_match().bit_is_set(),
            sfd_match: status.sfd_match().bit_is_set(),
        }
    }

    /// The eight words from `PAON_DELAY` to `DCDC_CTRL`, each read once and
    /// decoded field by field.
    #[doc(hidden)]
    pub fn power_sequence(&self) -> RawPowerSequence {
        let registers = &self.registers;
        let paon = registers.paon_delay().read();
        let txon = registers.txon_delay().read();
        let txen_stop = registers.txen_stop_delay().read();
        let txoff = registers.txoff_delay().read();
        let rxon = registers.rxon_delay().read();
        let txrx_switch = registers.txrx_switch_delay().read();
        let cont_rx = registers.cont_rx_delay().read();
        let dcdc = registers.dcdc_ctrl().read();
        RawPowerSequence {
            paon_delay: paon.paon_delay().bits(),
            txon_delay: txon.txon_delay().bits(),
            txen_stop_delay: txen_stop.txen_stop_dly().bits(),
            txoff_delay: txoff.txoff_delay().bits(),
            rxon_delay: rxon.rxon_delay().bits(),
            txrx_switch_delay: txrx_switch.txrx_switch_delay().bits(),
            cont_rx_delay: cont_rx.cont_rx_delay().bits(),
            dcdc_pre_up_delay: dcdc.dcdc_pre_up_delay().bits(),
            dcdc_down_delay: dcdc.dcdc_down_delay().bits(),
            dcdc_ctrl_enabled: dcdc.dcdc_ctrl_en().bit_is_set(),
            tx_dcdc_up: dcdc.tx_dcdc_up().bit_is_set(),
            reserved: [
                paon.bits_10_31_opaque().bits(),
                txon.bits_10_31_opaque().bits(),
                txen_stop.bits_6_31_opaque().bits(),
                txoff.bits_6_31_opaque().bits(),
                rxon.bits_11_31_opaque().bits(),
                txrx_switch.bits_10_31_opaque().bits(),
                cont_rx.bits_6_31_opaque().bits(),
                u32::from(dcdc.bits_17_30_opaque().bits()),
            ],
        }
    }

    /// The transmit and receive DMA error codes.
    #[doc(hidden)]
    pub fn dma_error_codes(&self) -> (u8, u8) {
        (
            self.registers.txdma_err().read().txdma_err().bits(),
            self.registers.rxdma_err().read().rxdma_err().bits(),
        )
    }

    /// `ieee802154_ll_get_tx_status`, decoded field by field.
    #[doc(hidden)]
    pub fn tx_status(&self) -> RawTxStatus {
        let status = self.registers.tx_status().read();
        RawTxStatus {
            state: status.state().bits(),
            abort_reason: status.abort_reason().bits(),
            security_error: status.security_error().bits(),
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
        assert!(
            offset <= 0x7f,
            "transmit-security payload offset exceeds seven bits"
        );
        self.registers.write_payload_offset(offset);
    }

    /// `ieee802154_ll_get_security_offset`.
    #[doc(hidden)]
    pub fn security_payload_offset(&self) -> u8 {
        self.registers
            .security_control()
            .read()
            .payload_offset()
            .bits()
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
        for (index, word) in key.chunks_exact(4).enumerate() {
            self.registers.write_key_word(
                index,
                u32::from_le_bytes([word[0], word[1], word[2], word[3]]),
            );
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
            RawDebugCounter::SfdTimeout => registers
                .sfd_timeout_counter()
                .read()
                .sfd_timeout_count()
                .bits(),
            RawDebugCounter::RxFilterNotWork => registers
                .sfd_timeout_counter()
                .read()
                .rx_filter_not_work_count()
                .bits(),
            RawDebugCounter::CrcError => registers
                .crc_error_counter()
                .read()
                .crc_error_count()
                .bits(),
            RawDebugCounter::RxPreambleDetectError => registers
                .crc_error_counter()
                .read()
                .rx_preamble_detect_error_count()
                .bits(),
            RawDebugCounter::EdAbort => registers.ed_abort_counter().read().count().bits(),
            RawDebugCounter::CcaFail => registers.cca_fail_counter().read().count().bits(),
            RawDebugCounter::RxFilterFail => {
                registers.rx_filter_fail_counter().read().count().bits()
            }
            RawDebugCounter::NoRssDetect => registers.no_rss_detect_counter().read().count().bits(),
            RawDebugCounter::RxAbortCoex => registers.rx_abort_coex_counter().read().count().bits(),
            RawDebugCounter::RxRestart => registers.rx_restart_counter().read().count().bits(),
            RawDebugCounter::TxAckAbortCoex => {
                registers.tx_ack_abort_coex_counter().read().count().bits()
            }
            RawDebugCounter::EdScanBreakCoex => {
                registers.ed_scan_break_coex_counter().read().count().bits()
            }
            RawDebugCounter::RxAckAbortCoex => {
                registers.rx_ack_abort_coex_counter().read().count().bits()
            }
            RawDebugCounter::RxAckTimeout => {
                registers.rx_ack_timeout_counter().read().count().bits()
            }
            RawDebugCounter::TxBreakCoex => registers.tx_break_coex_counter().read().count().bits(),
            RawDebugCounter::TxSecurityError => {
                registers.tx_security_error_counter().read().count().bits()
            }
            RawDebugCounter::CcaBusy => registers.cca_busy_counter().read().count().bits(),
        }
    }

    /// `ieee802154_ll_clear_debug_cnt` of one counter: a complete write of
    /// the clear register with only that counter's bit set.
    #[doc(hidden)]
    #[allow(
        unsafe_code,
        reason = "the write-only clear register is written as one complete word, as the public LL does"
    )]
    pub fn clear_debug_counter(&mut self, counter: RawDebugCounter) {
        // SAFETY: `DIAGNOSTIC_COUNTER_CLEAR` is a write-only word of
        // independent clear bits; the public LL writes it completely with only
        // the selected bits set, so every other bit is zero.
        let _ = unsafe {
            self.registers
                .diagnostic_counter_clear()
                .write_with_zero(|writer| match counter {
                    RawDebugCounter::SfdTimeout => writer.sfd_timeout().set_bit(),
                    RawDebugCounter::RxFilterNotWork => writer.rx_filter_not_work().set_bit(),
                    RawDebugCounter::CrcError => writer.crc_error().set_bit(),
                    RawDebugCounter::RxPreambleDetectError => {
                        writer.rx_preamble_detect_error().set_bit()
                    }
                    RawDebugCounter::EdAbort => writer.ed_abort().set_bit(),
                    RawDebugCounter::CcaFail => writer.cca_fail().set_bit(),
                    RawDebugCounter::RxFilterFail => writer.rx_filter_fail().set_bit(),
                    RawDebugCounter::NoRssDetect => writer.no_rss_detect().set_bit(),
                    RawDebugCounter::RxAbortCoex => writer.rx_abort_coex().set_bit(),
                    RawDebugCounter::RxRestart => writer.rx_restart().set_bit(),
                    RawDebugCounter::TxAckAbortCoex => writer.tx_ack_abort_coex().set_bit(),
                    RawDebugCounter::EdScanBreakCoex => writer.ed_scan_break_coex().set_bit(),
                    RawDebugCounter::RxAckAbortCoex => writer.rx_ack_abort_coex().set_bit(),
                    RawDebugCounter::RxAckTimeout => writer.rx_ack_timeout().set_bit(),
                    RawDebugCounter::TxBreakCoex => writer.tx_break_coex().set_bit(),
                    RawDebugCounter::TxSecurityError => writer.tx_security_error().set_bit(),
                    RawDebugCounter::CcaBusy => writer.cca_busy().set_bit(),
                })
        };
    }
}
