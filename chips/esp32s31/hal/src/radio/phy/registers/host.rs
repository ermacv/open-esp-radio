//! The PHY owner over separate host memory for each owned PAC part.

use std::{vec, vec::Vec};

use oer_chips_esp32s31_pac::{self as pac, instances};

use super::{
    AgcRegisters, BasebandRegisters, ClockRegisters, FrequencyRegisters, HostPlacement,
    RadioPhyParts, RadioPhyRegisters,
};

/// Lend the complete PHY owner without taking a device claim or leaking memory.
pub(crate) fn with_registers<R>(
    fill: u32,
    run: impl FnOnce(&mut RadioPhyRegisters<HostPlacement<'_>>) -> R,
) -> R {
    with_registers_memory(fill, run).0
}

/// The memory of one owned part after a run, named by its owner field.
pub(crate) type PartMemory = (&'static str, Vec<u32>);

/// Lend the complete PHY owner, then return every part's memory with the
/// result, so a test compares whole images.
pub(crate) fn with_registers_memory<R>(
    fill: u32,
    run: impl FnOnce(&mut RadioPhyRegisters<HostPlacement<'_>>) -> R,
) -> (R, Vec<PartMemory>) {
    macro_rules! parts {
        (@size $instance:ident) => { instances::$instance::SIZE };
        (@size $module:ident => $instance:ident) => { instances::$instance::SIZE };
        ($memory:ident; $($name:ident: $module:ident::$part:ident $(=> $instance:ident)?),* $(,)?) => {
            let mut $memory: Vec<PartMemory> = vec![
                $((stringify!($name), vec![fill; parts!(@size $module $(=> $instance)?).div_ceil(4)])),*
            ];
            let mut slots = $memory.iter_mut();
            $(
                let $name = pac::$module::$part::host(
                    &mut slots.next().expect("one memory a part").1,
                )
                .expect("host memory holds the instance");
            )*
        };
    }
    let mut local_time = vec![fill; instances::modem_pwr::SIZE.div_ceil(4)];
    parts! {
        memory;
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
        modem_syscon_clock_reset: modem_syscon::ModemSysconClockReset,
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
        bluetooth_lp_timer_clock: modem_lpcon::BluetoothLpTimerClock,
        coex_lp_clock: modem_lpcon::CoexLpClock,
        modem_lpcon_power: modem_lpcon::ModemLpconPower,
        modem_lpcon_phy_tick: modem_lpcon::ModemLpconPhyTick,
        lp_aon_radio_clock: lp_aonclkrst::LpAonRadioClock => lp_aon_clkrst,
        lp_aon_clkrst: lp_aonclkrst::LpAonClkrst => lp_aon_clkrst,
        lp_periclkrst: lp_peri_clkrst::LpPericlkrst => lp_periclkrst,
        lp_tsens: tsens::LpTsens => lp_tsens,
        i2c_ana_mst: i2c_ana_mst::I2cAnaMst,
        phy_memory: fe::PhyMemory,
        phy_nrx_recovered_gaps: wifi_bb::PhyNrxRecoveredGaps,
        phy_pbus: fe::PhyPbus,
        phy_rx_dco_oracle: fe::PhyRxDcoOracle,
    }
    let mac_local_time = crate::radio::ieee80211::registers::MacLocalTime::host(&mut local_time)
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
            modem_syscon_clock_reset,
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
            bluetooth_lp_timer_clock,
            coex_lp_clock,
            modem_lpcon_power,
            modem_lpcon_phy_tick,
            lp_aon_radio_clock,
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
    let result = run(&mut registers);
    drop(registers);
    memory.push(("mac_local_time", local_time));
    (result, memory)
}
