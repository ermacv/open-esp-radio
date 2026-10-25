//! Generated-PAC ownership for bounded parts of complete `hal_he_init`.

#![forbid(unsafe_code)]

use crate::{SharedRadioRegisters, WifiRadioRegisters};

impl WifiRadioRegisters {
    /// Apply the complete prefix through `hal_init_tb_tx`.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]` parent and
    /// leaves recorded as `BLOB_LIBPP_HAL_HE_INIT_PREFIX`. The transaction is
    /// twenty-two fresh-read RMW edges plus one final volatile status/sync read.
    /// It stops immediately before the parent calls `hal_init_tx_pwr`.
    pub fn initialize_mac_he_prefix(&mut self, shared: &mut SharedRadioRegisters) {
        let init = &self.peripherals.wifi_mac.wifi_mac_he_init_prefix;
        let options = self
            .peripherals
            .wifi_mac
            .wifi_mac_he_init_suffix
            .he_default_control();

        options.modify(|_, w| w.ul_mu_disable().clear_bit());
        options.modify(|_, w| {
            w.ul_mu_disable()
                .clear_bit()
                .ul_mu_data_disable()
                .clear_bit()
        });

        let timing = init.bf_timing_control();
        timing.modify(|_, w| w.non_tb_beam_ru_select().clear_bit());
        timing.modify(|_, w| w.bit_23_opaque().clear_bit());
        timing.modify(|_, w| w.he_beam_hw_sequence_enable().set_bit());
        timing.modify(|_, w| w.he_beam_ndp_time().set(0x71));
        timing.modify(|_, w| {
            w.bf_memory_write_enable()
                .clear_bit()
                .rx_bfrp_timeout_ms()
                .set(0x10)
        });
        timing.modify(|_, w| w.he_beam_hw_sequence_select().set(5));
        timing.modify(|_, w| w.bit_25_opaque().set_bit());

        init.bf_enable().modify(|_, w| w.bit_31_opaque().set_bit());
        init.bf_vector_control()
            .modify(|_, w| w.bits_12_23_opaque().set(0x801));
        init.bf_high_image()
            .modify(|_, w| w.bits_20_31_opaque().set(0x690));
        init.bf_mode_control()
            .modify(|_, w| w.high_selector_class().clear_bit());
        init.bf_mode_control()
            .modify(|_, w| w.high_selector_parity().set_bit());

        // Complete hal_he_set_bf_report_rate(1, 0x10, 0, 0) derives signal
        // mode one, normalized rate zero, DCM false and ER-SU false, then
        // publishes the 16-QAM, QPSK and BPSK profiles in that order.
        let report = init.bf_report_rate();
        report.modify(|_, w| {
            w.qam16_rate()
                .set(0)
                .qam16_signal_mode()
                .set(1)
                .qam16_dcm()
                .clear_bit()
                .qam16_ersu()
                .clear_bit()
        });
        report.modify(|_, w| {
            w.qpsk_rate()
                .set(0)
                .qpsk_signal_mode()
                .set(1)
                .qpsk_dcm()
                .clear_bit()
                .qpsk_ersu()
                .clear_bit()
        });
        report.modify(|_, w| {
            w.bpsk_rate()
                .set(0)
                .bpsk_signal_mode()
                .set(1)
                .bpsk_dcm()
                .clear_bit()
                .bpsk_ersu()
                .clear_bit()
        });

        // The complete leaf performs this read after all BF writes even though
        // its value is unused. Preserve it because MMIO reads may acknowledge
        // or synchronize hardware state.
        let _ = shared
            .radio_phy
            .peripherals
            .phy_agc_oracle
            .read_agc_init_high_control()
            .bits_24_31_opaque();

        self.peripherals
            .wifi_mac
            .wifi_mac_rx_bssid_list
            .control()
            .modify(|_, w| w.rx_control_9_bssid_position().set(2));
        init.rx_field_control()
            .modify(|_, w| w.timeout_seconds().set(0x3c));
        init.parent_enable()
            .modify(|_, w| w.bit_12_opaque().set_bit());
        init.tb_tx_control().modify(|_, w| w.clear_15().clear_bit());
        init.tb_tx_control().modify(|_, w| w.clear_14().clear_bit());
    }
}
