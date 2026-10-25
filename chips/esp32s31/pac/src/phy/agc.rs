//! Ownership-bound AGC control leaves.

#![forbid(unsafe_code)]

use crate::generated::{PhyAgcParameterByte, PhyRxGainTableLastIndex};
use crate::{PhyFtmEnableVendorArgument, RadioPhyRegisters};

impl RadioPhyRegisters {
    /// Apply all fourteen internal MMIO edges of `phy_bb_agc_reg_update`.
    pub fn update_agc_baseband_registers(&mut self) {
        let agc = &mut self.peripherals.phy_agc_oracle;
        agc.initialize_agc_update_8070();
        agc.initialize_agc_update_78a4();
        agc.clear_agc_baseband_update_mode();
        agc.initialize_agc_update_8010();
        agc.initialize_agc_update_8018();
        agc.initialize_agc_update_801c();
        agc.initialize_agc_update_8020();
        agc.initialize_agc_update_8028();
        agc.initialize_agc_update_802c();
        agc.complete_agc_baseband_update_mode();
        agc.initialize_rx_11b_path_control_0();
        agc.initialize_agc_update_7048();
        agc.initialize_rx_11b_window_control();
        agc.initialize_rx_11b_path_control_1();
    }

    /// Select the complete rev0 ROM AGC enable or disable sequence.
    pub fn set_agc_enabled(&mut self, enabled: bool) {
        let agc = &mut self.peripherals.phy_agc_oracle;
        if !enabled {
            agc.disable_phy_agc();
            return;
        }

        agc.enable_phy_agc();
        agc.raise_phy_agc_enable_pulse();
        agc.lower_phy_agc_enable_pulse();
    }

    /// Publish both complete pinned RX-compensation fields in order.
    pub fn configure_rx_compensation(&mut self) {
        let agc = &mut self.peripherals.phy_agc_oracle;
        agc.configure_phy_rx_compensation_low();
        agc.configure_phy_rx_compensation_high();
    }

    /// Pulse the generated DC-memory clear field through two fresh RMWs.
    pub fn clear_agc_dc_memory(&mut self) {
        let agc = &mut self.peripherals.phy_agc_oracle;
        agc.raise_phy_agc_dc_memory_clear_pulse();
        agc.lower_phy_agc_dc_memory_clear_pulse();
    }

    /// Apply the two MMIO edges after the PBus work-mode 1 µs delay.
    pub fn configure_pbus_work_mode_pulse(&mut self) {
        let agc = &mut self.peripherals.phy_agc_oracle;
        agc.configure_phy_pbus_work_mode_control();
        agc.raise_phy_pbus_work_mode_pulse();
    }

    /// Clear the PBus work-mode pulse after the caller-owned 2 µs delay.
    pub fn clear_pbus_work_mode_pulse(&mut self) {
        self.peripherals
            .phy_agc_oracle
            .lower_phy_pbus_work_mode_pulse();
    }

    /// Apply all ten fresh-read updates of complete `phy_agc_reg_init`.
    pub fn initialize_agc_registers(&mut self, parameter_121: u8, parameter_120: u8) {
        let agc = &mut self.peripherals.phy_agc_oracle;
        let parameter_121 = PhyAgcParameterByte::new(u32::from(parameter_121))
            .expect("every u8 is a complete PHY AGC parameter byte");
        let parameter_120 = PhyAgcParameterByte::new(u32::from(parameter_120))
            .expect("every u8 is a complete PHY AGC parameter byte");
        agc.configure_phy_agc_initial_rx_gain_limit(parameter_121);
        agc.configure_phy_agc_initial_gain_limit_low(parameter_121);
        agc.configure_phy_agc_initial_rx_gain_index(parameter_121);
        agc.configure_phy_agc_initial_saturation_low();
        agc.configure_phy_agc_parameter_121(parameter_121);
        agc.configure_phy_agc_parameter_120_offset(parameter_120);
        agc.configure_phy_agc_initial_control_high();
        agc.raise_phy_agc_initial_pulse();
        agc.lower_phy_agc_initial_pulse();
        agc.configure_phy_agc_initial_high();
    }

    /// Apply all three fresh-read antenna initialization updates.
    pub fn configure_agc_antenna(&mut self) {
        let agc = &mut self.peripherals.phy_agc_oracle;
        agc.clear_phy_agc_initial_antenna_fields();
        agc.configure_phy_agc_initial_antenna_control();
        agc.configure_phy_agc_initial_antenna_paths();
    }

    /// Apply either complete branch of rev0 ROM `phy_rfrx_sat_rst`.
    pub fn configure_rf_rx_saturation(&mut self, enabled: bool) {
        let agc = &mut self.peripherals.phy_agc_oracle;
        agc.initialize_rf_rx_saturation_config();
        if enabled {
            agc.enable_phy_rf_rx_saturation_state();
            agc.enable_phy_rf_rx_saturation_low();
        } else {
            agc.disable_phy_rf_rx_saturation_state();
            agc.disable_phy_rf_rx_saturation_low();
        }
    }

    /// Publish both final RX-gain limits through separate fresh RMWs.
    pub fn configure_rx_gain_limits(&mut self, wifi_last_index: u8) {
        let agc = &mut self.peripherals.phy_agc_oracle;
        let wifi_last_index = PhyRxGainTableLastIndex::new(u32::from(wifi_last_index))
            .expect("every u8 is a complete RX-gain-table final index");
        agc.configure_phy_rx_gain_table_final_index(wifi_last_index);
        agc.configure_phy_rx_gain_table_final_limit(wifi_last_index);
    }

    /// Publish one saturation-gain word to both recovered destinations.
    pub fn set_agc_saturation_gain(&mut self, value: u32) {
        let agc = &mut self.peripherals.phy_agc_oracle;
        agc.agc_saturation_gain_low(crate::generated::AgcSaturationGainLow::new(value));
        agc.agc_saturation_gain_high(crate::generated::AgcSaturationGainHigh::new(value));
    }

    /// Select the complete pinned `phy_set_ftm_en` one-bit image.
    pub fn set_ftm_enabled(&mut self, enabled: bool) {
        self.set_ftm_enabled_from_vendor_argument(PhyFtmEnableVendorArgument::new(u32::from(
            enabled,
        )));
    }

    /// Apply the complete vendor ABI projection through generated PAC masks.
    pub fn set_ftm_enabled_from_vendor_argument(&mut self, input: PhyFtmEnableVendorArgument) {
        self.peripherals
            .phy_agc_oracle
            .set_ftm_enabled_from_vendor_argument(input);
    }

    /// Read back the single field owned by `phy_set_ftm_en`.
    pub fn ftm_enabled(&self) -> bool {
        self.peripherals.phy_agc_oracle.read_ftm_control().enable()
    }

    /// Apply `phy_reg_update_new` while preserving the independently owned FTM mode.
    pub fn update_agc_post_initialization(&mut self) {
        let agc = &mut self.peripherals.phy_agc_oracle;
        agc.set_phy_agc_post_initialization_flag();
        self.set_agc_saturation_gain(0x0818_212d);
        let agc = &mut self.peripherals.phy_agc_oracle;
        agc.configure_phy_agc_post_initialization_window();
        agc.configure_phy_agc_post_initialization_low();
        agc.configure_phy_agc_post_initialization_high();
    }

    /// Apply either complete branch of rev0 ROM `phy_rx_11b_opt`.
    pub fn configure_rx_11b_optimization(&mut self, enabled: bool) {
        let agc = &mut self.peripherals.phy_agc_oracle;
        if enabled {
            agc.configure_enabled_rx_11b_path_0_high();
            agc.configure_enabled_rx_11b_path_0_low();
            agc.configure_enabled_rx_11b_path_1_high();
            agc.configure_enabled_rx_11b_path_1_low();
            agc.configure_enabled_rx_11b_mode();
        } else {
            agc.configure_disabled_rx_11b_path_0_high();
            agc.configure_disabled_rx_11b_path_0_low();
            agc.configure_disabled_rx_11b_path_1_high();
            agc.configure_disabled_rx_11b_path_1_low();
            agc.configure_disabled_rx_11b_mode();
        }
        agc.configure_rx_11b_optimization_window();
    }
}

pub(crate) mod runtime;
