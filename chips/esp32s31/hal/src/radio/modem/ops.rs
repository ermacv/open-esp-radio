//! Register-local modem operations over the physical generated parts.
#![forbid(unsafe_code)]
use super::values::*;
use oer_chips_esp32s31_pac::Bits;
use oer_chips_esp32s31_pac::dictionaries::{IcgState, Reset};
use oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate;
use oer_portable_mmio::Placement;

// SOURCE: ESP_RADIO_7F914782_S31_WIFI_CLOCKS, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_wifi_baseband_and_mac_reset<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: Reset,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_wifibb(value);
        w.set_rst_wifimac(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_wifi_baseband_reset<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: Reset,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_wifibb(value);
    });
}

// SOURCE: ROM_REV0_PHY_OPEN_FE_BB_CLK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_phy_calibration_clocks<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_wifibb_22m_en(ClockGate::Enabled);
        w.set_clk_wifibb_40m_en(ClockGate::Enabled);
        w.set_clk_wifibb_44m_en(ClockGate::Enabled);
        w.set_clk_wifibb_80m_en(ClockGate::Enabled);
        w.set_clk_wifibb_40x_en(ClockGate::Enabled);
        w.set_clk_wifibb_80x_en(ClockGate::Enabled);
        w.set_clk_wifibb_40x1_en(ClockGate::Enabled);
        w.set_clk_wifibb_80x1_en(ClockGate::Enabled);
        w.set_clk_wifibb_160x1_en(ClockGate::Enabled);
        w.set_clk_wifi_apb_en(ClockGate::Enabled);
        w.set_clk_fe_80m_en(ClockGate::Enabled);
        w.set_clk_fe_160m_en(ClockGate::Enabled);
        w.set_clk_fe_apb_en(ClockGate::Enabled);
        w.set_clk_bt_apb_en(ClockGate::Enabled);
        w.set_clk_btbb_en(ClockGate::Enabled);
        w.set_clk_fe_pwdet_adc_en(ClockGate::Enabled);
        w.set_clk_fe_adc_en(ClockGate::Enabled);
        w.set_clk_fe_dac_en(ClockGate::Enabled);
    });
}

// SOURCE: ROM_REV0_PHY_I2C, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn select_phy_i2c_160mhz_source<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
) {
    registers.modify_clk_conf(|w| {
        w.set_clk_i2c_mst_sel_160m(
            oer_chips_esp32s31_pac::modem_syscon::clk_conf::ClkI2cMstSel160m::I2cMst160m,
        );
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, ROM_REV0_PHY_I2C, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn restore_phy_i2c_160mhz_source<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: oer_chips_esp32s31_pac::modem_syscon::clk_conf::ClkI2cMstSel160m,
) {
    registers.modify_clk_conf(|w| {
        w.set_clk_i2c_mst_sel_160m(value);
    });
}

// SOURCE: ESP_RADIO_7F914782_S31_WIFI_CLOCKS, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_wifi_mac_clocks<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_wifibb_22m_en(ClockGate::Enabled);
        w.set_clk_wifibb_40m_en(ClockGate::Enabled);
        w.set_clk_wifibb_44m_en(ClockGate::Enabled);
        w.set_clk_wifibb_80m_en(ClockGate::Enabled);
        w.set_clk_wifibb_40x_en(ClockGate::Enabled);
        w.set_clk_wifibb_80x_en(ClockGate::Enabled);
        w.set_clk_wifibb_40x1_en(ClockGate::Enabled);
        w.set_clk_wifibb_80x1_en(ClockGate::Enabled);
        w.set_clk_wifibb_160x1_en(ClockGate::Enabled);
        w.set_clk_wifimac_en(ClockGate::Enabled);
        w.set_clk_wifi_apb_en(ClockGate::Enabled);
    });
}

// SOURCE: ESP_RADIO_7F914782_S31_WIFI_CLOCKS, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_wifi_mac_reset<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: Reset,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_wifimac(value);
    });
}

// SOURCE: S31_MODEM_SYSCON_STRUCT, S31_ESP_PACS_SVD.
pub(crate) fn clear_cold_start_wifi_control<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_wifi_bb_cfg(|w| {
        w.set_bit_0_opaque(false);
        w.set_wifi_enable(oer_chips_esp32s31_pac::modem_syscon::wifi_bb_cfg::WifiEnable::Disabled);
    });
}

// SOURCE: S31_MODEM_SYSCON_STRUCT, S31_ESP_PACS_SVD.
pub(crate) fn set_wifi_baseband_enable<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: oer_chips_esp32s31_pac::modem_syscon::wifi_bb_cfg::WifiEnable,
) {
    registers.modify_wifi_bb_cfg(|w| {
        w.set_wifi_enable(value);
    });
}

// SOURCE: S31_MODEM_SYSCON_STRUCT, S31_ESP_PACS_SVD, ROM_REV0_RADIO_ACCESSOR_FIELDS.
#[cfg(any(test, target_arch = "riscv32"))]
pub(crate) fn set_wifi_baseband_40mhz_state<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: oer_chips_esp32s31_pac::modem_syscon::wifi_bb_cfg::BssCbw40Digital,
) {
    registers.modify_wifi_bb_cfg(|w| {
        w.set_bss_cbw_40_digital(value);
        // Both states clear the adjacent bit.
        w.set_bit_3_opaque(false);
    });
}

// SOURCE: S31_MODEM_SYSCON_STRUCT, S31_ESP_PACS_SVD.
pub(crate) fn set_wifi_baseband_agc_update_mode<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: oer_chips_esp32s31_pac::modem_syscon::wifi_bb_cfg::Bits11_13Opaque,
) {
    registers.modify_wifi_bb_cfg(|w| {
        w.set_bits_11_13_opaque(value);
    });
}

// SOURCE: S31_MODEM_SYSCON_STRUCT, S31_ESP_PACS_SVD.
pub(crate) fn set_mac_baseband_enable<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: oer_chips_esp32s31_pac::modem_syscon::wifi_bb_cfg::Bit28Opaque,
) {
    registers.modify_wifi_bb_cfg(|w| {
        w.set_bit_28_opaque(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn prepare_modem_syscon_clock_map<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
) {
    registers.modify_clk_conf_power_st(|w| {
        w.set_clk_zb_st_map(w.clk_zb_st_map().with(IcgState::NogatingActive));
        w.set_clk_fe_st_map(
            w.clk_fe_st_map()
                .with(IcgState::NogatingModem)
                .with(IcgState::NogatingActive),
        );
        w.set_clk_bt_st_map(w.clk_bt_st_map().with(IcgState::NogatingActive));
        w.set_clk_wifi_st_map(
            w.clk_wifi_st_map()
                .with(IcgState::NogatingModem)
                .with(IcgState::NogatingActive),
        );
        w.set_clk_modem_peri_st_map(w.clk_modem_peri_st_map().with(IcgState::NogatingActive));
        w.set_clk_modem_apb_st_map(
            w.clk_modem_apb_st_map()
                .with(IcgState::NogatingModem)
                .with(IcgState::NogatingActive),
        );
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_modem_wifi_mac_clock<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: ClockGate,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_wifimac_en(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_modem_wifi_baseband_clocks<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: ClockGate,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_wifibb_22m_en(value);
        w.set_clk_wifibb_40m_en(value);
        w.set_clk_wifibb_80m_en(value);
        w.set_clk_wifibb_40x_en(value);
        w.set_clk_wifibb_80x_en(value);
        w.set_clk_wifibb_40x1_en(value);
        w.set_clk_wifibb_160x1_en(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_modem_wifi_apb_clock<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: ClockGate,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_wifi_apb_en(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_modem_wifi_baseband_44m_clock<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: ClockGate,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_wifibb_44m_en(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_modem_ieee802154_apb_clock<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: ClockGate,
) {
    registers.modify_clk_conf(|w| {
        w.set_clk_zb_apb_en(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_modem_ieee802154_mac_clock<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: ClockGate,
) {
    registers.modify_clk_conf(|w| {
        w.set_clk_zbmac_en(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_modem_frontend_apb_clock<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_fe_apb_en(ClockGate::Enabled);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_modem_frontend_80m_clock<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_fe_80m_en(ClockGate::Enabled);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_modem_frontend_160m_clock<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_fe_160m_en(ClockGate::Enabled);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_modem_frontend_dac_clock<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_fe_dac_en(ClockGate::Enabled);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_modem_frontend_pwdet_clock<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_fe_pwdet_adc_en(ClockGate::Enabled);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_modem_frontend_adc_clock<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_fe_adc_en(ClockGate::Enabled);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_wifi_baseband_80x1_clock<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: ClockGate,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_wifibb_80x1_en(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_etm_clock<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: ClockGate,
) {
    registers.modify_clk_conf(|w| {
        w.set_clk_etm_en(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_mac_clock<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: ClockGate,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_btmac_en(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_peripheral_clocks<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: ClockGate,
) {
    registers.modify_clk_conf(|w| {
        w.set_clk_modem_sec_ecb_en(value);
        w.set_clk_modem_sec_ccm_en(value);
        w.set_clk_modem_sec_bah_en(value);
        w.set_clk_modem_sec_en(value);
        w.set_clk_ble_timer_en(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_apb_clock<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: ClockGate,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_bt_apb_en(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_modem_security_apb_clock<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: ClockGate,
) {
    registers.modify_clk_conf(|w| {
        w.set_clk_modem_sec_apb_en(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_baseband_clock<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: ClockGate,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_btbb_en(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_mac_reset<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: Reset,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_btmac(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_mac_apb_reset<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: Reset,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_btmac_apb(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_timer_reset<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: Reset,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_ble_timer(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_modem_ecb_reset<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: Reset,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_modem_ecb(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_modem_ccm_reset<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: Reset,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_modem_ccm(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_modem_bah_reset<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: Reset,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_modem_bah(value);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_modem_security_reset<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    value: Reset,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_modem_sec(value);
    });
}

// SOURCE: S31_ESP_PACS_SVD.
pub(crate) fn enable_modem_register_bus_clock<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::hp_sys_clkrst::HpSysClkrstRadio<P>,
) {
    registers.modify_modem_ctrl0(|w| {
        w.set_reg_modem_clk_en(
            oer_chips_esp32s31_pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn::Enabled,
        );
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_ESP_PACS_SVD.
pub(crate) fn restore_modem_register_bus_clock<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::hp_sys_clkrst::HpSysClkrstRadio<P>,
    value: oer_chips_esp32s31_pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn,
) {
    registers.modify_modem_ctrl0(|w| {
        w.set_reg_modem_clk_en(value);
    });
}

// SOURCE: ESP_IDF_4D59230D_S31_MODEM_PLL_GATE.
pub(crate) fn configure_modem_source_clocks<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::hp_sys_clkrst::HpSysClkrstRadio<P>,
) {
    super::clock_device::open_pll_source_gate(&mut super::clock_device::PllSourceGate(registers));
}

// SOURCE: ESP_HAL_63E97BE_S31_MODEM_CLOCK_MAP, S31_PMU_HEADERS.
pub(crate) fn apply_modem_icg_selection<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::pmu::PmuRadio<P>,
) {
    registers.trigger_imm_modem_icg_update_dig_icg_modem_en();
}

// SOURCE: ESP_HAL_63E97BE_S31_MODEM_CLOCK_MAP, S31_PMU_HEADERS.
pub(crate) fn apply_sleep_icg_selection<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::pmu::PmuRadio<P>,
) {
    registers.trigger_imm_sleep_sysclk_update_dig_icg_switch();
}

// SOURCE: S31_PMU_HEADERS.
pub(crate) fn power_on_rf_circuits<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::pmu::PmuRadio<P>,
) {
    registers.modify_rf_pwc(|w| {
        w.set_xpd_rf_circuit(oer_chips_esp32s31_pac::pmu::rf_pwc::XpdRfCircuit::PoweredOn);
    });
}

// SOURCE: S31_PMU_HEADERS.
pub(crate) fn power_off_rf_circuits<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::pmu::PmuRadio<P>,
) {
    registers.modify_rf_pwc(|w| {
        w.set_xpd_rf_circuit(oer_chips_esp32s31_pac::pmu::rf_pwc::XpdRfCircuit::PoweredOff);
    });
}

/// Tie the baseband analog-I2C power high, or release the tie.
///
/// Releasing writes nothing. The header classes `TIE_HIGH_XPD_BB_I2C`
/// (bit 28 of `IMM_HP_CK_POWER_0`) write-trigger, so no write clears it and
/// a write of zero starts no action; the vendor's read-modify-write
/// clearing bit 28 has no counterpart in that access class, and what a
/// read of the bit returns is not established.
///
/// SOURCE: BLOB_LIBPHY_PHY_OPEN_I2C_XPD_NEW, S31_PMU_HEADERS.
pub(crate) fn tie_baseband_i2c_power<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::pmu::PmuRadio<P>,
) {
    registers.trigger_imm_hp_ck_power_0_tie_high_xpd_bb_i2c();
}

// SOURCE: BLOB_LIBPHY_PHY_OPEN_I2C_XPD_NEW, S31_PMU_HEADERS.
pub(crate) fn power_on_analog_i2c<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::pmu::PmuRadio<P>,
) {
    registers.modify_ana_peri_pwr_ctrl(|w| {
        w.set_xpd_perif_i2c(true);
    });
}

// SOURCE: BLOB_LIBPHY_PHY_OPEN_I2C_XPD_NEW, S31_PMU_HEADERS.
pub(crate) fn power_off_analog_i2c<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::pmu::PmuRadio<P>,
) {
    registers.modify_ana_peri_pwr_ctrl(|w| {
        w.set_xpd_perif_i2c(false);
    });
}

// SOURCE: BLOB_LIBPHY_PHY_OPEN_I2C_XPD_NEW, S31_PMU_HEADERS.
pub(crate) fn release_analog_i2c_reset<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::pmu::PmuRadio<P>,
) {
    registers.modify_ana_peri_pwr_ctrl(|w| {
        w.set_rstb_perif_i2c(true);
    });
}

// SOURCE: BLOB_LIBPHY_PHY_OPEN_I2C_XPD_NEW, S31_PMU_HEADERS.
pub(crate) fn assert_analog_i2c_reset<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::pmu::PmuRadio<P>,
) {
    registers.modify_ana_peri_pwr_ctrl(|w| {
        w.set_rstb_perif_i2c(false);
    });
}

// SOURCE: BLOB_ROM_S31_PMU_ACTIVE_CK_LOW4, S31_PMU_HEADERS.
pub(crate) fn enable_frontend_baseband_power<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::pmu::PmuRadio<P>,
) {
    registers.modify_hp_active_hp_ck_power(|w| {
        // SOURCE: BLOB_ROM_S31_PMU_ACTIVE_CK_LOW4; observed configuration image.
        w.set_bits_0_3_opaque(Bits::constant::<0xf>());
        w.set_hp_active_xpd_bb_i2c(true);
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_HAL_63E97BE_S31_MODEM_CLOCK_MAP.
pub(crate) fn initialize_shared_modem_power_state_map<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_lpcon::ModemLpconPower<P>,
) {
    registers.modify_clk_conf_power_st(|w| {
        w.set_clk_wifipwr_st_map(
            w.clk_wifipwr_st_map()
                .with(IcgState::NogatingModem)
                .with(IcgState::NogatingActive),
        );
        w.set_clk_coex_st_map(
            w.clk_coex_st_map()
                .with(IcgState::NogatingModem)
                .with(IcgState::NogatingActive),
        );
        w.set_clk_i2c_mst_st_map(
            w.clk_i2c_mst_st_map()
                .with(IcgState::NogatingModem)
                .with(IcgState::NogatingActive),
        );
        w.set_clk_lp_apb_st_map(
            w.clk_lp_apb_st_map()
                .with(IcgState::NogatingModem)
                .with(IcgState::NogatingActive),
        );
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_4D59230D_S31_WIFI_LOW_POWER_CLOCK.
pub(crate) fn deselect_wifi_low_power_clock_slow_oscillator<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_lpcon::ModemLpconPower<P>,
) {
    registers.modify_wifi_lp_clk_conf(|w| {
        w.set_clk_wifipwr_lp_sel_osc_slow(false);
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_4D59230D_S31_WIFI_LOW_POWER_CLOCK.
pub(crate) fn deselect_wifi_low_power_clock_fast_oscillator<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_lpcon::ModemLpconPower<P>,
) {
    registers.modify_wifi_lp_clk_conf(|w| {
        w.set_clk_wifipwr_lp_sel_osc_fast(false);
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_4D59230D_S31_WIFI_LOW_POWER_CLOCK.
pub(crate) fn deselect_wifi_low_power_clock_crystal_32khz<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_lpcon::ModemLpconPower<P>,
) {
    registers.modify_wifi_lp_clk_conf(|w| {
        w.set_clk_wifipwr_lp_sel_xtal32k(false);
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_4D59230D_S31_WIFI_LOW_POWER_CLOCK.
pub(crate) fn deselect_wifi_low_power_clock_crystal<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_lpcon::ModemLpconPower<P>,
) {
    registers.modify_wifi_lp_clk_conf(|w| {
        w.set_clk_wifipwr_lp_sel_xtal(false);
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_4D59230D_S31_WIFI_LOW_POWER_CLOCK.
pub(crate) fn select_wifi_low_power_clock_slow_oscillator<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_lpcon::ModemLpconPower<P>,
) {
    registers.modify_wifi_lp_clk_conf(|w| {
        w.set_clk_wifipwr_lp_sel_osc_slow(true);
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_4D59230D_S31_WIFI_LOW_POWER_CLOCK.
pub(crate) fn select_wifi_low_power_clock_crystal_32khz<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_lpcon::ModemLpconPower<P>,
) {
    registers.modify_wifi_lp_clk_conf(|w| {
        w.set_clk_wifipwr_lp_sel_xtal32k(true);
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_4D59230D_S31_WIFI_LOW_POWER_CLOCK.
pub(crate) fn select_modem_32khz_clock_crystal<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_lpcon::ModemLpconPower<P>,
) {
    registers.modify_modem_32k_clk_conf(|w| {
        w.set_clk_modem_32k_sel(Bits::constant::<0>());
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_4D59230D_S31_WIFI_LOW_POWER_CLOCK.
pub(crate) fn set_wifi_low_power_clock_divider<P: Placement>(
    registers: &mut oer_chips_esp32s31_pac::modem_lpcon::ModemLpconPower<P>,
    value: ModemLowPowerClockDivider,
) {
    registers.modify_wifi_lp_clk_conf(|w| {
        w.set_clk_wifipwr_lp_div_num(value.field());
    });
}

#[cfg(test)]
mod tests {
    use super::tie_baseband_i2c_power;
    use oer_chips_esp32s31_pac::{instances, pmu::PmuRadio};
    use std::vec;

    /// `IMM_HP_CK_POWER_0`, the word of the immediate clock ties.
    const IMM_HP_CK_POWER_0: usize = 0xf0;

    #[test]
    fn the_tie_stores_its_one_trigger() {
        for fill in [0, u32::MAX] {
            let mut memory = vec![fill; instances::pmu::SIZE.div_ceil(4)];
            let mut expected = memory.clone();
            tie_baseband_i2c_power(&mut PmuRadio::host(&mut memory).expect("PMU"));
            expected[IMM_HP_CK_POWER_0 / 4] = 1 << 28;
            assert_eq!(memory, expected, "the tie stores its one trigger");
        }
    }
}
