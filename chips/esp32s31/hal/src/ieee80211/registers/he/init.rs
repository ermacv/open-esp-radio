//! The bounded prefix of complete `hal_he_init`, through `hal_init_tb_tx`.

use oer_esp32s31_pac::wifi_bb::PhyAgcOracleWifiBb;
use oer_mmio::{Bits, Placement};

use crate::ieee80211::registers::{MacPlacement, WifiRadioRegisters};

impl<M: MacPlacement> WifiRadioRegisters<M> {
    /// Apply the complete prefix through `hal_init_tb_tx`.
    ///
    /// SOURCE: complete pinned `libpp.a[hal_mac_ctl.o]` parent and
    /// leaves recorded as `BLOB_LIBPP_HAL_HE_INIT_PREFIX`. The transaction is
    /// twenty-two fresh-read RMW edges plus one final volatile status/sync read
    /// of the shared PHY's AGC word, which `agc` lends.
    /// It stops immediately before the parent calls `hal_init_tx_pwr`.
    pub fn initialize_mac_he_prefix<A: Placement>(&mut self, agc: &PhyAgcOracleWifiBb<A>) {
        let options = &mut self.mac.wifi_mac_he_init_suffix;
        options.modify_he_default_control(|w| {
            w.set_ul_mu_disable(false);
        });
        options.modify_he_default_control(|w| {
            w.set_ul_mu_disable(false).set_ul_mu_data_disable(false);
        });

        let init = &mut self.mac.wifi_mac_he_init_prefix;
        init.modify_bf_timing_control(|w| {
            w.set_non_tb_beam_ru_select(false);
        });
        init.modify_bf_timing_control(|w| {
            w.set_bit_23_opaque(false);
        });
        init.modify_bf_timing_control(|w| {
            w.set_he_beam_hw_sequence_enable(true);
        });
        init.modify_bf_timing_control(|w| {
            w.set_he_beam_ndp_time(0x71);
        });
        init.modify_bf_timing_control(|w| {
            w.set_bf_memory_write_enable(false)
                .set_rx_bfrp_timeout_ms(Bits::constant::<0x10>());
        });
        init.modify_bf_timing_control(|w| {
            w.set_he_beam_hw_sequence_select(Bits::constant::<5>());
        });
        init.modify_bf_timing_control(|w| {
            w.set_bit_25_opaque(true);
        });

        init.modify_bf_enable(|w| {
            w.set_bit_31_opaque(true);
        });
        init.modify_bf_vector_control(|w| {
            w.set_bits_12_23_opaque(Bits::constant::<0x801>());
        });
        init.modify_bf_high_image(|w| {
            w.set_bits_20_31_opaque(Bits::constant::<0x690>());
        });
        init.modify_bf_mode_control(|w| {
            w.set_high_selector_class(false);
        });
        init.modify_bf_mode_control(|w| {
            w.set_high_selector_parity(true);
        });

        // Complete hal_he_set_bf_report_rate(1, 0x10, 0, 0) derives signal
        // mode one, normalized rate zero, DCM false and ER-SU false, then
        // publishes the 16-QAM, QPSK and BPSK profiles in that order.
        init.modify_bf_report_rate(|w| {
            w.set_qam16_rate(Bits::truncate(
                oer_esp32s31_pac::wifi_mac::bf_report_rate::Qam16Rate::NormalizedRate0.bits(),
            ))
            .set_qam16_signal_mode(Bits::truncate(
                oer_esp32s31_pac::wifi_mac::bf_report_rate::Qam16SignalMode::Mode1.bits(),
            ))
            .set_qam16_dcm(false)
            .set_qam16_ersu(false);
        });
        init.modify_bf_report_rate(|w| {
            w.set_qpsk_rate(Bits::truncate(
                oer_esp32s31_pac::wifi_mac::bf_report_rate::QpskRate::NormalizedRate0.bits(),
            ))
            .set_qpsk_signal_mode(Bits::truncate(
                oer_esp32s31_pac::wifi_mac::bf_report_rate::QpskSignalMode::Mode1.bits(),
            ))
            .set_qpsk_dcm(false)
            .set_qpsk_ersu(false);
        });
        init.modify_bf_report_rate(|w| {
            w.set_bpsk_rate(Bits::truncate(
                oer_esp32s31_pac::wifi_mac::bf_report_rate::BpskRate::NormalizedRate0.bits(),
            ))
            .set_bpsk_signal_mode(Bits::truncate(
                oer_esp32s31_pac::wifi_mac::bf_report_rate::BpskSignalMode::Mode1.bits(),
            ))
            .set_bpsk_dcm(false)
            .set_bpsk_ersu(false);
        });

        // The complete leaf performs this read after all BF writes even though
        // its value is unused. Preserve it because MMIO reads may acknowledge
        // or synchronize hardware state.
        let _ = agc.read_agc_init_high_control().bits_24_31_opaque();

        self.mac
            .wifi_mac_rx_bssid_list
            .modify_rx_bssid_list_control(|w| {
                w.set_rx_control_9_bssid_position(Bits::constant::<2>());
            });
        let init = &mut self.mac.wifi_mac_he_init_prefix;
        init.modify_rx_field_control(|w| {
            w.set_timeout_seconds(Bits::constant::<0x3c>());
        });
        init.modify_parent_enable(|w| {
            w.set_bit_12_opaque(true);
        });
        init.modify_tb_tx_control(|w| {
            w.set_clear_15(false);
        });
        init.modify_tb_tx_control(|w| {
            w.set_clear_14(false);
        });
    }
}

#[cfg(test)]
mod tests;
