//! Typed snapshots of hardware RX counters recovered from the blob decoders.

use crate::WifiRadioRegisters;

/// BSS-color collision state read by the complete blob debug decoder.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacHeColorCollisionSnapshot {
    pub observed_color_bitmap: u64,
    pub collision_threshold: u8,
    pub timeout_seconds: u8,
    pub color_bitmap_clear: bool,
    pub bitmap_control_high: bool,
}

/// Ordinary receive, completion and interface counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacRxPrimaryStatistics {
    pub mpdu_count: u16,
    /// Signed upper half of `WDEVRX_MPDU`, multiplied by 40 exactly as the
    /// complete blob decoder prints `WDEVRX_CFO`.
    pub cfo_scaled_40: i32,
    pub fcs_error: u16,
    pub abort: u16,
    pub abort_fcs_pass: u16,
    pub power_drop_error: u16,
    pub he_sig_b_error: u16,
    pub same_bm_error: u16,
    pub signal_field: u16,
    pub end: u16,
    pub data_success: u16,
    pub other_unicast: u16,
    pub buffer_full: u16,
    pub fifo_overflow: u16,
    pub tkip_error: u16,
    pub bt_block_error: u16,
    pub frequency_hop_error: u16,
    pub last_unmatched_error: u16,
    pub ack_interrupt: u16,
    pub rts_interrupt: u16,
}

/// Wrapping deltas for the 16-bit counters in [`MacRxPrimaryStatistics`].
///
/// `cfo_scaled_40` is intentionally absent: it is a signed accumulator sample,
/// not an event counter.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacRxPrimaryStatisticsDelta {
    pub mpdu_count: u16,
    pub fcs_error: u16,
    pub abort: u16,
    pub abort_fcs_pass: u16,
    pub power_drop_error: u16,
    pub he_sig_b_error: u16,
    pub same_bm_error: u16,
    pub signal_field: u16,
    pub end: u16,
    pub data_success: u16,
    pub other_unicast: u16,
    pub buffer_full: u16,
    pub fifo_overflow: u16,
    pub tkip_error: u16,
    pub bt_block_error: u16,
    pub frequency_hop_error: u16,
    pub last_unmatched_error: u16,
    pub ack_interrupt: u16,
    pub rts_interrupt: u16,
}

impl MacRxPrimaryStatistics {
    /// Return counter increments since an earlier hardware snapshot.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_debug.o]::
    /// dbg_read_rx_count` reads these values as unsigned halfwords. Hardware
    /// runs show the primary counters crossing `0xffff` during sustained HE20
    /// traffic, so interval telemetry must use 16-bit wrapping subtraction
    /// rather than comparing absolute boot-lifetime values.
    pub fn wrapping_delta_since(self, earlier: Self) -> MacRxPrimaryStatisticsDelta {
        MacRxPrimaryStatisticsDelta {
            mpdu_count: self.mpdu_count.wrapping_sub(earlier.mpdu_count),
            fcs_error: self.fcs_error.wrapping_sub(earlier.fcs_error),
            abort: self.abort.wrapping_sub(earlier.abort),
            abort_fcs_pass: self.abort_fcs_pass.wrapping_sub(earlier.abort_fcs_pass),
            power_drop_error: self.power_drop_error.wrapping_sub(earlier.power_drop_error),
            he_sig_b_error: self.he_sig_b_error.wrapping_sub(earlier.he_sig_b_error),
            same_bm_error: self.same_bm_error.wrapping_sub(earlier.same_bm_error),
            signal_field: self.signal_field.wrapping_sub(earlier.signal_field),
            end: self.end.wrapping_sub(earlier.end),
            data_success: self.data_success.wrapping_sub(earlier.data_success),
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
        }
    }
}

/// Ten-bit baseband and normal-RX decoder error counters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacRxDecodeErrorStatistics {
    pub brx_agc: u16,
    pub brx: u16,
    pub nrx: u16,
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
}

/// Wrapping deltas for the ten-bit decoder counters.
///
/// These counters wrap at `0x400`, not at the Rust storage width. Keeping the
/// arithmetic beside the recovered register representation prevents telemetry
/// consumers from accidentally interpreting a wrap as roughly 65k errors.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacRxDecodeErrorStatisticsDelta {
    pub brx_agc: u16,
    pub brx: u16,
    pub nrx: u16,
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
}

impl MacRxDecodeErrorStatistics {
    pub fn wrapping_delta_since(self, earlier: Self) -> MacRxDecodeErrorStatisticsDelta {
        const MASK: u16 = 0x03ff;
        let delta = |current: u16, previous: u16| current.wrapping_sub(previous) & MASK;
        MacRxDecodeErrorStatisticsDelta {
            brx_agc: delta(self.brx_agc, earlier.brx_agc),
            brx: delta(self.brx, earlier.brx),
            nrx: delta(self.nrx, earlier.nrx),
            nrx_abort: delta(self.nrx_abort, earlier.nrx_abort),
            nrx_agc_exit: delta(self.nrx_agc_exit, earlier.nrx_agc_exit),
            nrx_baseband_off: delta(self.nrx_baseband_off, earlier.nrx_baseband_off),
            nrx_fdm_watchdog: delta(self.nrx_fdm_watchdog, earlier.nrx_fdm_watchdog),
            nrx_restart: delta(self.nrx_restart, earlier.nrx_restart),
            nrx_service: delta(self.nrx_service, earlier.nrx_service),
            nrx_tx_over: delta(self.nrx_tx_over, earlier.nrx_tx_over),
            nrx_unsupported: delta(self.nrx_unsupported, earlier.nrx_unsupported),
            nrx_he_format: delta(self.nrx_he_format, earlier.nrx_he_format),
            nrx_ht_sig: delta(self.nrx_ht_sig, earlier.nrx_ht_sig),
            nrx_he_unsupported: delta(self.nrx_he_unsupported, earlier.nrx_he_unsupported),
            nrx_he_sig_a_crc: delta(self.nrx_he_sig_a_crc, earlier.nrx_he_sig_a_crc),
        }
    }
}

/// MAC/baseband hang counters exposed by the same complete decoder.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacRxHangStatistics {
    pub rx: u8,
    pub tx: u8,
    pub rx_tx_hang: u32,
    pub rx_tx_panic: u32,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MacRxHangStatisticsDelta {
    pub rx: u8,
    pub tx: u8,
    pub rx_tx_hang: u32,
    pub rx_tx_panic: u32,
}

impl MacRxHangStatistics {
    pub fn wrapping_delta_since(self, earlier: Self) -> MacRxHangStatisticsDelta {
        MacRxHangStatisticsDelta {
            rx: self.rx.wrapping_sub(earlier.rx),
            tx: self.tx.wrapping_sub(earlier.tx),
            rx_tx_hang: self.rx_tx_hang.wrapping_sub(earlier.rx_tx_hang),
            rx_tx_panic: self.rx_tx_panic.wrapping_sub(earlier.rx_tx_panic),
        }
    }
}

/// One allocation-free view of every field printed by `dbg_read_rx_count`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacRxStatisticsSnapshot {
    pub primary: MacRxPrimaryStatistics,
    pub decode_errors: MacRxDecodeErrorStatistics,
    pub hang: MacRxHangStatistics,
}

impl WifiRadioRegisters {
    /// Read only the hang/panic counters for a bounded wait diagnostic.
    /// These are cumulative counters, not a live CCA or receiver-state decode.
    pub fn rx_hang_statistics_snapshot(&self) -> MacRxHangStatistics {
        let hangs = &self.peripherals.wifi_mac.wifi_mac_rx_hang_statistics;
        let hang = hangs.hang().read();
        MacRxHangStatistics {
            rx: hang.rx_count().bits(),
            tx: hang.tx_count().bits(),
            rx_tx_hang: hangs.rx_tx_hang().read().count().bits(),
            rx_tx_panic: hangs.rx_tx_panic().read().count().bits(),
        }
    }

    /// Read only the receive-buffer starvation counter.
    ///
    /// Unlike [`Self::rx_statistics_snapshot`], this single-register accessor
    /// is cheap enough to sample at an RX service boundary. It exists so the
    /// live owner can correlate a newly observed starvation event with its
    /// descriptor frontier and software credits without decoding the complete
    /// diagnostic register bank on every wake.
    pub fn mac_rx_buffer_full_count(&self) -> u16 {
        self.peripherals
            .wifi_mac
            .wifi_mac_rx_statistics
            .buffer_full()
            .read()
            .count()
            .bits()
    }

    /// Read the recovered HE BSS-color collision state.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_debug.o]`
    /// `dbg_read_color_collision`, size `0x64`, and its format strings.
    pub fn he_color_collision_snapshot(&self) -> MacHeColorCollisionSnapshot {
        let colors = &self.peripherals.wifi_mac.wifi_mac_he_color_collision;
        let control = self
            .peripherals
            .wifi_mac
            .wifi_mac_he_init_prefix
            .rx_field_control()
            .read();
        let low = colors.bss_color_bitmap_low().read().value().bits();
        let high = colors.bss_color_bitmap_high().read().value().bits();

        MacHeColorCollisionSnapshot {
            observed_color_bitmap: u64::from(low) | (u64::from(high) << 32),
            collision_threshold: control.collision_threshold().bits(),
            timeout_seconds: control.timeout_seconds().bits(),
            color_bitmap_clear: control.color_bitmap_clear().bit(),
            bitmap_control_high: control.bitmap_control_high().bit(),
        }
    }

    /// Read every statistic decoded by the complete `dbg_read_rx_count`.
    ///
    /// This is a best-effort multi-register diagnostic sample: the hardware
    /// counters continue changing while the snapshot is assembled.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_debug.o]`
    /// `dbg_read_rx_count`, size `0x21a`, and its two format strings.
    pub fn rx_statistics_snapshot(&self) -> MacRxStatisticsSnapshot {
        let registers = &self.peripherals.wifi_mac.wifi_mac_rx_statistics;
        let mpdu_and_cfo = registers.mpdu_and_cfo().read();
        let abort = registers.abort().read();
        let hangs = &self.peripherals.wifi_mac.wifi_mac_rx_hang_statistics;
        let hang = hangs.hang().read();

        MacRxStatisticsSnapshot {
            primary: MacRxPrimaryStatistics {
                mpdu_count: mpdu_and_cfo.mpdu_count().bits(),
                cfo_scaled_40: i32::from(mpdu_and_cfo.cfo_accumulator().bits() as i16) * 40,
                fcs_error: registers.fcs_error().read().count().bits(),
                abort: abort.count().bits(),
                abort_fcs_pass: abort.fcs_pass_count().bits(),
                power_drop_error: registers.nrx_error_power_drop().read().count().bits(),
                he_sig_b_error: registers.nrx_he_sig_b_error().read().count().bits(),
                same_bm_error: registers.same_bm_error().read().count().bits(),
                signal_field: registers.signal_field().read().count().bits(),
                end: registers.end().read().count().bits(),
                data_success: registers.data_success().read().count().bits(),
                other_unicast: registers.other_unicast().read().count().bits(),
                buffer_full: registers.buffer_full().read().count().bits(),
                fifo_overflow: registers.fifo_overflow().read().count().bits(),
                tkip_error: registers.tkip_error().read().count().bits(),
                bt_block_error: registers.bt_block_error().read().count().bits(),
                frequency_hop_error: registers.frequency_hop_error().read().count().bits(),
                last_unmatched_error: registers.last_unmatched_error().read().count().bits(),
                ack_interrupt: registers.ack_interrupt().read().count().bits(),
                rts_interrupt: registers.rts_interrupt().read().count().bits(),
            },
            decode_errors: MacRxDecodeErrorStatistics {
                brx_agc: registers.brx_error_agc().read().count().bits(),
                brx: registers.brx_error().read().count().bits(),
                nrx: registers.nrx_error().read().count().bits(),
                nrx_abort: registers.nrx_error_abort().read().count().bits(),
                nrx_agc_exit: registers.nrx_error_agc_exit().read().count().bits(),
                nrx_baseband_off: registers.nrx_error_baseband_off().read().count().bits(),
                nrx_fdm_watchdog: registers.nrx_error_fdm_watchdog().read().count().bits(),
                nrx_restart: registers.nrx_error_restart().read().count().bits(),
                nrx_service: registers.nrx_error_service().read().count().bits(),
                nrx_tx_over: registers.nrx_error_tx_over().read().count().bits(),
                nrx_unsupported: registers.nrx_unsupported().read().count().bits(),
                nrx_he_format: registers.nrx_he_format().read().count().bits(),
                nrx_ht_sig: registers.nrx_ht_sig_error().read().count().bits(),
                nrx_he_unsupported: registers.nrx_he_unsupported().read().count().bits(),
                nrx_he_sig_a_crc: registers.nrx_he_sig_a_crc().read().count().bits(),
            },
            hang: MacRxHangStatistics {
                rx: hang.rx_count().bits(),
                tx: hang.tx_count().bits(),
                rx_tx_hang: hangs.rx_tx_hang().read().count().bits(),
                rx_tx_panic: hangs.rx_tx_panic().read().count().bits(),
            },
        }
    }
}

#[cfg(test)]
mod tests;
