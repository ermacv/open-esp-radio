//! Ownership-bound AGC control leaves.

#![forbid(unsafe_code)]

use crate::generated::{PhyAgcParameterByte, PhyRxGainTableLastIndex};
use crate::{PhyFtmEnableVendorArgument, RadioPhyRegisters};

impl RadioPhyRegisters {
    /// Apply all fourteen internal MMIO edges of `phy_bb_agc_reg_update`.
    pub fn update_agc_baseband_registers(&mut self) {
        let agc = &self.peripherals.phy_agc_oracle;
        crate::svd::fixed_register_image::initialize_agc_update_8070(agc);
        crate::svd::fixed_register_image::initialize_agc_update_78a4(agc);
        crate::generated::clear_agc_baseband_update_mode(agc);
        crate::svd::fixed_register_image::initialize_agc_update_8010(agc);
        crate::svd::fixed_register_image::initialize_agc_update_8018(agc);
        crate::svd::fixed_register_image::initialize_agc_update_801c(agc);
        crate::svd::fixed_register_image::initialize_agc_update_8020(agc);
        crate::svd::fixed_register_image::initialize_agc_update_8028(agc);
        crate::svd::fixed_register_image::initialize_agc_update_802c(agc);
        crate::generated::complete_agc_baseband_update_mode(agc);
        crate::svd::fixed_register_image::initialize_rx_11b_path_control_0(agc);
        crate::svd::fixed_register_image::initialize_agc_update_7048(agc);
        crate::svd::fixed_register_image::initialize_rx_11b_window_control(agc);
        crate::svd::fixed_register_image::initialize_rx_11b_path_control_1(agc);
    }

    /// Select the complete rev0 ROM AGC enable or disable sequence.
    pub fn set_agc_enabled(&mut self, enabled: bool) {
        let agc = &self.peripherals.phy_agc_oracle;
        if !enabled {
            crate::generated::disable_phy_agc(agc);
            return;
        }

        crate::generated::enable_phy_agc(agc);
        crate::generated::raise_phy_agc_enable_pulse(agc);
        crate::generated::lower_phy_agc_enable_pulse(agc);
    }

    /// Publish both complete pinned RX-compensation fields in order.
    pub fn configure_rx_compensation(&mut self) {
        let agc = &self.peripherals.phy_agc_oracle;
        crate::generated::configure_phy_rx_compensation_low(agc);
        crate::generated::configure_phy_rx_compensation_high(agc);
    }

    /// Pulse the generated DC-memory clear field through two fresh RMWs.
    pub fn clear_agc_dc_memory(&mut self) {
        let agc = &self.peripherals.phy_agc_oracle;
        crate::generated::raise_phy_agc_dc_memory_clear_pulse(agc);
        crate::generated::lower_phy_agc_dc_memory_clear_pulse(agc);
    }

    /// Apply the two MMIO edges after the PBus work-mode 1 µs delay.
    pub fn configure_pbus_work_mode_pulse(&mut self) {
        let agc = &self.peripherals.phy_agc_oracle;
        crate::generated::configure_phy_pbus_work_mode_control(agc);
        crate::generated::raise_phy_pbus_work_mode_pulse(agc);
    }

    /// Clear the PBus work-mode pulse after the caller-owned 2 µs delay.
    pub fn clear_pbus_work_mode_pulse(&mut self) {
        crate::generated::lower_phy_pbus_work_mode_pulse(&self.peripherals.phy_agc_oracle);
    }

    /// Apply all ten fresh-read updates of complete `phy_agc_reg_init`.
    pub fn initialize_agc_registers(&mut self, parameter_121: u8, parameter_120: u8) {
        let agc = &self.peripherals.phy_agc_oracle;
        let parameter_121 = PhyAgcParameterByte::new(u32::from(parameter_121))
            .expect("every u8 is a complete PHY AGC parameter byte");
        let parameter_120 = PhyAgcParameterByte::new(u32::from(parameter_120))
            .expect("every u8 is a complete PHY AGC parameter byte");
        crate::generated::configure_phy_agc_initial_rx_gain_limit(agc, parameter_121);
        crate::generated::configure_phy_agc_initial_gain_limit_low(agc, parameter_121);
        crate::generated::configure_phy_agc_initial_rx_gain_index(agc, parameter_121);
        crate::generated::configure_phy_agc_initial_saturation_low(agc);
        crate::generated::configure_phy_agc_parameter_121(agc, parameter_121);
        crate::generated::configure_phy_agc_parameter_120_offset(agc, parameter_120);
        crate::generated::configure_phy_agc_initial_control_high(agc);
        crate::generated::raise_phy_agc_initial_pulse(agc);
        crate::generated::lower_phy_agc_initial_pulse(agc);
        crate::generated::configure_phy_agc_initial_high(agc);
    }

    /// Apply all three fresh-read antenna initialization updates.
    pub fn configure_agc_antenna(&mut self) {
        let agc = &self.peripherals.phy_agc_oracle;
        crate::generated::clear_phy_agc_initial_antenna_fields(agc);
        crate::generated::configure_phy_agc_initial_antenna_control(agc);
        crate::generated::configure_phy_agc_initial_antenna_paths(agc);
    }

    /// Apply either complete branch of rev0 ROM `phy_rfrx_sat_rst`.
    pub fn configure_rf_rx_saturation(&mut self, enabled: bool) {
        let agc = &self.peripherals.phy_agc_oracle;
        crate::svd::fixed_register_image::initialize_rf_rx_saturation_config(agc);
        if enabled {
            crate::generated::enable_phy_rf_rx_saturation_state(agc);
            crate::generated::enable_phy_rf_rx_saturation_low(agc);
        } else {
            crate::generated::disable_phy_rf_rx_saturation_state(agc);
            crate::generated::disable_phy_rf_rx_saturation_low(agc);
        }
    }

    /// Publish both final RX-gain limits through separate fresh RMWs.
    pub fn configure_rx_gain_limits(&mut self, wifi_last_index: u8) {
        let agc = &self.peripherals.phy_agc_oracle;
        let wifi_last_index = PhyRxGainTableLastIndex::new(u32::from(wifi_last_index))
            .expect("every u8 is a complete RX-gain-table final index");
        crate::generated::configure_phy_rx_gain_table_final_index(agc, wifi_last_index);
        crate::generated::configure_phy_rx_gain_table_final_limit(agc, wifi_last_index);
    }

    /// Publish one saturation-gain word to both recovered destinations.
    pub fn set_agc_saturation_gain(&mut self, value: u32) {
        let agc = &self.peripherals.phy_agc_oracle;
        crate::generated::agc_saturation_gain_low(
            agc,
            crate::generated::AgcSaturationGainLow::new(value),
        );
        crate::generated::agc_saturation_gain_high(
            agc,
            crate::generated::AgcSaturationGainHigh::new(value),
        );
    }

    /// Select the complete pinned `phy_set_ftm_en` one-bit image.
    pub fn set_ftm_enabled(&mut self, enabled: bool) {
        self.set_ftm_enabled_from_vendor_argument(PhyFtmEnableVendorArgument::new(u32::from(
            enabled,
        )));
    }

    /// Apply the complete vendor ABI projection through generated PAC masks.
    pub fn set_ftm_enabled_from_vendor_argument(&mut self, input: PhyFtmEnableVendorArgument) {
        crate::generated::set_ftm_enabled_from_vendor_argument(
            &self.peripherals.phy_agc_oracle,
            input,
        );
    }

    /// Read back the single field owned by `phy_set_ftm_en`.
    pub fn ftm_enabled(&self) -> bool {
        crate::svd::field_read::observe_phy_ftm_enabled(&self.peripherals.phy_agc_oracle)
    }

    /// Apply `phy_reg_update_new` while preserving the independently owned FTM mode.
    pub fn update_agc_post_initialization(&mut self) {
        let agc = &self.peripherals.phy_agc_oracle;
        crate::generated::set_phy_agc_post_initialization_flag(agc);
        self.set_agc_saturation_gain(0x0818_212d);
        let agc = &self.peripherals.phy_agc_oracle;
        crate::generated::configure_phy_agc_post_initialization_window(agc);
        crate::generated::configure_phy_agc_post_initialization_low(agc);
        crate::generated::configure_phy_agc_post_initialization_high(agc);
    }

    /// Apply either complete branch of rev0 ROM `phy_rx_11b_opt`.
    pub fn configure_rx_11b_optimization(&mut self, enabled: bool) {
        let agc = &self.peripherals.phy_agc_oracle;
        if enabled {
            crate::generated::configure_enabled_rx_11b_path_0_high(agc);
            crate::generated::configure_enabled_rx_11b_path_0_low(agc);
            crate::generated::configure_enabled_rx_11b_path_1_high(agc);
            crate::generated::configure_enabled_rx_11b_path_1_low(agc);
            crate::generated::configure_enabled_rx_11b_mode(agc);
        } else {
            crate::generated::configure_disabled_rx_11b_path_0_high(agc);
            crate::generated::configure_disabled_rx_11b_path_0_low(agc);
            crate::generated::configure_disabled_rx_11b_path_1_high(agc);
            crate::generated::configure_disabled_rx_11b_path_1_low(agc);
            crate::generated::configure_disabled_rx_11b_mode(agc);
        }
        crate::generated::configure_rx_11b_optimization_window(agc);
    }
}

pub(crate) mod runtime;
