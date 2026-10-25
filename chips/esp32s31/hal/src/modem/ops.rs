//! Register-local modem operations over the physical generated parts.
#![forbid(unsafe_code)]
use super::values::*;
use oer_mmio::{Bits, Placement};

// SOURCE: ESP_RADIO_7F914782_S31_WIFI_CLOCKS, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_wifi_baseband_and_mac_reset<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconResetState,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_wifibb(match value {
            ModemSysconResetState::Released => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstWifibb::Released
            }
            ModemSysconResetState::Asserted => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstWifibb::Asserted
            }
        });
        w.set_rst_wifimac(match value {
            ModemSysconResetState::Released => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstWifimac::Released
            }
            ModemSysconResetState::Asserted => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstWifimac::Asserted
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_wifi_baseband_reset<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconResetState,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_wifibb(match value {
            ModemSysconResetState::Released => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstWifibb::Released
            }
            ModemSysconResetState::Asserted => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstWifibb::Asserted
            }
        });
    });
}

// SOURCE: ROM_REV0_PHY_OPEN_FE_BB_CLK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_phy_calibration_clocks<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_wifibb_22m_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb22mEn::Enabled);
        w.set_clk_wifibb_40m_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb40mEn::Enabled);
        w.set_clk_wifibb_44m_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb44mEn::Enabled);
        w.set_clk_wifibb_80m_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb80mEn::Enabled);
        w.set_clk_wifibb_40x_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb40xEn::Enabled);
        w.set_clk_wifibb_80x_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb80xEn::Enabled);
        w.set_clk_wifibb_40x1_en(
            oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb40x1En::Enabled,
        );
        w.set_clk_wifibb_80x1_en(
            oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb80x1En::Enabled,
        );
        w.set_clk_wifibb_160x1_en(
            oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb160x1En::Enabled,
        );
        w.set_clk_wifi_apb_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifiApbEn::Enabled);
        w.set_clk_fe_80m_en(true);
        w.set_clk_fe_160m_en(true);
        w.set_clk_fe_apb_en(true);
        w.set_clk_bt_apb_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkBtApbEn::Enabled);
        w.set_clk_btbb_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkBtbbEn::Enabled);
        w.set_clk_fe_pwdet_adc_en(true);
        w.set_clk_fe_adc_en(true);
        w.set_clk_fe_dac_en(true);
    });
}

// SOURCE: ROM_REV0_PHY_I2C, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn select_phy_i2c_160mhz_source<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf(|w| {
        w.set_clk_i2c_mst_sel_160m(
            oer_esp32s31_pac::modem_syscon::clk_conf::ClkI2cMstSel160m::Enabled,
        );
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, ROM_REV0_PHY_I2C, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn restore_phy_i2c_160mhz_source<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconClockGateState,
) {
    registers.modify_clk_conf(|w| {
        w.set_clk_i2c_mst_sel_160m(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkI2cMstSel160m::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkI2cMstSel160m::Enabled
            }
        });
    });
}

// SOURCE: ESP_RADIO_7F914782_S31_WIFI_CLOCKS, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_wifi_mac_clocks<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_wifibb_22m_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb22mEn::Enabled);
        w.set_clk_wifibb_40m_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb40mEn::Enabled);
        w.set_clk_wifibb_44m_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb44mEn::Enabled);
        w.set_clk_wifibb_80m_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb80mEn::Enabled);
        w.set_clk_wifibb_40x_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb40xEn::Enabled);
        w.set_clk_wifibb_80x_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb80xEn::Enabled);
        w.set_clk_wifibb_40x1_en(
            oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb40x1En::Enabled,
        );
        w.set_clk_wifibb_80x1_en(
            oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb80x1En::Enabled,
        );
        w.set_clk_wifibb_160x1_en(
            oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb160x1En::Enabled,
        );
        w.set_clk_wifimac_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifimacEn::Enabled);
        w.set_clk_wifi_apb_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifiApbEn::Enabled);
    });
}

// SOURCE: ESP_RADIO_7F914782_S31_WIFI_CLOCKS, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_wifi_mac_reset<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconResetState,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_wifimac(match value {
            ModemSysconResetState::Released => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstWifimac::Released
            }
            ModemSysconResetState::Asserted => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstWifimac::Asserted
            }
        });
    });
}

// SOURCE: S31_MODEM_SYSCON_STRUCT, S31_ESP_PACS_SVD.
pub(crate) fn clear_cold_start_wifi_control<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_wifi_bb_cfg(|w| {
        w.set_bit_0_opaque(false);
        w.set_wifi_enable(oer_esp32s31_pac::modem_syscon::wifi_bb_cfg::WifiEnable::Disabled);
    });
}

// SOURCE: S31_MODEM_SYSCON_STRUCT, S31_ESP_PACS_SVD.
pub(crate) fn set_wifi_baseband_enable<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconEnableState,
) {
    registers.modify_wifi_bb_cfg(|w| {
        w.set_wifi_enable(match value {
            ModemSysconEnableState::Disabled => {
                oer_esp32s31_pac::modem_syscon::wifi_bb_cfg::WifiEnable::Disabled
            }
            ModemSysconEnableState::Enabled => {
                oer_esp32s31_pac::modem_syscon::wifi_bb_cfg::WifiEnable::Enabled
            }
        });
    });
}

// SOURCE: S31_MODEM_SYSCON_STRUCT, S31_ESP_PACS_SVD, ROM_REV0_RADIO_ACCESSOR_FIELDS.
pub(crate) fn set_wifi_baseband_40mhz_state<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: WifiBaseband40MhzState,
) {
    registers.modify_wifi_bb_cfg(|w| {
        w.set_bss_cbw_40_digital(match value {
            WifiBaseband40MhzState::Disabled => {
                oer_esp32s31_pac::modem_syscon::wifi_bb_cfg::BssCbw40Digital::Disabled
            }
            WifiBaseband40MhzState::Enabled => {
                oer_esp32s31_pac::modem_syscon::wifi_bb_cfg::BssCbw40Digital::Enabled
            }
        });
        w.set_bit_3_opaque(match value {
            WifiBaseband40MhzState::Disabled => false,
            WifiBaseband40MhzState::Enabled => false,
        });
    });
}

// SOURCE: S31_MODEM_SYSCON_STRUCT, S31_ESP_PACS_SVD.
pub(crate) fn set_wifi_baseband_agc_update_mode<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: WifiBasebandAgcUpdateMode,
) {
    registers.modify_wifi_bb_cfg(|w| {
        w.set_bits_11_13_opaque(match value {
            WifiBasebandAgcUpdateMode::Initialization => {
                oer_esp32s31_pac::modem_syscon::wifi_bb_cfg::Bits11_13Opaque::Initialization
            }
            WifiBasebandAgcUpdateMode::RegisterUpdatesEnabled => {
                oer_esp32s31_pac::modem_syscon::wifi_bb_cfg::Bits11_13Opaque::RegisterUpdatesEnabled
            }
        });
    });
}

// SOURCE: S31_MODEM_SYSCON_STRUCT, S31_ESP_PACS_SVD.
pub(crate) fn set_mac_baseband_enable<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconEnableState,
) {
    registers.modify_wifi_bb_cfg(|w| {
        w.set_bit_28_opaque(match value {
            ModemSysconEnableState::Disabled => {
                oer_esp32s31_pac::modem_syscon::wifi_bb_cfg::Bit28Opaque::Disabled
            }
            ModemSysconEnableState::Enabled => {
                oer_esp32s31_pac::modem_syscon::wifi_bb_cfg::Bit28Opaque::Enabled
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn prepare_ieee802154_modem_apb_clock_map<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf_power_st(|w| {
        w.set_clk_modem_apb_st_map(Bits::truncate(w.clk_modem_apb_st_map().get() | 6));
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn prepare_ieee802154_modem_peripheral_clock_map<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf_power_st(|w| {
        w.set_clk_modem_peri_st_map(Bits::truncate(w.clk_modem_peri_st_map().get() | 4));
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn prepare_ieee802154_wifi_clock_map<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf_power_st(|w| {
        w.set_clk_wifi_st_map(Bits::truncate(w.clk_wifi_st_map().get() | 6));
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn prepare_ieee802154_bluetooth_clock_map<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf_power_st(|w| {
        w.set_clk_bt_st_map(Bits::truncate(w.clk_bt_st_map().get() | 4));
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn prepare_ieee802154_frontend_clock_map<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf_power_st(|w| {
        w.set_clk_fe_st_map(Bits::truncate(w.clk_fe_st_map().get() | 6));
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn prepare_ieee802154_clock_map<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf_power_st(|w| {
        w.set_clk_zb_st_map(Bits::truncate(w.clk_zb_st_map().get() | 4));
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_ieee802154_wifi_baseband_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_wifibb_80x1_en(
            oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb80x1En::Enabled,
        );
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_ieee802154_etm_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf(|w| {
        w.set_clk_etm_en(oer_esp32s31_pac::modem_syscon::clk_conf::ClkEtmEn::Enabled);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_ieee802154_bluetooth_apb_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_bt_apb_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkBtApbEn::Enabled);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_ieee802154_modem_security_apb_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf(|w| {
        w.set_clk_modem_sec_apb_en(
            oer_esp32s31_pac::modem_syscon::clk_conf::ClkModemSecApbEn::Enabled,
        );
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_ieee802154_common_baseband_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_btbb_en(oer_esp32s31_pac::modem_syscon::clk_conf1::ClkBtbbEn::Enabled);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_ieee802154_apb_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf(|w| {
        w.set_clk_zb_apb_en(oer_esp32s31_pac::modem_syscon::clk_conf::ClkZbApbEn::Enabled);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_ieee802154_mac_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf(|w| {
        w.set_clk_zbmac_en(oer_esp32s31_pac::modem_syscon::clk_conf::ClkZbmacEn::Enabled);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_ieee802154_mac_reset<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconResetState,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_zbmac(match value {
            ModemSysconResetState::Released => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstZbmac::Released
            }
            ModemSysconResetState::Asserted => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstZbmac::Asserted
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_ieee802154_apb_reset<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconResetState,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_zbmac_apb(match value {
            ModemSysconResetState::Released => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstZbmacApb::Released
            }
            ModemSysconResetState::Asserted => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstZbmacApb::Asserted
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn prepare_modem_syscon_clock_map<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf_power_st(|w| {
        w.set_clk_zb_st_map(Bits::truncate(w.clk_zb_st_map().get() | 4));
        w.set_clk_fe_st_map(Bits::truncate(w.clk_fe_st_map().get() | 6));
        w.set_clk_bt_st_map(Bits::truncate(w.clk_bt_st_map().get() | 4));
        w.set_clk_wifi_st_map(Bits::truncate(w.clk_wifi_st_map().get() | 6));
        w.set_clk_modem_peri_st_map(Bits::truncate(w.clk_modem_peri_st_map().get() | 4));
        w.set_clk_modem_apb_st_map(Bits::truncate(w.clk_modem_apb_st_map().get() | 6));
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_modem_wifi_mac_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconClockGateState,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_wifimac_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifimacEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifimacEn::Enabled
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_modem_wifi_baseband_clocks<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconClockGateState,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_wifibb_22m_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb22mEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb22mEn::Enabled
            }
        });
        w.set_clk_wifibb_40m_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb40mEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb40mEn::Enabled
            }
        });
        w.set_clk_wifibb_80m_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb80mEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb80mEn::Enabled
            }
        });
        w.set_clk_wifibb_40x_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb40xEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb40xEn::Enabled
            }
        });
        w.set_clk_wifibb_80x_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb80xEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb80xEn::Enabled
            }
        });
        w.set_clk_wifibb_40x1_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb40x1En::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb40x1En::Enabled
            }
        });
        w.set_clk_wifibb_160x1_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb160x1En::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb160x1En::Enabled
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_modem_wifi_apb_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconClockGateState,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_wifi_apb_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifiApbEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifiApbEn::Enabled
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_modem_wifi_baseband_44m_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconClockGateState,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_wifibb_44m_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb44mEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb44mEn::Enabled
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_modem_ieee802154_apb_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconClockGateState,
) {
    registers.modify_clk_conf(|w| {
        w.set_clk_zb_apb_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkZbApbEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkZbApbEn::Enabled
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_modem_ieee802154_mac_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconClockGateState,
) {
    registers.modify_clk_conf(|w| {
        w.set_clk_zbmac_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkZbmacEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkZbmacEn::Enabled
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_modem_frontend_apb_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_fe_apb_en(true);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_modem_frontend_80m_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_fe_80m_en(true);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_modem_frontend_160m_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_fe_160m_en(true);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_modem_frontend_dac_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_fe_dac_en(true);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_modem_frontend_pwdet_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_fe_pwdet_adc_en(true);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn enable_modem_frontend_adc_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_fe_adc_en(true);
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_wifi_baseband_80x1_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconClockGateState,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_wifibb_80x1_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb80x1En::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkWifibb80x1En::Enabled
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_etm_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconClockGateState,
) {
    registers.modify_clk_conf(|w| {
        w.set_clk_etm_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkEtmEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkEtmEn::Enabled
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_mac_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconClockGateState,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_btmac_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkBtmacEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkBtmacEn::Enabled
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_peripheral_clocks<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconClockGateState,
) {
    registers.modify_clk_conf(|w| {
        w.set_clk_modem_sec_ecb_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkModemSecEcbEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkModemSecEcbEn::Enabled
            }
        });
        w.set_clk_modem_sec_ccm_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkModemSecCcmEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkModemSecCcmEn::Enabled
            }
        });
        w.set_clk_modem_sec_bah_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkModemSecBahEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkModemSecBahEn::Enabled
            }
        });
        w.set_clk_modem_sec_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkModemSecEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkModemSecEn::Enabled
            }
        });
        w.set_clk_ble_timer_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkBleTimerEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkBleTimerEn::Enabled
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_apb_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconClockGateState,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_bt_apb_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkBtApbEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkBtApbEn::Enabled
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_modem_security_apb_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconClockGateState,
) {
    registers.modify_clk_conf(|w| {
        w.set_clk_modem_sec_apb_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkModemSecApbEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf::ClkModemSecApbEn::Enabled
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_baseband_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconClockGateState,
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_btbb_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkBtbbEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::modem_syscon::clk_conf1::ClkBtbbEn::Enabled
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_mac_reset<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconResetState,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_btmac(match value {
            ModemSysconResetState::Released => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstBtmac::Released
            }
            ModemSysconResetState::Asserted => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstBtmac::Asserted
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_mac_apb_reset<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconResetState,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_btmac_apb(match value {
            ModemSysconResetState::Released => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstBtmacApb::Released
            }
            ModemSysconResetState::Asserted => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstBtmacApb::Asserted
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_timer_reset<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconResetState,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_ble_timer(match value {
            ModemSysconResetState::Released => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstBleTimer::Released
            }
            ModemSysconResetState::Asserted => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstBleTimer::Asserted
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_modem_ecb_reset<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconResetState,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_modem_ecb(match value {
            ModemSysconResetState::Released => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstModemEcb::Released
            }
            ModemSysconResetState::Asserted => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstModemEcb::Asserted
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_modem_ccm_reset<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconResetState,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_modem_ccm(match value {
            ModemSysconResetState::Released => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstModemCcm::Released
            }
            ModemSysconResetState::Asserted => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstModemCcm::Asserted
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_modem_bah_reset<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconResetState,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_modem_bah(match value {
            ModemSysconResetState::Released => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstModemBah::Released
            }
            ModemSysconResetState::Asserted => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstModemBah::Asserted
            }
        });
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_MODEM_SYSCON_STRUCT.
pub(crate) fn set_bluetooth_modem_security_reset<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_syscon::ModemSysconRadio<P>,
    value: ModemSysconResetState,
) {
    registers.modify_modem_rst_conf(|w| {
        w.set_rst_modem_sec(match value {
            ModemSysconResetState::Released => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstModemSec::Released
            }
            ModemSysconResetState::Asserted => {
                oer_esp32s31_pac::modem_syscon::modem_rst_conf::RstModemSec::Asserted
            }
        });
    });
}

// SOURCE: S31_ESP_PACS_SVD.
pub(crate) fn enable_modem_register_bus_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::hp_sys_clkrst::HpSysClkrstRadio<P>,
) {
    registers.modify_modem_ctrl0(|w| {
        w.set_reg_modem_clk_en(
            oer_esp32s31_pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn::Enabled,
        );
    });
}

// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK, S31_ESP_PACS_SVD.
pub(crate) fn restore_modem_register_bus_clock<P: Placement>(
    registers: &mut oer_esp32s31_pac::hp_sys_clkrst::HpSysClkrstRadio<P>,
    value: ModemSysconClockGateState,
) {
    registers.modify_modem_ctrl0(|w| {
        w.set_reg_modem_clk_en(match value {
            ModemSysconClockGateState::Disabled => {
                oer_esp32s31_pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn::Disabled
            }
            ModemSysconClockGateState::Enabled => {
                oer_esp32s31_pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn::Enabled
            }
        });
    });
}

// SOURCE: S31_ESP_PACS_SVD.
pub(crate) fn configure_modem_source_clocks<P: Placement>(
    registers: &mut oer_esp32s31_pac::hp_sys_clkrst::HpSysClkrstRadio<P>,
) {
    registers.modify_modem_conf(|w| {
        w.set_modem_apb_clk_en(true);
        w.set_modem_rst_en(false);
        w.set_modem_clk_en(true);
        w.set_modem_clk_source_sel(true);
        w.set_modem_pll_clk_en(true);
        w.set_modem_xtal_clk_en(true);
    });
}

// SOURCE: ESP_HAL_63E97BE_S31_MODEM_CLOCK_MAP, S31_PMU_HEADERS.
pub(crate) fn apply_modem_icg_selection<P: Placement>(
    registers: &mut oer_esp32s31_pac::pmu::PmuRadio<P>,
) {
    registers.trigger_imm_modem_icg_update_dig_icg_modem_en();
}

// SOURCE: ESP_HAL_63E97BE_S31_MODEM_CLOCK_MAP, S31_PMU_HEADERS.
pub(crate) fn apply_sleep_icg_selection<P: Placement>(
    registers: &mut oer_esp32s31_pac::pmu::PmuRadio<P>,
) {
    registers.trigger_imm_sleep_sysclk_update_dig_icg_switch();
}

// SOURCE: S31_PMU_HEADERS.
pub(crate) fn power_on_rf_circuits<P: Placement>(
    registers: &mut oer_esp32s31_pac::pmu::PmuRadio<P>,
) {
    registers.modify_rf_pwc(|w| {
        w.set_xpd_rf_circuit(oer_esp32s31_pac::pmu::rf_pwc::XpdRfCircuit::PoweredOn);
    });
}

// SOURCE: S31_PMU_HEADERS.
pub(crate) fn power_off_rf_circuits<P: Placement>(
    registers: &mut oer_esp32s31_pac::pmu::PmuRadio<P>,
) {
    registers.modify_rf_pwc(|w| {
        w.set_xpd_rf_circuit(oer_esp32s31_pac::pmu::rf_pwc::XpdRfCircuit::PoweredOff);
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
pub(crate) fn set_baseband_i2c_power_tie<P: Placement>(
    registers: &mut oer_esp32s31_pac::pmu::PmuRadio<P>,
    enabled: bool,
) {
    if enabled {
        registers.trigger_imm_hp_ck_power_0_tie_high_xpd_bb_i2c();
    }
}

// SOURCE: BLOB_LIBPHY_PHY_OPEN_I2C_XPD_NEW, S31_PMU_HEADERS.
pub(crate) fn power_on_analog_i2c<P: Placement>(
    registers: &mut oer_esp32s31_pac::pmu::PmuRadio<P>,
) {
    registers.modify_ana_peri_pwr_ctrl(|w| {
        w.set_xpd_perif_i2c(true);
    });
}

// SOURCE: BLOB_LIBPHY_PHY_OPEN_I2C_XPD_NEW, S31_PMU_HEADERS.
pub(crate) fn power_off_analog_i2c<P: Placement>(
    registers: &mut oer_esp32s31_pac::pmu::PmuRadio<P>,
) {
    registers.modify_ana_peri_pwr_ctrl(|w| {
        w.set_xpd_perif_i2c(false);
    });
}

// SOURCE: BLOB_LIBPHY_PHY_OPEN_I2C_XPD_NEW, S31_PMU_HEADERS.
pub(crate) fn release_analog_i2c_reset<P: Placement>(
    registers: &mut oer_esp32s31_pac::pmu::PmuRadio<P>,
) {
    registers.modify_ana_peri_pwr_ctrl(|w| {
        w.set_rstb_perif_i2c(true);
    });
}

// SOURCE: BLOB_LIBPHY_PHY_OPEN_I2C_XPD_NEW, S31_PMU_HEADERS.
pub(crate) fn assert_analog_i2c_reset<P: Placement>(
    registers: &mut oer_esp32s31_pac::pmu::PmuRadio<P>,
) {
    registers.modify_ana_peri_pwr_ctrl(|w| {
        w.set_rstb_perif_i2c(false);
    });
}

// SOURCE: ROM_REV0_PHY_OPEN_FE_BB_CLK, S31_PMU_HEADERS.
pub(crate) fn enable_frontend_baseband_power<P: Placement>(
    registers: &mut oer_esp32s31_pac::pmu::PmuRadio<P>,
) {
    registers.modify_hp_active_hp_ck_power(|w| {
        w.set_bits_0_3_opaque(
            oer_esp32s31_pac::pmu::hp_active_hp_ck_power::Bits0_3Opaque::RomRequired,
        );
        w.set_hp_active_xpd_bb_i2c(true);
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_HAL_63E97BE_S31_MODEM_CLOCK_MAP.
pub(crate) fn initialize_shared_modem_power_state_map<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_lpcon::ModemLpconSharedClock<P>,
) {
    registers.modify_clk_conf_power_st(|w| {
        w.set_clk_wifipwr_st_map(Bits::truncate(w.clk_wifipwr_st_map().get() | 6));
        w.set_clk_coex_st_map(Bits::truncate(w.clk_coex_st_map().get() | 6));
        w.set_clk_i2c_mst_st_map(Bits::truncate(w.clk_i2c_mst_st_map().get() | 6));
        w.set_clk_lp_apb_st_map(Bits::truncate(w.clk_lp_apb_st_map().get() | 6));
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_7B9CC1AC_S31_MODEM_LPCON_LL.
pub(crate) fn deselect_bluetooth_low_power_timer_slow_oscillator<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_lpcon::ModemLpconSharedClock<P>,
) {
    registers.modify_lp_timer_conf(|w| {
        w.set_clk_lp_timer_sel_osc_slow(false);
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_7B9CC1AC_S31_MODEM_LPCON_LL.
pub(crate) fn deselect_bluetooth_low_power_timer_fast_oscillator<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_lpcon::ModemLpconSharedClock<P>,
) {
    registers.modify_lp_timer_conf(|w| {
        w.set_clk_lp_timer_sel_osc_fast(false);
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_7B9CC1AC_S31_MODEM_LPCON_LL.
pub(crate) fn deselect_bluetooth_low_power_timer_crystal_32khz<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_lpcon::ModemLpconSharedClock<P>,
) {
    registers.modify_lp_timer_conf(|w| {
        w.set_clk_lp_timer_sel_xtal32k(false);
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_7B9CC1AC_S31_MODEM_LPCON_LL.
pub(crate) fn deselect_bluetooth_low_power_timer_crystal<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_lpcon::ModemLpconSharedClock<P>,
) {
    registers.modify_lp_timer_conf(|w| {
        w.set_clk_lp_timer_sel_xtal(false);
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_7B9CC1AC_S31_MODEM_LPCON_LL.
pub(crate) fn select_bluetooth_low_power_timer_crystal<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_lpcon::ModemLpconSharedClock<P>,
) {
    registers.modify_lp_timer_conf(|w| {
        w.set_clk_lp_timer_sel_xtal(true);
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_7B9CC1AC_S31_MODEM_LPCON_LL.
pub(crate) fn set_bluetooth_low_power_timer_divider<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_lpcon::ModemLpconSharedClock<P>,
    value: ModemLowPowerClockDivider,
) {
    registers.modify_lp_timer_conf(|w| {
        w.set_clk_lp_timer_div_num(Bits::truncate(value.get()));
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_4D59230D_S31_WIFI_LOW_POWER_CLOCK.
pub(crate) fn deselect_wifi_low_power_clock_slow_oscillator<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_lpcon::ModemLpconSharedClock<P>,
) {
    registers.modify_wifi_lp_clk_conf(|w| {
        w.set_clk_wifipwr_lp_sel_osc_slow(false);
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_4D59230D_S31_WIFI_LOW_POWER_CLOCK.
pub(crate) fn deselect_wifi_low_power_clock_fast_oscillator<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_lpcon::ModemLpconSharedClock<P>,
) {
    registers.modify_wifi_lp_clk_conf(|w| {
        w.set_clk_wifipwr_lp_sel_osc_fast(false);
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_4D59230D_S31_WIFI_LOW_POWER_CLOCK.
pub(crate) fn deselect_wifi_low_power_clock_crystal_32khz<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_lpcon::ModemLpconSharedClock<P>,
) {
    registers.modify_wifi_lp_clk_conf(|w| {
        w.set_clk_wifipwr_lp_sel_xtal32k(false);
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_4D59230D_S31_WIFI_LOW_POWER_CLOCK.
pub(crate) fn deselect_wifi_low_power_clock_crystal<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_lpcon::ModemLpconSharedClock<P>,
) {
    registers.modify_wifi_lp_clk_conf(|w| {
        w.set_clk_wifipwr_lp_sel_xtal(false);
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_4D59230D_S31_WIFI_LOW_POWER_CLOCK.
pub(crate) fn select_wifi_low_power_clock_slow_oscillator<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_lpcon::ModemLpconSharedClock<P>,
) {
    registers.modify_wifi_lp_clk_conf(|w| {
        w.set_clk_wifipwr_lp_sel_osc_slow(true);
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_4D59230D_S31_WIFI_LOW_POWER_CLOCK.
pub(crate) fn select_wifi_low_power_clock_crystal_32khz<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_lpcon::ModemLpconSharedClock<P>,
) {
    registers.modify_wifi_lp_clk_conf(|w| {
        w.set_clk_wifipwr_lp_sel_xtal32k(true);
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_4D59230D_S31_WIFI_LOW_POWER_CLOCK.
pub(crate) fn select_modem_32khz_clock_crystal<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_lpcon::ModemLpconSharedClock<P>,
) {
    registers.modify_modem_32k_clk_conf(|w| {
        w.set_clk_modem_32k_sel(Bits::constant::<0>());
    });
}

// SOURCE: S31_MODEM_LPCON_STRUCT, ESP_IDF_4D59230D_S31_WIFI_LOW_POWER_CLOCK.
pub(crate) fn set_wifi_low_power_clock_divider<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_lpcon::ModemLpconSharedClock<P>,
    value: ModemLowPowerClockDivider,
) {
    registers.modify_wifi_lp_clk_conf(|w| {
        w.set_clk_wifipwr_lp_div_num(Bits::truncate(value.get()));
    });
}

// SOURCE: BLOB_LIBCOEX_TIMER_CLOCK, S31_MODEM_LPCON_STRUCT.
pub(crate) fn select_coex_timer_clock_source<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_lpcon::ModemLpconSharedClock<P>,
    value: CoexTimerClockSource,
) {
    registers.modify_coex_lp_clk_conf(|w| {
        w.set_clk_coex_lp_sel_osc_slow(match value {
            CoexTimerClockSource::Selector1 => true,
            CoexTimerClockSource::Selector2 => false,
            CoexTimerClockSource::Selector4 => false,
            CoexTimerClockSource::Selector8 => false,
        });
        w.set_clk_coex_lp_sel_osc_fast(match value {
            CoexTimerClockSource::Selector1 => false,
            CoexTimerClockSource::Selector2 => true,
            CoexTimerClockSource::Selector4 => false,
            CoexTimerClockSource::Selector8 => false,
        });
        w.set_clk_coex_lp_sel_xtal(match value {
            CoexTimerClockSource::Selector1 => false,
            CoexTimerClockSource::Selector2 => false,
            CoexTimerClockSource::Selector4 => true,
            CoexTimerClockSource::Selector8 => false,
        });
        w.set_clk_coex_lp_sel_xtal32k(match value {
            CoexTimerClockSource::Selector1 => false,
            CoexTimerClockSource::Selector2 => false,
            CoexTimerClockSource::Selector4 => false,
            CoexTimerClockSource::Selector8 => true,
        });
    });
}

// SOURCE: BLOB_LIBCOEX_TIMER_CLOCK, S31_MODEM_LPCON_STRUCT.
pub(crate) fn set_coex_timer_clock_divider<P: Placement>(
    registers: &mut oer_esp32s31_pac::modem_lpcon::ModemLpconSharedClock<P>,
    value: CoexTimerClockDividerMinusOne,
) {
    registers.modify_coex_lp_clk_conf(|w| {
        w.set_clk_coex_lp_div_num(Bits::truncate(value.get()));
    });
}

#[cfg(test)]
mod tests {
    use super::set_baseband_i2c_power_tie;
    use oer_esp32s31_pac::{instances, pmu::PmuRadio};
    use std::vec;

    /// `IMM_HP_CK_POWER_0`, the word of the immediate clock ties.
    const IMM_HP_CK_POWER_0: usize = 0xf0;

    #[test]
    fn the_tie_triggers_high_and_its_release_writes_nothing() {
        for fill in [0, u32::MAX] {
            let mut memory = vec![fill; instances::pmu::SIZE.div_ceil(4)];
            let mut expected = memory.clone();
            set_baseband_i2c_power_tie(&mut PmuRadio::host(&mut memory).expect("PMU"), false);
            assert_eq!(memory, expected, "the release writes nothing");
            set_baseband_i2c_power_tie(&mut PmuRadio::host(&mut memory).expect("PMU"), true);
            expected[IMM_HP_CK_POWER_0 / 4] = 1 << 28;
            assert_eq!(memory, expected, "the tie stores its one trigger");
        }
    }
}
