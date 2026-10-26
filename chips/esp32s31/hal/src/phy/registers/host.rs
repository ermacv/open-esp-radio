//! The PHY owner over separate host memory for each owned PAC part.

use std::vec;

use oer_esp32s31_pac::{self as pac, instances};

use super::{
    AgcRegisters, BasebandRegisters, ClockRegisters, FrequencyRegisters, HostPlacement,
    RadioPhyParts, RadioPhyRegisters,
};

/// Lend the complete PHY owner without taking a device claim or leaking memory.
pub(crate) fn with_registers<R>(
    fill: u32,
    run: impl FnOnce(&mut RadioPhyRegisters<HostPlacement<'_>>) -> R,
) -> R {
    macro_rules! parts {
        (@size $instance:ident) => { instances::$instance::SIZE };
        (@size $module:ident => $instance:ident) => { instances::$instance::SIZE };
        ($($name:ident: $module:ident::$part:ident $(=> $instance:ident)?),* $(,)?) => {
            $(
                let mut $name = vec![fill; parts!(@size $module $(=> $instance)?).div_ceil(4)];
                let $name = pac::$module::$part::host(&mut $name)
                    .expect("host memory holds the instance");
            )*
        };
    }
    parts! {
        phy_agc_oracle_fe: fe::PhyAgcOracleFe,
        phy_agc_oracle_wifi_bb: wifi_bb::PhyAgcOracleWifiBb,
        phy_agc_oracle_wifi_bb_brx: wifi_bb_brx::PhyAgcOracleWifiBbBrx,
        phy_baseband_config_oracle_fe: fe::PhyBasebandConfigOracleFe,
        phy_baseband_config_oracle_wifi_bb: wifi_bb::PhyBasebandConfigOracleWifiBb,
        phy_baseband_config_oracle_wifi_mac: wifi_mac::PhyBasebandConfigOracleWifiMac,
        phy_clock_oracle_fe: fe::PhyClockOracleFe,
        phy_clock_oracle_wifi_bb: wifi_bb::PhyClockOracleWifiBb,
        phy_frequency_channel_oracle_fe: fe::PhyFrequencyChannelOracleFe,
        phy_frequency_channel_oracle_wifi_bb: wifi_bb::PhyFrequencyChannelOracleWifiBb,
        phy_frequency_channel_oracle_wifi_mac: wifi_mac::PhyFrequencyChannelOracleWifiMac,
        hp_sys_clkrst_radio: hp_sys_clkrst::HpSysClkrstRadio,
        pmu_radio: pmu::PmuRadio,
        modem_syscon_radio: modem_syscon::ModemSysconRadio,
        phy_agc_recovered_gaps: wifi_bb::PhyAgcRecoveredGaps,
        phy_bb_recovered_gaps: wifi_bb::PhyBbRecoveredGaps,
        phy_brx_recovered_gaps: wifi_bb_brx::PhyBrxRecoveredGaps,
        phy_btagc_recovered: bt_bb::PhyBtagcRecovered,
        phy_fectrl_recovered: fe::PhyFectrlRecovered,
        phy_adc_dump_recovered_fe: fe::PhyAdcDumpRecoveredFe,
        phy_adc_dump_recovered_bt_bb: bt_bb::PhyAdcDumpRecoveredBtBb,
        phy_adc_dump_recovered_wifi_bb: wifi_bb::PhyAdcDumpRecoveredWifiBb,
        phy_adc_dump_recovered_modem_datadump: modem_datadump::PhyAdcDumpRecoveredModemDatadump,
        phy_fedata_recovered: fe::PhyFedataRecovered,
        phy_fedata_wifi_recovered: fe::PhyFedataWifiRecovered,
        phy_i2c_command_ram: i2c_ana_mst_mem::PhyI2cCommandRam,
        phy_iq_estimator_oracle: fe::PhyIqEstimatorOracle,
        modem_lpcon_shared_clock: modem_lpcon::ModemLpconSharedClock,
        modem_lpcon_phy_tick: modem_lpcon::ModemLpconPhyTick,
        lp_aon_clkrst: lp_aonclkrst::LpAonClkrst => lp_aon_clkrst,
        lp_periclkrst: lp_peri_clkrst::LpPericlkrst => lp_periclkrst,
        lp_tsens: tsens::LpTsens => lp_tsens,
        i2c_ana_mst: i2c_ana_mst::I2cAnaMst,
        phy_memory: fe::PhyMemory,
        phy_nrx_recovered_gaps: wifi_bb::PhyNrxRecoveredGaps,
        phy_pbus: fe::PhyPbus,
        phy_rx_dco_oracle: fe::PhyRxDcoOracle,
    }
    let mut local_time = vec![fill; instances::modem_pwr::SIZE.div_ceil(4)];
    let mac_local_time = crate::ieee80211::registers::MacLocalTime::host(&mut local_time)
        .expect("host memory holds the local-time counter");
    let mut registers = RadioPhyRegisters {
        peripherals: RadioPhyParts {
            phy_agc_oracle: AgcRegisters {
                fe: phy_agc_oracle_fe,
                wifi_bb: phy_agc_oracle_wifi_bb,
                wifi_bb_brx: phy_agc_oracle_wifi_bb_brx,
            },
            phy_baseband_config_oracle: BasebandRegisters {
                fe: phy_baseband_config_oracle_fe,
                wifi_bb: phy_baseband_config_oracle_wifi_bb,
                wifi_mac: phy_baseband_config_oracle_wifi_mac,
            },
            phy_clock_oracle: ClockRegisters {
                fe: phy_clock_oracle_fe,
                wifi_bb: phy_clock_oracle_wifi_bb,
            },
            phy_frequency_channel_oracle: FrequencyRegisters {
                fe: phy_frequency_channel_oracle_fe,
                wifi_bb: phy_frequency_channel_oracle_wifi_bb,
                wifi_mac: phy_frequency_channel_oracle_wifi_mac,
            },
            hp_sys_clkrst_radio,
            pmu_radio,
            modem_syscon_radio,
            phy_agc_recovered_gaps,
            phy_bb_recovered_gaps,
            phy_brx_recovered_gaps,
            phy_btagc_recovered,
            phy_fectrl_recovered,
            phy_adc_dump_recovered_fe,
            phy_adc_dump_recovered_bt_bb,
            phy_adc_dump_recovered_wifi_bb,
            phy_adc_dump_recovered_modem_datadump,
            phy_fedata_recovered,
            phy_fedata_wifi_recovered,
            phy_i2c_command_ram,
            phy_iq_estimator_oracle,
            modem_lpcon_shared_clock,
            modem_lpcon_phy_tick,
            lp_aon_clkrst,
            lp_periclkrst,
            lp_tsens,
            i2c_ana_mst,
            phy_memory,
            phy_nrx_recovered_gaps,
            phy_pbus,
            phy_rx_dco_oracle,
        },
        mac_local_time,
    };
    run(&mut registers)
}
