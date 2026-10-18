//! Value-only RX observations and conversion into HIL wire evidence.

#[cfg(feature = "station-exit-evidence")]
use oer_hil_protocol::wifi::WifiMacRxHardwareEvidence;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::product_hil) struct ObservedRxStatistics {
    pub mpdu_count: u16,
    pub data_success: u16,
    pub fcs_error: u16,
    pub abort: u16,
    pub abort_fcs_pass: u16,
    pub power_drop_error: u16,
    pub he_sig_b_error: u16,
    pub same_bm_error: u16,
    pub signal_field: u16,
    pub end: u16,
    pub other_unicast: u16,
    pub buffer_full: u16,
    pub fifo_overflow: u16,
    pub tkip_error: u16,
    pub bt_block_error: u16,
    pub frequency_hop_error: u16,
    pub last_unmatched_error: u16,
    pub ack_interrupt: u16,
    pub rts_interrupt: u16,
    pub brx_agc_error: u16,
    pub brx_error: u16,
    pub nrx_error: u16,
    pub nrx_abort: u16,
    pub nrx_agc_exit: u16,
    pub nrx_baseband_off: u16,
    pub nrx_fdm_watchdog: u16,
    pub nrx_restart: u16,
    pub nrx_service: u16,
    pub nrx_tx_over: u16,
    pub nrx_unsupported: u16,
    pub nrx_he_format: u16,
    pub nrx_ht_sig: u16,
    pub nrx_he_unsupported: u16,
    pub nrx_he_sig_a_crc: u16,
    pub rx_hang: u8,
    pub tx_hang: u8,
    pub rx_tx_hang: u32,
    pub rx_tx_panic: u32,
}

impl ObservedRxStatistics {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        const DECODE_MASK: u16 = 0x03ff;
        let decode_delta =
            |current: u16, previous: u16| current.wrapping_sub(previous) & DECODE_MASK;
        Self {
            mpdu_count: self.mpdu_count.wrapping_sub(earlier.mpdu_count),
            data_success: self.data_success.wrapping_sub(earlier.data_success),
            fcs_error: self.fcs_error.wrapping_sub(earlier.fcs_error),
            abort: self.abort.wrapping_sub(earlier.abort),
            abort_fcs_pass: self.abort_fcs_pass.wrapping_sub(earlier.abort_fcs_pass),
            power_drop_error: self.power_drop_error.wrapping_sub(earlier.power_drop_error),
            he_sig_b_error: self.he_sig_b_error.wrapping_sub(earlier.he_sig_b_error),
            same_bm_error: self.same_bm_error.wrapping_sub(earlier.same_bm_error),
            signal_field: self.signal_field.wrapping_sub(earlier.signal_field),
            end: self.end.wrapping_sub(earlier.end),
            other_unicast: self.other_unicast.wrapping_sub(earlier.other_unicast),
            buffer_full: self.buffer_full.wrapping_sub(earlier.buffer_full),
            fifo_overflow: self.fifo_overflow.wrapping_sub(earlier.fifo_overflow),
            tkip_error: self.tkip_error.wrapping_sub(earlier.tkip_error),
            bt_block_error: self.bt_block_error.wrapping_sub(earlier.bt_block_error),
            frequency_hop_error: self
                .frequency_hop_error
                .wrapping_sub(earlier.frequency_hop_error),
            last_unmatched_error: self
                .last_unmatched_error
                .wrapping_sub(earlier.last_unmatched_error),
            ack_interrupt: self.ack_interrupt.wrapping_sub(earlier.ack_interrupt),
            rts_interrupt: self.rts_interrupt.wrapping_sub(earlier.rts_interrupt),
            brx_agc_error: decode_delta(self.brx_agc_error, earlier.brx_agc_error),
            brx_error: decode_delta(self.brx_error, earlier.brx_error),
            nrx_error: decode_delta(self.nrx_error, earlier.nrx_error),
            nrx_abort: decode_delta(self.nrx_abort, earlier.nrx_abort),
            nrx_agc_exit: decode_delta(self.nrx_agc_exit, earlier.nrx_agc_exit),
            nrx_baseband_off: decode_delta(self.nrx_baseband_off, earlier.nrx_baseband_off),
            nrx_fdm_watchdog: decode_delta(self.nrx_fdm_watchdog, earlier.nrx_fdm_watchdog),
            nrx_restart: decode_delta(self.nrx_restart, earlier.nrx_restart),
            nrx_service: decode_delta(self.nrx_service, earlier.nrx_service),
            nrx_tx_over: decode_delta(self.nrx_tx_over, earlier.nrx_tx_over),
            nrx_unsupported: decode_delta(self.nrx_unsupported, earlier.nrx_unsupported),
            nrx_he_format: decode_delta(self.nrx_he_format, earlier.nrx_he_format),
            nrx_ht_sig: decode_delta(self.nrx_ht_sig, earlier.nrx_ht_sig),
            nrx_he_unsupported: decode_delta(self.nrx_he_unsupported, earlier.nrx_he_unsupported),
            nrx_he_sig_a_crc: decode_delta(self.nrx_he_sig_a_crc, earlier.nrx_he_sig_a_crc),
            rx_hang: self.rx_hang.wrapping_sub(earlier.rx_hang),
            tx_hang: self.tx_hang.wrapping_sub(earlier.tx_hang),
            rx_tx_hang: self.rx_tx_hang.wrapping_sub(earlier.rx_tx_hang),
            rx_tx_panic: self.rx_tx_panic.wrapping_sub(earlier.rx_tx_panic),
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(feature = "station-exit-evidence")]
impl From<oer_esp32s31_ieee80211_system::DiagnosticRxStatistics> for ObservedRxStatistics {
    fn from(statistics: oer_esp32s31_ieee80211_system::DiagnosticRxStatistics) -> Self {
        Self {
            mpdu_count: statistics.mpdu_count,
            data_success: statistics.data_success,
            fcs_error: statistics.fcs_error,
            abort: statistics.abort,
            abort_fcs_pass: statistics.abort_fcs_pass,
            power_drop_error: statistics.power_drop_error,
            he_sig_b_error: statistics.he_sig_b_error,
            same_bm_error: statistics.same_bm_error,
            signal_field: statistics.signal_field,
            end: statistics.end,
            other_unicast: statistics.other_unicast,
            buffer_full: statistics.buffer_full,
            fifo_overflow: statistics.fifo_overflow,
            tkip_error: statistics.tkip_error,
            bt_block_error: statistics.bt_block_error,
            frequency_hop_error: statistics.frequency_hop_error,
            last_unmatched_error: statistics.last_unmatched_error,
            ack_interrupt: statistics.ack_interrupt,
            rts_interrupt: statistics.rts_interrupt,
            brx_agc_error: statistics.brx_agc_error,
            brx_error: statistics.brx_error,
            nrx_error: statistics.nrx_error,
            nrx_abort: statistics.nrx_abort,
            nrx_agc_exit: statistics.nrx_agc_exit,
            nrx_baseband_off: statistics.nrx_baseband_off,
            nrx_fdm_watchdog: statistics.nrx_fdm_watchdog,
            nrx_restart: statistics.nrx_restart,
            nrx_service: statistics.nrx_service,
            nrx_tx_over: statistics.nrx_tx_over,
            nrx_unsupported: statistics.nrx_unsupported,
            nrx_he_format: statistics.nrx_he_format,
            nrx_ht_sig: statistics.nrx_ht_sig,
            nrx_he_unsupported: statistics.nrx_he_unsupported,
            nrx_he_sig_a_crc: statistics.nrx_he_sig_a_crc,
            rx_hang: statistics.rx_hang,
            tx_hang: statistics.tx_hang,
            rx_tx_hang: statistics.rx_tx_hang,
            rx_tx_panic: statistics.rx_tx_panic,
        }
    }
}

#[cfg(feature = "station-exit-evidence")]
impl From<ObservedRxStatistics> for WifiMacRxHardwareEvidence {
    fn from(statistics: ObservedRxStatistics) -> Self {
        Self {
            mpdu_count: statistics.mpdu_count,
            data_success: statistics.data_success,
            fcs_error: statistics.fcs_error,
            abort: statistics.abort,
            abort_fcs_pass: statistics.abort_fcs_pass,
            power_drop_error: statistics.power_drop_error,
            he_sig_b_error: statistics.he_sig_b_error,
            same_bm_error: statistics.same_bm_error,
            signal_field: statistics.signal_field,
            end: statistics.end,
            other_unicast: statistics.other_unicast,
            buffer_full: statistics.buffer_full,
            fifo_overflow: statistics.fifo_overflow,
            tkip_error: statistics.tkip_error,
            bluetooth_block_error: statistics.bt_block_error,
            frequency_hop_error: statistics.frequency_hop_error,
            last_unmatched_error: statistics.last_unmatched_error,
            ack_interrupt: statistics.ack_interrupt,
            rts_interrupt: statistics.rts_interrupt,
            brx_agc_error: statistics.brx_agc_error,
            brx_error: statistics.brx_error,
            nrx_error: statistics.nrx_error,
            nrx_abort: statistics.nrx_abort,
            nrx_agc_exit: statistics.nrx_agc_exit,
            nrx_baseband_off: statistics.nrx_baseband_off,
            nrx_fdm_watchdog: statistics.nrx_fdm_watchdog,
            nrx_restart: statistics.nrx_restart,
            nrx_service: statistics.nrx_service,
            nrx_tx_over: statistics.nrx_tx_over,
            nrx_unsupported: statistics.nrx_unsupported,
            nrx_he_format: statistics.nrx_he_format,
            nrx_ht_sig: statistics.nrx_ht_sig,
            nrx_he_unsupported: statistics.nrx_he_unsupported,
            nrx_he_sig_a_crc: statistics.nrx_he_sig_a_crc,
            rx_hang: statistics.rx_hang,
            tx_hang: statistics.tx_hang,
            rx_tx_hang: statistics.rx_tx_hang,
            rx_tx_panic: statistics.rx_tx_panic,
        }
    }
}
