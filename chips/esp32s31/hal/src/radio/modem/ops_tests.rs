//! Golden memory images of the modem and coexistence register operations.
//!
//! Each case runs one operation on host memory filled with one value and
//! compares the whole part against the fill with the listed words, given as
//! byte offsets from the instance's base and their stored values, replaced.
//! The images were recorded from the operations as merged in 9fbc8ea43 and
//! are written out here, so a later rewrite of an operation cannot pass by
//! reading back its own error. The access order inside one operation is not
//! visible in host memory.

use super::ops;
use super::values::*;
use crate::radio::coex::external::*;
use oer_chips_esp32s31_pac::{
    coex::CoexExternal,
    dictionaries::Reset,
    hp_sys_clkrst::HpSysClkrstRadio,
    instances,
    modem_lpcon::{BluetoothLpTimerClock, CoexLpClock, ModemLpconPower},
    modem_syscon::{ModemSysconClockReset, ModemSysconRadio, dictionaries::ClockGate},
    pmu::PmuRadio,
};
use std::{vec, vec::Vec};

/// Run `operation` over `words` words filled with `fill` and compare the
/// whole memory with the fill and `changed` words replaced.
fn check(words: usize, fill: u32, changed: &[(usize, u32)], operation: impl FnOnce(&mut [u32])) {
    let mut memory: Vec<u32> = vec![fill; words];
    operation(&mut memory);
    let mut expected: Vec<u32> = vec![fill; words];
    for &(offset, value) in changed {
        expected[offset / 4] = value;
    }
    for (index, (got, want)) in memory.iter().zip(&expected).enumerate() {
        assert_eq!(
            got,
            want,
            "word {:#06x} from fill {fill:#010x}: {got:#010x}, expected {want:#010x}",
            index * 4
        );
    }
}

macro_rules! golden {
    ($name:ident, $part:ident, $instance:ident, |$b:ident| $operation:block, [$(($fill:expr, $changed:expr)),* $(,)?] $(,)?) => {
        #[test]
        fn $name() {
            $(
                check(instances::$instance::SIZE.div_ceil(4), $fill, $changed, |memory| {
                    let mut $b = $part::host(memory).expect("memory holds the part");
                    $operation
                });
            )*
        }
    };
}

golden!(
    set_wifi_baseband_and_mac_reset_released,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_wifi_baseband_and_mac_reset(&mut b, Reset::Released);
    },
    [(0, &[]), (u32::MAX, &[(0x0010, 0xfffffcff)])]
);

golden!(
    set_wifi_baseband_and_mac_reset_asserted,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_wifi_baseband_and_mac_reset(&mut b, Reset::Asserted);
    },
    [(0, &[(0x0010, 0x00000300)]), (u32::MAX, &[])]
);

golden!(
    set_wifi_baseband_reset_released,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_wifi_baseband_reset(&mut b, Reset::Released);
    },
    [(0, &[]), (u32::MAX, &[(0x0010, 0xfffffeff)])]
);

golden!(
    set_wifi_baseband_reset_asserted,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_wifi_baseband_reset(&mut b, Reset::Asserted);
    },
    [(0, &[(0x0010, 0x00000100)]), (u32::MAX, &[])]
);

golden!(
    enable_phy_calibration_clocks,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::enable_phy_calibration_clocks(&mut b);
    },
    [(0, &[(0x0014, 0x003be5ff)]), (u32::MAX, &[])]
);

golden!(
    select_phy_i2c_160mhz_source,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::select_phy_i2c_160mhz_source(&mut b);
    },
    [(0, &[(0x0004, 0x00001000)]), (u32::MAX, &[])]
);

golden!(
    restore_phy_i2c_160mhz_source_default,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::restore_phy_i2c_160mhz_source(
            &mut b,
            oer_chips_esp32s31_pac::modem_syscon::clk_conf::ClkI2cMstSel160m::Default,
        );
    },
    [(0, &[]), (u32::MAX, &[(0x0004, 0xffffefff)])]
);

golden!(
    restore_phy_i2c_160mhz_source_mst_160m,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::restore_phy_i2c_160mhz_source(
            &mut b,
            oer_chips_esp32s31_pac::modem_syscon::clk_conf::ClkI2cMstSel160m::I2cMst160m,
        );
    },
    [(0, &[(0x0004, 0x00001000)]), (u32::MAX, &[])]
);

golden!(
    enable_wifi_mac_clocks,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::enable_wifi_mac_clocks(&mut b);
    },
    [(0, &[(0x0014, 0x000007ff)]), (u32::MAX, &[])]
);

golden!(
    set_wifi_mac_reset_released,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_wifi_mac_reset(&mut b, Reset::Released);
    },
    [(0, &[]), (u32::MAX, &[(0x0010, 0xfffffdff)])]
);

golden!(
    set_wifi_mac_reset_asserted,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_wifi_mac_reset(&mut b, Reset::Asserted);
    },
    [(0, &[(0x0010, 0x00000200)]), (u32::MAX, &[])]
);

golden!(
    clear_cold_start_wifi_control,
    ModemSysconRadio,
    modem_syscon,
    |b| {
        ops::clear_cold_start_wifi_control(&mut b);
    },
    [(0, &[]), (u32::MAX, &[(0x0018, 0xfffffffc)])]
);

golden!(
    set_wifi_baseband_enable_disabled,
    ModemSysconRadio,
    modem_syscon,
    |b| {
        ops::set_wifi_baseband_enable(
            &mut b,
            oer_chips_esp32s31_pac::modem_syscon::wifi_bb_cfg::WifiEnable::Disabled,
        );
    },
    [(0, &[]), (u32::MAX, &[(0x0018, 0xfffffffd)])]
);

golden!(
    set_wifi_baseband_enable_enabled,
    ModemSysconRadio,
    modem_syscon,
    |b| {
        ops::set_wifi_baseband_enable(
            &mut b,
            oer_chips_esp32s31_pac::modem_syscon::wifi_bb_cfg::WifiEnable::Enabled,
        );
    },
    [(0, &[(0x0018, 0x00000002)]), (u32::MAX, &[])]
);

golden!(
    set_wifi_baseband_40mhz_state_disabled,
    ModemSysconRadio,
    modem_syscon,
    |b| {
        ops::set_wifi_baseband_40mhz_state(
            &mut b,
            oer_chips_esp32s31_pac::modem_syscon::wifi_bb_cfg::BssCbw40Digital::Disabled,
        );
    },
    [(0, &[]), (u32::MAX, &[(0x0018, 0xfffffff3)])]
);

golden!(
    set_wifi_baseband_40mhz_state_enabled,
    ModemSysconRadio,
    modem_syscon,
    |b| {
        ops::set_wifi_baseband_40mhz_state(
            &mut b,
            oer_chips_esp32s31_pac::modem_syscon::wifi_bb_cfg::BssCbw40Digital::Enabled,
        );
    },
    [
        (0, &[(0x0018, 0x00000004)]),
        (u32::MAX, &[(0x0018, 0xfffffff7)])
    ]
);

golden!(
    set_wifi_baseband_agc_update_mode_initialization,
    ModemSysconRadio,
    modem_syscon,
    |b| {
        ops::set_wifi_baseband_agc_update_mode(
            &mut b,
            oer_chips_esp32s31_pac::modem_syscon::wifi_bb_cfg::Bits11_13Opaque::Initialization,
        );
    },
    [
        (0, &[(0x0018, 0x00000800)]),
        (u32::MAX, &[(0x0018, 0xffffcfff)])
    ]
);

golden!(
    set_wifi_baseband_agc_update_mode_updates,
    ModemSysconRadio,
    modem_syscon,
    |b| {
        ops::set_wifi_baseband_agc_update_mode(&mut b, oer_chips_esp32s31_pac::modem_syscon::wifi_bb_cfg::Bits11_13Opaque::RegisterUpdatesEnabled);
    },
    [(0, &[(0x0018, 0x00003800)]), (u32::MAX, &[])]
);

golden!(
    set_mac_baseband_enable_disabled,
    ModemSysconRadio,
    modem_syscon,
    |b| {
        ops::set_mac_baseband_enable(
            &mut b,
            oer_chips_esp32s31_pac::modem_syscon::wifi_bb_cfg::Bit28Opaque::Disabled,
        );
    },
    [(0, &[]), (u32::MAX, &[(0x0018, 0xefffffff)])]
);

golden!(
    set_mac_baseband_enable_enabled,
    ModemSysconRadio,
    modem_syscon,
    |b| {
        ops::set_mac_baseband_enable(
            &mut b,
            oer_chips_esp32s31_pac::modem_syscon::wifi_bb_cfg::Bit28Opaque::Enabled,
        );
    },
    [(0, &[(0x0018, 0x10000000)]), (u32::MAX, &[])]
);

golden!(
    prepare_modem_syscon_clock_map,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::prepare_modem_syscon_clock_map(&mut b);
    },
    [(0, &[(0x000c, 0x64646400)]), (u32::MAX, &[])]
);

golden!(
    set_modem_wifi_mac_clock_disabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_modem_wifi_mac_clock(&mut b, ClockGate::Disabled);
    },
    [(0, &[]), (u32::MAX, &[(0x0014, 0xfffffdff)])]
);

golden!(
    set_modem_wifi_mac_clock_enabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_modem_wifi_mac_clock(&mut b, ClockGate::Enabled);
    },
    [(0, &[(0x0014, 0x00000200)]), (u32::MAX, &[])]
);

golden!(
    set_modem_wifi_baseband_clocks_disabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_modem_wifi_baseband_clocks(&mut b, ClockGate::Disabled);
    },
    [(0, &[]), (u32::MAX, &[(0x0014, 0xfffffe84)])]
);

golden!(
    set_modem_wifi_baseband_clocks_enabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_modem_wifi_baseband_clocks(&mut b, ClockGate::Enabled);
    },
    [(0, &[(0x0014, 0x0000017b)]), (u32::MAX, &[])]
);

golden!(
    set_modem_wifi_apb_clock_disabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_modem_wifi_apb_clock(&mut b, ClockGate::Disabled);
    },
    [(0, &[]), (u32::MAX, &[(0x0014, 0xfffffbff)])]
);

golden!(
    set_modem_wifi_apb_clock_enabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_modem_wifi_apb_clock(&mut b, ClockGate::Enabled);
    },
    [(0, &[(0x0014, 0x00000400)]), (u32::MAX, &[])]
);

golden!(
    set_modem_wifi_baseband_44m_clock_disabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_modem_wifi_baseband_44m_clock(&mut b, ClockGate::Disabled);
    },
    [(0, &[]), (u32::MAX, &[(0x0014, 0xfffffffb)])]
);

golden!(
    set_modem_wifi_baseband_44m_clock_enabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_modem_wifi_baseband_44m_clock(&mut b, ClockGate::Enabled);
    },
    [(0, &[(0x0014, 0x00000004)]), (u32::MAX, &[])]
);

golden!(
    set_modem_ieee802154_apb_clock_disabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_modem_ieee802154_apb_clock(&mut b, ClockGate::Disabled);
    },
    [(0, &[]), (u32::MAX, &[(0x0004, 0xff7fffff)])]
);

golden!(
    set_modem_ieee802154_apb_clock_enabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_modem_ieee802154_apb_clock(&mut b, ClockGate::Enabled);
    },
    [(0, &[(0x0004, 0x00800000)]), (u32::MAX, &[])]
);

golden!(
    set_modem_ieee802154_mac_clock_disabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_modem_ieee802154_mac_clock(&mut b, ClockGate::Disabled);
    },
    [(0, &[]), (u32::MAX, &[(0x0004, 0xfeffffff)])]
);

golden!(
    set_modem_ieee802154_mac_clock_enabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_modem_ieee802154_mac_clock(&mut b, ClockGate::Enabled);
    },
    [(0, &[(0x0004, 0x01000000)]), (u32::MAX, &[])]
);

golden!(
    enable_modem_frontend_apb_clock,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::enable_modem_frontend_apb_clock(&mut b);
    },
    [(0, &[(0x0014, 0x00008000)]), (u32::MAX, &[])]
);

golden!(
    enable_modem_frontend_80m_clock,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::enable_modem_frontend_80m_clock(&mut b);
    },
    [(0, &[(0x0014, 0x00002000)]), (u32::MAX, &[])]
);

golden!(
    enable_modem_frontend_160m_clock,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::enable_modem_frontend_160m_clock(&mut b);
    },
    [(0, &[(0x0014, 0x00004000)]), (u32::MAX, &[])]
);

golden!(
    enable_modem_frontend_dac_clock,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::enable_modem_frontend_dac_clock(&mut b);
    },
    [(0, &[(0x0014, 0x00200000)]), (u32::MAX, &[])]
);

golden!(
    enable_modem_frontend_pwdet_clock,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::enable_modem_frontend_pwdet_clock(&mut b);
    },
    [(0, &[(0x0014, 0x00080000)]), (u32::MAX, &[])]
);

golden!(
    enable_modem_frontend_adc_clock,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::enable_modem_frontend_adc_clock(&mut b);
    },
    [(0, &[(0x0014, 0x00100000)]), (u32::MAX, &[])]
);

golden!(
    set_bluetooth_wifi_baseband_80x1_clock_disabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_wifi_baseband_80x1_clock(&mut b, ClockGate::Disabled);
    },
    [(0, &[]), (u32::MAX, &[(0x0014, 0xffffff7f)])]
);

golden!(
    set_bluetooth_wifi_baseband_80x1_clock_enabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_wifi_baseband_80x1_clock(&mut b, ClockGate::Enabled);
    },
    [(0, &[(0x0014, 0x00000080)]), (u32::MAX, &[])]
);

golden!(
    set_bluetooth_etm_clock_disabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_etm_clock(&mut b, ClockGate::Disabled);
    },
    [(0, &[]), (u32::MAX, &[(0x0004, 0xffbfffff)])]
);

golden!(
    set_bluetooth_etm_clock_enabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_etm_clock(&mut b, ClockGate::Enabled);
    },
    [(0, &[(0x0004, 0x00400000)]), (u32::MAX, &[])]
);

golden!(
    set_bluetooth_mac_clock_disabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_mac_clock(&mut b, ClockGate::Disabled);
    },
    [(0, &[]), (u32::MAX, &[(0x0014, 0xfffbffff)])]
);

golden!(
    set_bluetooth_mac_clock_enabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_mac_clock(&mut b, ClockGate::Enabled);
    },
    [(0, &[(0x0014, 0x00040000)]), (u32::MAX, &[])]
);

golden!(
    set_bluetooth_peripheral_clocks_disabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_peripheral_clocks(&mut b, ClockGate::Disabled);
    },
    [(0, &[]), (u32::MAX, &[(0x0004, 0x91ffffff)])]
);

golden!(
    set_bluetooth_peripheral_clocks_enabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_peripheral_clocks(&mut b, ClockGate::Enabled);
    },
    [(0, &[(0x0004, 0x6e000000)]), (u32::MAX, &[])]
);

golden!(
    set_bluetooth_apb_clock_disabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_apb_clock(&mut b, ClockGate::Disabled);
    },
    [(0, &[]), (u32::MAX, &[(0x0014, 0xfffeffff)])]
);

golden!(
    set_bluetooth_apb_clock_enabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_apb_clock(&mut b, ClockGate::Enabled);
    },
    [(0, &[(0x0014, 0x00010000)]), (u32::MAX, &[])]
);

golden!(
    set_bluetooth_modem_security_apb_clock_disabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_modem_security_apb_clock(&mut b, ClockGate::Disabled);
    },
    [(0, &[]), (u32::MAX, &[(0x0004, 0xefffffff)])]
);

golden!(
    set_bluetooth_modem_security_apb_clock_enabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_modem_security_apb_clock(&mut b, ClockGate::Enabled);
    },
    [(0, &[(0x0004, 0x10000000)]), (u32::MAX, &[])]
);

golden!(
    set_bluetooth_baseband_clock_disabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_baseband_clock(&mut b, ClockGate::Disabled);
    },
    [(0, &[]), (u32::MAX, &[(0x0014, 0xfffdffff)])]
);

golden!(
    set_bluetooth_baseband_clock_enabled,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_baseband_clock(&mut b, ClockGate::Enabled);
    },
    [(0, &[(0x0014, 0x00020000)]), (u32::MAX, &[])]
);

golden!(
    set_bluetooth_mac_reset_released,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_mac_reset(&mut b, Reset::Released);
    },
    [(0, &[]), (u32::MAX, &[(0x0010, 0xfffeffff)])]
);

golden!(
    set_bluetooth_mac_reset_asserted,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_mac_reset(&mut b, Reset::Asserted);
    },
    [(0, &[(0x0010, 0x00010000)]), (u32::MAX, &[])]
);

golden!(
    set_bluetooth_mac_apb_reset_released,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_mac_apb_reset(&mut b, Reset::Released);
    },
    [(0, &[]), (u32::MAX, &[(0x0010, 0xffff7fff)])]
);

golden!(
    set_bluetooth_mac_apb_reset_asserted,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_mac_apb_reset(&mut b, Reset::Asserted);
    },
    [(0, &[(0x0010, 0x00008000)]), (u32::MAX, &[])]
);

golden!(
    set_bluetooth_timer_reset_released,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_timer_reset(&mut b, Reset::Released);
    },
    [(0, &[]), (u32::MAX, &[(0x0010, 0xbfffffff)])]
);

golden!(
    set_bluetooth_timer_reset_asserted,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_timer_reset(&mut b, Reset::Asserted);
    },
    [(0, &[(0x0010, 0x40000000)]), (u32::MAX, &[])]
);

golden!(
    set_bluetooth_modem_ecb_reset_released,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_modem_ecb_reset(&mut b, Reset::Released);
    },
    [(0, &[]), (u32::MAX, &[(0x0010, 0xfdffffff)])]
);

golden!(
    set_bluetooth_modem_ecb_reset_asserted,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_modem_ecb_reset(&mut b, Reset::Asserted);
    },
    [(0, &[(0x0010, 0x02000000)]), (u32::MAX, &[])]
);

golden!(
    set_bluetooth_modem_ccm_reset_released,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_modem_ccm_reset(&mut b, Reset::Released);
    },
    [(0, &[]), (u32::MAX, &[(0x0010, 0xfbffffff)])]
);

golden!(
    set_bluetooth_modem_ccm_reset_asserted,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_modem_ccm_reset(&mut b, Reset::Asserted);
    },
    [(0, &[(0x0010, 0x04000000)]), (u32::MAX, &[])]
);

golden!(
    set_bluetooth_modem_bah_reset_released,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_modem_bah_reset(&mut b, Reset::Released);
    },
    [(0, &[]), (u32::MAX, &[(0x0010, 0xf7ffffff)])]
);

golden!(
    set_bluetooth_modem_bah_reset_asserted,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_modem_bah_reset(&mut b, Reset::Asserted);
    },
    [(0, &[(0x0010, 0x08000000)]), (u32::MAX, &[])]
);

golden!(
    set_bluetooth_modem_security_reset_released,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_modem_security_reset(&mut b, Reset::Released);
    },
    [(0, &[]), (u32::MAX, &[(0x0010, 0xdfffffff)])]
);

golden!(
    set_bluetooth_modem_security_reset_asserted,
    ModemSysconClockReset,
    modem_syscon,
    |b| {
        ops::set_bluetooth_modem_security_reset(&mut b, Reset::Asserted);
    },
    [(0, &[(0x0010, 0x20000000)]), (u32::MAX, &[])]
);

golden!(
    enable_modem_register_bus_clock,
    HpSysClkrstRadio,
    hp_sys_clkrst,
    |b| {
        ops::enable_modem_register_bus_clock(&mut b);
    },
    [(0, &[(0x0040, 0x00000001)]), (u32::MAX, &[])]
);

golden!(
    restore_modem_register_bus_clock_disabled,
    HpSysClkrstRadio,
    hp_sys_clkrst,
    |b| {
        ops::restore_modem_register_bus_clock(
            &mut b,
            oer_chips_esp32s31_pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn::Disabled,
        );
    },
    [(0, &[]), (u32::MAX, &[(0x0040, 0xfffffffe)])]
);

golden!(
    restore_modem_register_bus_clock_enabled,
    HpSysClkrstRadio,
    hp_sys_clkrst,
    |b| {
        ops::restore_modem_register_bus_clock(
            &mut b,
            oer_chips_esp32s31_pac::hp_sys_clkrst::modem_ctrl0::RegModemClkEn::Enabled,
        );
    },
    [(0, &[(0x0040, 0x00000001)]), (u32::MAX, &[])]
);

golden!(
    configure_modem_source_clocks,
    HpSysClkrstRadio,
    hp_sys_clkrst,
    |b| {
        ops::configure_modem_source_clocks(&mut b);
    },
    [
        (0, &[(0x01e0, 0x0000001d)]),
        (u32::MAX, &[(0x01e0, 0x0000001d)])
    ]
);

golden!(
    apply_modem_icg_selection,
    PmuRadio,
    pmu,
    |b| {
        ops::apply_modem_icg_selection(&mut b);
    },
    [
        (0, &[(0x0104, 0x80000000)]),
        (u32::MAX, &[(0x0104, 0x80000000)])
    ]
);

golden!(
    apply_sleep_icg_selection,
    PmuRadio,
    pmu,
    |b| {
        ops::apply_sleep_icg_selection(&mut b);
    },
    [
        (0, &[(0x00f8, 0x10000000)]),
        (u32::MAX, &[(0x00f8, 0x10000000)])
    ]
);

golden!(
    power_on_rf_circuits,
    PmuRadio,
    pmu,
    |b| {
        ops::power_on_rf_circuits(&mut b);
    },
    [(0, &[(0x0184, 0xffff0000)]), (u32::MAX, &[])]
);

golden!(
    power_off_rf_circuits,
    PmuRadio,
    pmu,
    |b| {
        ops::power_off_rf_circuits(&mut b);
    },
    [(0, &[]), (u32::MAX, &[(0x0184, 0x0000ffff)])]
);

golden!(
    tie_baseband_i2c_power,
    PmuRadio,
    pmu,
    |b| {
        ops::tie_baseband_i2c_power(&mut b);
    },
    [
        (0, &[(0x00f0, 0x10000000)]),
        (u32::MAX, &[(0x00f0, 0x10000000)])
    ]
);

golden!(
    power_on_analog_i2c,
    PmuRadio,
    pmu,
    |b| {
        ops::power_on_analog_i2c(&mut b);
    },
    [(0, &[(0x0208, 0x40000000)]), (u32::MAX, &[])]
);

golden!(
    power_off_analog_i2c,
    PmuRadio,
    pmu,
    |b| {
        ops::power_off_analog_i2c(&mut b);
    },
    [(0, &[]), (u32::MAX, &[(0x0208, 0xbfffffff)])]
);

golden!(
    release_analog_i2c_reset,
    PmuRadio,
    pmu,
    |b| {
        ops::release_analog_i2c_reset(&mut b);
    },
    [(0, &[(0x0208, 0x80000000)]), (u32::MAX, &[])]
);

golden!(
    assert_analog_i2c_reset,
    PmuRadio,
    pmu,
    |b| {
        ops::assert_analog_i2c_reset(&mut b);
    },
    [(0, &[]), (u32::MAX, &[(0x0208, 0x7fffffff)])]
);

golden!(
    enable_frontend_baseband_power,
    PmuRadio,
    pmu,
    |b| {
        ops::enable_frontend_baseband_power(&mut b);
    },
    [(0, &[(0x001c, 0x0040000f)]), (u32::MAX, &[])]
);

golden!(
    initialize_shared_modem_power_state_map,
    ModemLpconPower,
    modem_lpcon,
    |b| {
        ops::initialize_shared_modem_power_state_map(&mut b);
    },
    [(0, &[(0x0020, 0x66660000)]), (u32::MAX, &[])]
);

golden!(
    deselect_bluetooth_low_power_timer_slow_oscillator,
    BluetoothLpTimerClock,
    modem_lpcon,
    |b| {
        ops::deselect_bluetooth_low_power_timer_slow_oscillator(&mut b);
    },
    [(0, &[]), (u32::MAX, &[(0x0004, 0xfffffffe)])]
);

golden!(
    deselect_bluetooth_low_power_timer_fast_oscillator,
    BluetoothLpTimerClock,
    modem_lpcon,
    |b| {
        ops::deselect_bluetooth_low_power_timer_fast_oscillator(&mut b);
    },
    [(0, &[]), (u32::MAX, &[(0x0004, 0xfffffffd)])]
);

golden!(
    deselect_bluetooth_low_power_timer_crystal_32khz,
    BluetoothLpTimerClock,
    modem_lpcon,
    |b| {
        ops::deselect_bluetooth_low_power_timer_crystal_32khz(&mut b);
    },
    [(0, &[]), (u32::MAX, &[(0x0004, 0xfffffff7)])]
);

golden!(
    deselect_bluetooth_low_power_timer_crystal,
    BluetoothLpTimerClock,
    modem_lpcon,
    |b| {
        ops::deselect_bluetooth_low_power_timer_crystal(&mut b);
    },
    [(0, &[]), (u32::MAX, &[(0x0004, 0xfffffffb)])]
);

golden!(
    select_bluetooth_low_power_timer_crystal,
    BluetoothLpTimerClock,
    modem_lpcon,
    |b| {
        ops::select_bluetooth_low_power_timer_crystal(&mut b);
    },
    [(0, &[(0x0004, 0x00000004)]), (u32::MAX, &[])]
);

golden!(
    set_bluetooth_low_power_timer_divider_zero,
    BluetoothLpTimerClock,
    modem_lpcon,
    |b| {
        ops::set_bluetooth_low_power_timer_divider(
            &mut b,
            ModemLowPowerClockDivider::new(0).expect("divider"),
        );
    },
    [(0, &[]), (u32::MAX, &[(0x0004, 0xffff000f)])]
);

golden!(
    set_bluetooth_low_power_timer_divider_a5a,
    BluetoothLpTimerClock,
    modem_lpcon,
    |b| {
        ops::set_bluetooth_low_power_timer_divider(
            &mut b,
            ModemLowPowerClockDivider::new(0xa5a).expect("divider"),
        );
    },
    [
        (0, &[(0x0004, 0x0000a5a0)]),
        (u32::MAX, &[(0x0004, 0xffffa5af)])
    ]
);

golden!(
    deselect_wifi_low_power_clock_slow_oscillator,
    ModemLpconPower,
    modem_lpcon,
    |b| {
        ops::deselect_wifi_low_power_clock_slow_oscillator(&mut b);
    },
    [(0, &[]), (u32::MAX, &[(0x000c, 0xfffffffe)])]
);

golden!(
    deselect_wifi_low_power_clock_fast_oscillator,
    ModemLpconPower,
    modem_lpcon,
    |b| {
        ops::deselect_wifi_low_power_clock_fast_oscillator(&mut b);
    },
    [(0, &[]), (u32::MAX, &[(0x000c, 0xfffffffd)])]
);

golden!(
    deselect_wifi_low_power_clock_crystal_32khz,
    ModemLpconPower,
    modem_lpcon,
    |b| {
        ops::deselect_wifi_low_power_clock_crystal_32khz(&mut b);
    },
    [(0, &[]), (u32::MAX, &[(0x000c, 0xfffffff7)])]
);

golden!(
    deselect_wifi_low_power_clock_crystal,
    ModemLpconPower,
    modem_lpcon,
    |b| {
        ops::deselect_wifi_low_power_clock_crystal(&mut b);
    },
    [(0, &[]), (u32::MAX, &[(0x000c, 0xfffffffb)])]
);

golden!(
    select_wifi_low_power_clock_slow_oscillator,
    ModemLpconPower,
    modem_lpcon,
    |b| {
        ops::select_wifi_low_power_clock_slow_oscillator(&mut b);
    },
    [(0, &[(0x000c, 0x00000001)]), (u32::MAX, &[])]
);

golden!(
    select_wifi_low_power_clock_crystal_32khz,
    ModemLpconPower,
    modem_lpcon,
    |b| {
        ops::select_wifi_low_power_clock_crystal_32khz(&mut b);
    },
    [(0, &[(0x000c, 0x00000008)]), (u32::MAX, &[])]
);

golden!(
    select_modem_32khz_clock_crystal,
    ModemLpconPower,
    modem_lpcon,
    |b| {
        ops::select_modem_32khz_clock_crystal(&mut b);
    },
    [(0, &[]), (u32::MAX, &[(0x0014, 0xfffffffc)])]
);

golden!(
    set_wifi_low_power_clock_divider_zero,
    ModemLpconPower,
    modem_lpcon,
    |b| {
        ops::set_wifi_low_power_clock_divider(
            &mut b,
            ModemLowPowerClockDivider::new(0).expect("divider"),
        );
    },
    [(0, &[]), (u32::MAX, &[(0x000c, 0xffff000f)])]
);

golden!(
    set_wifi_low_power_clock_divider_a5a,
    ModemLpconPower,
    modem_lpcon,
    |b| {
        ops::set_wifi_low_power_clock_divider(
            &mut b,
            ModemLowPowerClockDivider::new(0xa5a).expect("divider"),
        );
    },
    [
        (0, &[(0x000c, 0x0000a5a0)]),
        (u32::MAX, &[(0x000c, 0xffffa5af)])
    ]
);

golden!(
    select_coex_timer_clock_source_s1,
    CoexLpClock,
    modem_lpcon,
    |b| {
        ops::select_coex_timer_clock_source(&mut b, CoexTimerClockSource::Selector1);
    },
    [
        (0, &[(0x0008, 0x00000001)]),
        (u32::MAX, &[(0x0008, 0xfffffff1)])
    ]
);

golden!(
    select_coex_timer_clock_source_s2,
    CoexLpClock,
    modem_lpcon,
    |b| {
        ops::select_coex_timer_clock_source(&mut b, CoexTimerClockSource::Selector2);
    },
    [
        (0, &[(0x0008, 0x00000002)]),
        (u32::MAX, &[(0x0008, 0xfffffff2)])
    ]
);

golden!(
    select_coex_timer_clock_source_s4,
    CoexLpClock,
    modem_lpcon,
    |b| {
        ops::select_coex_timer_clock_source(&mut b, CoexTimerClockSource::Selector4);
    },
    [
        (0, &[(0x0008, 0x00000004)]),
        (u32::MAX, &[(0x0008, 0xfffffff4)])
    ]
);

golden!(
    select_coex_timer_clock_source_s8,
    CoexLpClock,
    modem_lpcon,
    |b| {
        ops::select_coex_timer_clock_source(&mut b, CoexTimerClockSource::Selector8);
    },
    [
        (0, &[(0x0008, 0x00000008)]),
        (u32::MAX, &[(0x0008, 0xfffffff8)])
    ]
);

golden!(
    set_coex_timer_clock_divider_zero,
    CoexLpClock,
    modem_lpcon,
    |b| {
        ops::set_coex_timer_clock_divider(
            &mut b,
            CoexTimerClockDividerMinusOne::new(0).expect("divider"),
        );
    },
    [(0, &[]), (u32::MAX, &[(0x0008, 0xffff000f)])]
);

golden!(
    set_coex_timer_clock_divider_a5,
    CoexLpClock,
    modem_lpcon,
    |b| {
        ops::set_coex_timer_clock_divider(
            &mut b,
            CoexTimerClockDividerMinusOne::new(0xa5).expect("divider"),
        );
    },
    [
        (0, &[(0x0008, 0x00000a50)]),
        (u32::MAX, &[(0x0008, 0xffff0a5f)])
    ]
);

golden!(
    external_mode_leader_one,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b)
            .configure_external_coex_mode(ExternalCoexRole::Leader, ExternalCoexWires::One);
    },
    [
        (0, &[(0x00a0, 0x00005000)]),
        (u32::MAX, &[(0x009c, 0xcfffffff), (0x00a0, 0xfff053ff)])
    ]
);

golden!(
    external_mode_leader_two,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b)
            .configure_external_coex_mode(ExternalCoexRole::Leader, ExternalCoexWires::Two);
    },
    [
        (0, &[(0x00a0, 0x00005000)]),
        (u32::MAX, &[(0x009c, 0xcfffffff), (0x00a0, 0xfff053ff)])
    ]
);

golden!(
    external_mode_leader_three,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b)
            .configure_external_coex_mode(ExternalCoexRole::Leader, ExternalCoexWires::Three);
    },
    [
        (0, &[(0x00a0, 0x00005000)]),
        (u32::MAX, &[(0x009c, 0xcfffffff), (0x00a0, 0xfff053ff)])
    ]
);

golden!(
    external_mode_leader_four,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b)
            .configure_external_coex_mode(ExternalCoexRole::Leader, ExternalCoexWires::Four);
    },
    [
        (0, &[(0x00a0, 0x00005000)]),
        (u32::MAX, &[(0x009c, 0xcfffffff), (0x00a0, 0xfff053ff)])
    ]
);

golden!(
    external_mode_follower_one,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b)
            .configure_external_coex_mode(ExternalCoexRole::Follower, ExternalCoexWires::One);
    },
    [
        (0, &[(0x009c, 0x21110000)]),
        (u32::MAX, &[(0x009c, 0xe111ffff), (0x00a0, 0xfff003ff)])
    ]
);

golden!(
    external_mode_follower_two,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b)
            .configure_external_coex_mode(ExternalCoexRole::Follower, ExternalCoexWires::Two);
    },
    [
        (0, &[(0x009c, 0x218c0000)]),
        (u32::MAX, &[(0x009c, 0xe18cffff), (0x00a0, 0xfff003ff)])
    ]
);

golden!(
    external_mode_follower_three,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b)
            .configure_external_coex_mode(ExternalCoexRole::Follower, ExternalCoexWires::Three);
    },
    [
        (0, &[(0x009c, 0x218c0000)]),
        (u32::MAX, &[(0x009c, 0xe18cffff), (0x00a0, 0xfff003ff)])
    ]
);

golden!(
    external_mode_follower_four,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b)
            .configure_external_coex_mode(ExternalCoexRole::Follower, ExternalCoexWires::Four);
    },
    [
        (0, &[(0x009c, 0x218c0000)]),
        (u32::MAX, &[(0x009c, 0xe18cffff), (0x00a0, 0xfff003ff)])
    ]
);

golden!(
    external_grant_zero_low,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b).configure_external_coex_grant(
            ExternalCoexGrantDelay::new(0).expect("delay"),
            ExternalCoexGrantValidity::ValidLow,
        );
    },
    [(0, &[]), (u32::MAX, &[(0x00a0, 0xfffffe00)])]
);

golden!(
    external_grant_zero_high,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b).configure_external_coex_grant(
            ExternalCoexGrantDelay::new(0).expect("delay"),
            ExternalCoexGrantValidity::ValidHigh,
        );
    },
    [
        (0, &[(0x00a0, 0x00000001)]),
        (u32::MAX, &[(0x00a0, 0xfffffe01)])
    ]
);

golden!(
    external_grant_a_low,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b).configure_external_coex_grant(
            ExternalCoexGrantDelay::new(0xa).expect("delay"),
            ExternalCoexGrantValidity::ValidLow,
        );
    },
    [
        (0, &[(0x00a0, 0x00000140)]),
        (u32::MAX, &[(0x00a0, 0xffffff40)])
    ]
);

golden!(
    external_grant_a_high,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b).configure_external_coex_grant(
            ExternalCoexGrantDelay::new(0xa).expect("delay"),
            ExternalCoexGrantValidity::ValidHigh,
        );
    },
    [
        (0, &[(0x00a0, 0x00000141)]),
        (u32::MAX, &[(0x00a0, 0xffffff41)])
    ]
);

golden!(
    external_priorities_leader_one,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b).publish_external_coex_priorities(
            ExternalCoexRole::Leader,
            ExternalCoexWires::One,
            [
                ExternalCoexPriority::new(1).expect("priority"),
                ExternalCoexPriority::new(0xa).expect("priority"),
                ExternalCoexPriority::new(0x5).expect("priority"),
            ],
        );
    },
    [
        (0, &[(0x009c, 0x000001a5)]),
        (u32::MAX, &[(0x009c, 0xffff01a5)])
    ]
);

golden!(
    external_priorities_leader_two,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b).publish_external_coex_priorities(
            ExternalCoexRole::Leader,
            ExternalCoexWires::Two,
            [
                ExternalCoexPriority::new(1).expect("priority"),
                ExternalCoexPriority::new(0xa).expect("priority"),
                ExternalCoexPriority::new(0x5).expect("priority"),
            ],
        );
    },
    [
        (0, &[(0x009c, 0x000001a5)]),
        (u32::MAX, &[(0x009c, 0xffff01a5)])
    ]
);

golden!(
    external_priorities_leader_three,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b).publish_external_coex_priorities(
            ExternalCoexRole::Leader,
            ExternalCoexWires::Three,
            [
                ExternalCoexPriority::new(1).expect("priority"),
                ExternalCoexPriority::new(0xa).expect("priority"),
                ExternalCoexPriority::new(0x5).expect("priority"),
            ],
        );
    },
    [
        (0, &[(0x009c, 0x000001a5)]),
        (u32::MAX, &[(0x009c, 0xffff01a5)])
    ]
);

golden!(
    external_priorities_leader_four,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b).publish_external_coex_priorities(
            ExternalCoexRole::Leader,
            ExternalCoexWires::Four,
            [
                ExternalCoexPriority::new(1).expect("priority"),
                ExternalCoexPriority::new(0xa).expect("priority"),
                ExternalCoexPriority::new(0x5).expect("priority"),
            ],
        );
    },
    [
        (0, &[(0x009c, 0x000001a5)]),
        (u32::MAX, &[(0x009c, 0xffff01a5)])
    ]
);

golden!(
    external_clear_leader,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b).clear_external_coex_priorities(ExternalCoexRole::Leader);
    },
    [(0, &[]), (u32::MAX, &[(0x009c, 0xffff0000)])]
);

golden!(
    external_priorities_follower_one,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b).publish_external_coex_priorities(
            ExternalCoexRole::Follower,
            ExternalCoexWires::One,
            [
                ExternalCoexPriority::new(1).expect("priority"),
                ExternalCoexPriority::new(0xa).expect("priority"),
                ExternalCoexPriority::new(0x5).expect("priority"),
            ],
        );
    },
    [
        (0, &[(0x009c, 0x000000f0)]),
        (u32::MAX, &[(0x009c, 0xffff00f0)])
    ]
);

golden!(
    external_priorities_follower_two,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b).publish_external_coex_priorities(
            ExternalCoexRole::Follower,
            ExternalCoexWires::Two,
            [
                ExternalCoexPriority::new(1).expect("priority"),
                ExternalCoexPriority::new(0xa).expect("priority"),
                ExternalCoexPriority::new(0x5).expect("priority"),
            ],
        );
    },
    [
        (0, &[(0x009c, 0x0000f0f0)]),
        (u32::MAX, &[(0x009c, 0xfffff0f0)])
    ]
);

golden!(
    external_priorities_follower_three,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b).publish_external_coex_priorities(
            ExternalCoexRole::Follower,
            ExternalCoexWires::Three,
            [
                ExternalCoexPriority::new(1).expect("priority"),
                ExternalCoexPriority::new(0xa).expect("priority"),
                ExternalCoexPriority::new(0x5).expect("priority"),
            ],
        );
    },
    [
        (0, &[(0x009c, 0x0000f0f0)]),
        (u32::MAX, &[(0x009c, 0xfffff0f0)])
    ]
);

golden!(
    external_priorities_follower_four,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b).publish_external_coex_priorities(
            ExternalCoexRole::Follower,
            ExternalCoexWires::Four,
            [
                ExternalCoexPriority::new(1).expect("priority"),
                ExternalCoexPriority::new(0xa).expect("priority"),
                ExternalCoexPriority::new(0x5).expect("priority"),
            ],
        );
    },
    [
        (0, &[(0x009c, 0x000000f0)]),
        (u32::MAX, &[(0x009c, 0xffff00f0)])
    ]
);

golden!(
    external_clear_follower,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b).clear_external_coex_priorities(ExternalCoexRole::Follower);
    },
    [
        (0, &[(0x009c, 0x0fff0000)]),
        (u32::MAX, &[(0x009c, 0xffff0000)])
    ]
);

golden!(
    external_disable,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b).disable_external_coex();
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[
                (0x00a0, 0xfffffdff),
                (0x00a4, 0xffffbfff),
                (0x00d0, 0xfffffff0),
                (0x00d8, 0xfffffff0)
            ]
        )
    ]
);

golden!(
    external_enable,
    CoexExternal,
    coex,
    |b| {
        ExternalCoexControl::new(&mut b).enable_external_coex();
    },
    [
        (
            0,
            &[
                (0x00a0, 0x00000200),
                (0x00a4, 0x00004000),
                (0x00d0, 0x0000000f),
                (0x00d8, 0x0000000f)
            ]
        ),
        (u32::MAX, &[])
    ]
);

/// Run `operation` on the PHY owner over memory filled with `fill` and
/// compare every part with the fill and `changed` words, named by the part,
/// replaced.
fn check_owner(
    fill: u32,
    changed: &[(&str, usize, u32)],
    operation: impl FnOnce(
        &mut crate::radio::phy::registers::RadioPhyRegisters<
            crate::radio::phy::registers::HostPlacement<'_>,
        >,
    ),
) {
    let ((), memory) = crate::radio::phy::registers::host::with_registers_memory(fill, operation);
    for (part, words) in &memory {
        for (index, got) in words.iter().enumerate() {
            let want = changed
                .iter()
                .find(|(name, offset, _)| name == part && offset / 4 == index)
                .map_or(fill, |&(_, _, value)| value);
            assert_eq!(
                *got,
                want,
                "{part} word {:#06x} from fill {fill:#010x}: {got:#010x}, expected {want:#010x}",
                index * 4
            );
        }
    }
}

macro_rules! owner_golden {
    ($name:ident, |$r:ident| $operation:block, [$(($fill:expr, $changed:expr)),* $(,)?] $(,)?) => {
        #[test]
        fn $name() {
            $(
                check_owner($fill, $changed, |$r| $operation);
            )*
        }
    };
}

owner_golden!(
    owner_restore_modem_syscon_power_baseline_disabled,
    |r| {
        r.restore_modem_syscon_power_baseline(super::syscon::ModemSysconPowerBaseline::DISABLED);
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[
                ("modem_syscon_clock_reset", 0x0004, 0xffffefff),
                ("modem_syscon_clock_reset", 0x0014, 0xffc41a00)
            ]
        )
    ]
);

owner_golden!(
    owner_prepare_modem_syscon_clock_map,
    |r| {
        r.prepare_modem_syscon_clock_map();
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x000c, 0x64646400)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_configure_wifi_power_clock_map,
    |r| {
        r.configure_wifi_power_clock_map();
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x000c, 0x64646400)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_enable_phy_calibration_clocks,
    |r| {
        r.enable_phy_calibration_clocks();
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0014, 0x003be5ff)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_select_phy_i2c_160mhz_source,
    |r| {
        r.select_phy_i2c_160mhz_source();
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0004, 0x00001000)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_enable_wifi_mac_clocks,
    |r| {
        r.enable_wifi_mac_clocks();
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0014, 0x000007ff)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_clear_cold_start_wifi_control,
    |r| {
        r.clear_cold_start_wifi_control();
    },
    [
        (0, &[]),
        (u32::MAX, &[("modem_syscon_radio", 0x0018, 0xfffffffc)])
    ]
);

owner_golden!(
    owner_enable_mac_baseband,
    |r| {
        r.enable_mac_baseband();
    },
    [
        (0, &[("modem_syscon_radio", 0x0018, 0x10000002)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_reset_bluetooth_controller_domains,
    |r| {
        r.reset_bluetooth_controller_domains();
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[("modem_syscon_clock_reset", 0x0010, 0x91fe7fff)]
        )
    ]
);

owner_golden!(
    owner_prepare_shared_modem_clock_map,
    |r| {
        r.prepare_shared_modem_clock_map();
    },
    [
        (0, &[("modem_lpcon_power", 0x0020, 0x66660000)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_deselect_bluetooth_low_power_timer_sources,
    |r| {
        r.deselect_bluetooth_low_power_timer_sources();
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[("bluetooth_lp_timer_clock", 0x0004, 0xfffffff0)]
        )
    ]
);

owner_golden!(
    owner_publish_modem_power_state,
    |r| {
        r.publish_modem_power_state();
    },
    [
        (
            0,
            &[
                ("pmu_radio", 0x0014, 0x80000000),
                ("pmu_radio", 0x00f8, 0x10000000),
                ("pmu_radio", 0x0104, 0x80000000)
            ]
        ),
        (
            u32::MAX,
            &[
                ("pmu_radio", 0x0014, 0xbfffffff),
                ("pmu_radio", 0x00f8, 0x10000000),
                ("pmu_radio", 0x0104, 0x80000000)
            ]
        )
    ]
);

owner_golden!(
    owner_select_hp_active_modem_icg,
    |r| {
        r.select_hp_active_modem_icg();
    },
    [
        (0, &[("pmu_radio", 0x0014, 0x80000000)]),
        (u32::MAX, &[("pmu_radio", 0x0014, 0xbfffffff)])
    ]
);

owner_golden!(
    owner_apply_modem_icg_selection,
    |r| {
        r.apply_modem_icg_selection();
    },
    [
        (0, &[("pmu_radio", 0x0104, 0x80000000)]),
        (u32::MAX, &[("pmu_radio", 0x0104, 0x80000000)])
    ]
);

owner_golden!(
    owner_apply_sleep_icg_selection,
    |r| {
        r.apply_sleep_icg_selection();
    },
    [
        (0, &[("pmu_radio", 0x00f8, 0x10000000)]),
        (u32::MAX, &[("pmu_radio", 0x00f8, 0x10000000)])
    ]
);

owner_golden!(
    owner_enable_modem_register_bus_clock,
    |r| {
        r.enable_modem_register_bus_clock();
    },
    [
        (0, &[("hp_sys_clkrst_radio", 0x0040, 0x00000001)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_configure_modem_source_clocks,
    |r| {
        r.configure_modem_source_clocks();
    },
    [
        (0, &[("hp_sys_clkrst_radio", 0x01e0, 0x0000001d)]),
        (u32::MAX, &[("hp_sys_clkrst_radio", 0x01e0, 0x0000001d)])
    ]
);

owner_golden!(
    owner_enable_frontend_baseband_power,
    |r| {
        r.enable_frontend_baseband_power();
    },
    [(0, &[("pmu_radio", 0x001c, 0x0040000f)]), (u32::MAX, &[])]
);

owner_golden!(
    owner_configure_fixed_xtal_40mhz_tick,
    |r| {
        r.configure_fixed_xtal_40mhz_tick();
    },
    [
        (0, &[("modem_lpcon_phy_tick", 0x0028, 0x00000027)]),
        (u32::MAX, &[("modem_lpcon_phy_tick", 0x0028, 0xffffffe7)])
    ]
);

owner_golden!(
    owner_set_wifi_baseband_and_mac_reset_released,
    |r| {
        r.set_wifi_baseband_and_mac_reset(oer_chips_esp32s31_pac::dictionaries::Reset::Released);
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[("modem_syscon_clock_reset", 0x0010, 0xfffffcff)]
        )
    ]
);

owner_golden!(
    owner_set_wifi_baseband_and_mac_reset_asserted,
    |r| {
        r.set_wifi_baseband_and_mac_reset(oer_chips_esp32s31_pac::dictionaries::Reset::Asserted);
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0010, 0x00000300)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_set_wifi_baseband_reset_released,
    |r| {
        r.set_wifi_baseband_reset(oer_chips_esp32s31_pac::dictionaries::Reset::Released);
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[("modem_syscon_clock_reset", 0x0010, 0xfffffeff)]
        )
    ]
);

owner_golden!(
    owner_set_wifi_baseband_reset_asserted,
    |r| {
        r.set_wifi_baseband_reset(oer_chips_esp32s31_pac::dictionaries::Reset::Asserted);
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0010, 0x00000100)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_set_wifi_mac_reset_released,
    |r| {
        r.set_wifi_mac_reset(oer_chips_esp32s31_pac::dictionaries::Reset::Released);
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[("modem_syscon_clock_reset", 0x0010, 0xfffffdff)]
        )
    ]
);

owner_golden!(
    owner_set_wifi_mac_reset_asserted,
    |r| {
        r.set_wifi_mac_reset(oer_chips_esp32s31_pac::dictionaries::Reset::Asserted);
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0010, 0x00000200)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_set_wifi_baseband_enabled_disabled,
    |r| {
        r.set_wifi_baseband_enabled(
            oer_chips_esp32s31_pac::modem_syscon::wifi_bb_cfg::WifiEnable::Disabled,
        );
    },
    [
        (0, &[]),
        (u32::MAX, &[("modem_syscon_radio", 0x0018, 0xfffffffd)])
    ]
);

owner_golden!(
    owner_set_mac_baseband_enabled_disabled,
    |r| {
        r.set_mac_baseband_enabled(
            oer_chips_esp32s31_pac::modem_syscon::wifi_bb_cfg::Bit28Opaque::Disabled,
        );
    },
    [
        (0, &[]),
        (u32::MAX, &[("modem_syscon_radio", 0x0018, 0xefffffff)])
    ]
);

owner_golden!(
    owner_set_wifi_baseband_enabled_enabled,
    |r| {
        r.set_wifi_baseband_enabled(
            oer_chips_esp32s31_pac::modem_syscon::wifi_bb_cfg::WifiEnable::Enabled,
        );
    },
    [
        (0, &[("modem_syscon_radio", 0x0018, 0x00000002)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_set_mac_baseband_enabled_enabled,
    |r| {
        r.set_mac_baseband_enabled(
            oer_chips_esp32s31_pac::modem_syscon::wifi_bb_cfg::Bit28Opaque::Enabled,
        );
    },
    [
        (0, &[("modem_syscon_radio", 0x0018, 0x10000000)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_power_off_rf_circuits,
    |r| {
        r.power_off_rf_circuits();
    },
    [(0, &[]), (u32::MAX, &[("pmu_radio", 0x0184, 0x0000ffff)])]
);

owner_golden!(
    owner_power_off_analog_i2c,
    |r| {
        r.power_off_analog_i2c();
    },
    [(0, &[]), (u32::MAX, &[("pmu_radio", 0x0208, 0xbfffffff)])]
);

owner_golden!(
    owner_assert_analog_i2c_reset,
    |r| {
        r.assert_analog_i2c_reset();
    },
    [(0, &[]), (u32::MAX, &[("pmu_radio", 0x0208, 0x7fffffff)])]
);

owner_golden!(
    owner_power_on_rf_circuits,
    |r| {
        r.power_on_rf_circuits();
    },
    [(0, &[("pmu_radio", 0x0184, 0xffff0000)]), (u32::MAX, &[])]
);

owner_golden!(
    owner_tie_baseband_i2c_power,
    |r| {
        r.tie_baseband_i2c_power();
    },
    [
        (0, &[("pmu_radio", 0x00f0, 0x10000000)]),
        (u32::MAX, &[("pmu_radio", 0x00f0, 0x10000000)])
    ]
);

owner_golden!(
    owner_power_on_analog_i2c,
    |r| {
        r.power_on_analog_i2c();
    },
    [(0, &[("pmu_radio", 0x0208, 0x40000000)]), (u32::MAX, &[])]
);

owner_golden!(
    owner_release_analog_i2c_reset,
    |r| {
        r.release_analog_i2c_reset();
    },
    [(0, &[("pmu_radio", 0x0208, 0x80000000)]), (u32::MAX, &[])]
);

owner_golden!(
    owner_set_bb_agc_update_mode_initialization,
    |r| {
        r.set_bb_agc_update_mode(super::WifiBasebandAgcUpdate::Initialization);
    },
    [
        (0, &[("modem_syscon_radio", 0x0018, 0x00000800)]),
        (u32::MAX, &[("modem_syscon_radio", 0x0018, 0xffffcfff)])
    ]
);

owner_golden!(
    owner_set_bb_agc_update_mode_updates,
    |r| {
        r.set_bb_agc_update_mode(super::WifiBasebandAgcUpdate::RegisterUpdatesEnabled);
    },
    [
        (0, &[("modem_syscon_radio", 0x0018, 0x00003800)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_enable_bluetooth_clock_wifi_baseband_80x1,
    |r| {
        r.enable_bluetooth_clock(super::ModemSysconBluetoothClock::WifiBaseband80x1);
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0014, 0x00000080)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_restore_bluetooth_clock_wifi_baseband_80x1_disabled,
    |r| {
        r.restore_bluetooth_clock(
            super::ModemSysconBluetoothClock::WifiBaseband80x1,
            super::BluetoothClockBaseline::default(),
        );
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[("modem_syscon_clock_reset", 0x0014, 0xffffff7f)]
        )
    ]
);

owner_golden!(
    owner_restore_bluetooth_clock_wifi_baseband_80x1_enabled,
    |r| {
        r.restore_bluetooth_clock(
            super::ModemSysconBluetoothClock::WifiBaseband80x1,
            super::BluetoothClockBaseline::all_enabled_for_host_test(),
        );
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0014, 0x00000080)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_enable_bluetooth_clock_etm,
    |r| {
        r.enable_bluetooth_clock(super::ModemSysconBluetoothClock::Etm);
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0004, 0x00400000)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_restore_bluetooth_clock_etm_disabled,
    |r| {
        r.restore_bluetooth_clock(
            super::ModemSysconBluetoothClock::Etm,
            super::BluetoothClockBaseline::default(),
        );
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[("modem_syscon_clock_reset", 0x0004, 0xffbfffff)]
        )
    ]
);

owner_golden!(
    owner_restore_bluetooth_clock_etm_enabled,
    |r| {
        r.restore_bluetooth_clock(
            super::ModemSysconBluetoothClock::Etm,
            super::BluetoothClockBaseline::all_enabled_for_host_test(),
        );
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0004, 0x00400000)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_enable_bluetooth_clock_bluetooth_mac,
    |r| {
        r.enable_bluetooth_clock(super::ModemSysconBluetoothClock::BluetoothMac);
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0014, 0x00040000)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_restore_bluetooth_clock_bluetooth_mac_disabled,
    |r| {
        r.restore_bluetooth_clock(
            super::ModemSysconBluetoothClock::BluetoothMac,
            super::BluetoothClockBaseline::default(),
        );
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[("modem_syscon_clock_reset", 0x0014, 0xfffbffff)]
        )
    ]
);

owner_golden!(
    owner_restore_bluetooth_clock_bluetooth_mac_enabled,
    |r| {
        r.restore_bluetooth_clock(
            super::ModemSysconBluetoothClock::BluetoothMac,
            super::BluetoothClockBaseline::all_enabled_for_host_test(),
        );
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0014, 0x00040000)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_enable_bluetooth_clock_bluetooth_peripheral,
    |r| {
        r.enable_bluetooth_clock(super::ModemSysconBluetoothClock::BluetoothPeripheral);
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0004, 0x6e000000)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_restore_bluetooth_clock_bluetooth_peripheral_disabled,
    |r| {
        r.restore_bluetooth_clock(
            super::ModemSysconBluetoothClock::BluetoothPeripheral,
            super::BluetoothClockBaseline::default(),
        );
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[("modem_syscon_clock_reset", 0x0004, 0x91ffffff)]
        )
    ]
);

owner_golden!(
    owner_restore_bluetooth_clock_bluetooth_peripheral_enabled,
    |r| {
        r.restore_bluetooth_clock(
            super::ModemSysconBluetoothClock::BluetoothPeripheral,
            super::BluetoothClockBaseline::all_enabled_for_host_test(),
        );
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0004, 0x6e000000)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_enable_bluetooth_clock_bluetooth_apb,
    |r| {
        r.enable_bluetooth_clock(super::ModemSysconBluetoothClock::BluetoothApb);
    },
    [
        (
            0,
            &[
                ("modem_syscon_clock_reset", 0x0004, 0x10000000),
                ("modem_syscon_clock_reset", 0x0014, 0x00010000)
            ]
        ),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_restore_bluetooth_clock_bluetooth_apb_disabled,
    |r| {
        r.restore_bluetooth_clock(
            super::ModemSysconBluetoothClock::BluetoothApb,
            super::BluetoothClockBaseline::default(),
        );
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[
                ("modem_syscon_clock_reset", 0x0004, 0xefffffff),
                ("modem_syscon_clock_reset", 0x0014, 0xfffeffff)
            ]
        )
    ]
);

owner_golden!(
    owner_restore_bluetooth_clock_bluetooth_apb_enabled,
    |r| {
        r.restore_bluetooth_clock(
            super::ModemSysconBluetoothClock::BluetoothApb,
            super::BluetoothClockBaseline::all_enabled_for_host_test(),
        );
    },
    [
        (
            0,
            &[
                ("modem_syscon_clock_reset", 0x0004, 0x10000000),
                ("modem_syscon_clock_reset", 0x0014, 0x00010000)
            ]
        ),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_enable_bluetooth_clock_bluetooth_baseband,
    |r| {
        r.enable_bluetooth_clock(super::ModemSysconBluetoothClock::BluetoothBaseband);
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0014, 0x00020000)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_restore_bluetooth_clock_bluetooth_baseband_disabled,
    |r| {
        r.restore_bluetooth_clock(
            super::ModemSysconBluetoothClock::BluetoothBaseband,
            super::BluetoothClockBaseline::default(),
        );
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[("modem_syscon_clock_reset", 0x0014, 0xfffdffff)]
        )
    ]
);

owner_golden!(
    owner_restore_bluetooth_clock_bluetooth_baseband_enabled,
    |r| {
        r.restore_bluetooth_clock(
            super::ModemSysconBluetoothClock::BluetoothBaseband,
            super::BluetoothClockBaseline::all_enabled_for_host_test(),
        );
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0014, 0x00020000)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_select_wifi_low_power_clock_slow_oscillator,
    |r| {
        r.select_wifi_low_power_clock(
            oer_chips_family_radio_power::clocks::WifiLowPowerClockSource::SlowOscillator,
        );
    },
    [
        (0, &[("modem_lpcon_power", 0x000c, 0x00000001)]),
        (u32::MAX, &[("modem_lpcon_power", 0x000c, 0xffff0001)])
    ]
);

owner_golden!(
    owner_select_wifi_low_power_clock_crystal32_khz,
    |r| {
        r.select_wifi_low_power_clock(
            oer_chips_family_radio_power::clocks::WifiLowPowerClockSource::Crystal32Khz,
        );
    },
    [
        (0, &[("modem_lpcon_power", 0x000c, 0x00000008)]),
        (
            u32::MAX,
            &[
                ("modem_lpcon_power", 0x000c, 0xffff0008),
                ("modem_lpcon_power", 0x0014, 0xfffffffc)
            ]
        )
    ]
);

owner_golden!(
    owner_configure_coexistence_timer_clock_selector1,
    |r| {
        r.configure_coexistence_timer_clock(
            CoexTimerClockSource::Selector1,
            CoexTimerClockDividerMinusOne::new(0xa5).expect("divider"),
        );
    },
    [
        (0, &[("coex_lp_clock", 0x0008, 0x00000a51)]),
        (u32::MAX, &[("coex_lp_clock", 0x0008, 0xffff0a51)])
    ]
);

owner_golden!(
    owner_configure_coexistence_timer_clock_selector2,
    |r| {
        r.configure_coexistence_timer_clock(
            CoexTimerClockSource::Selector2,
            CoexTimerClockDividerMinusOne::new(0xa5).expect("divider"),
        );
    },
    [
        (0, &[("coex_lp_clock", 0x0008, 0x00000a52)]),
        (u32::MAX, &[("coex_lp_clock", 0x0008, 0xffff0a52)])
    ]
);

owner_golden!(
    owner_configure_coexistence_timer_clock_selector4,
    |r| {
        r.configure_coexistence_timer_clock(
            CoexTimerClockSource::Selector4,
            CoexTimerClockDividerMinusOne::new(0xa5).expect("divider"),
        );
    },
    [
        (0, &[("coex_lp_clock", 0x0008, 0x00000a54)]),
        (u32::MAX, &[("coex_lp_clock", 0x0008, 0xffff0a54)])
    ]
);

owner_golden!(
    owner_configure_coexistence_timer_clock_selector8,
    |r| {
        r.configure_coexistence_timer_clock(
            CoexTimerClockSource::Selector8,
            CoexTimerClockDividerMinusOne::new(0xa5).expect("divider"),
        );
    },
    [
        (0, &[("coex_lp_clock", 0x0008, 0x00000a58)]),
        (u32::MAX, &[("coex_lp_clock", 0x0008, 0xffff0a58)])
    ]
);

owner_golden!(
    owner_select_bluetooth_low_power_timer_main_crystal_zero,
    |r| {
        r.select_bluetooth_low_power_timer_main_crystal(
            ModemLowPowerClockDivider::new(0).expect("divider"),
        );
    },
    [
        (0, &[("bluetooth_lp_timer_clock", 0x0004, 0x00000004)]),
        (
            u32::MAX,
            &[("bluetooth_lp_timer_clock", 0x0004, 0xffff0004)]
        )
    ]
);

owner_golden!(
    owner_select_bluetooth_low_power_timer_main_crystal_a5a,
    |r| {
        r.select_bluetooth_low_power_timer_main_crystal(
            ModemLowPowerClockDivider::new(0xa5a).expect("divider"),
        );
    },
    [
        (0, &[("bluetooth_lp_timer_clock", 0x0004, 0x0000a5a4)]),
        (
            u32::MAX,
            &[("bluetooth_lp_timer_clock", 0x0004, 0xffffa5a4)]
        )
    ]
);

owner_golden!(
    owner_restore_platform_pll_source_baseline_default,
    |r| {
        r.restore_platform_pll_source_baseline(super::PlatformPllSourceBaseline::default());
    },
    [
        (0, &[]),
        (u32::MAX, &[("hp_sys_clkrst_radio", 0x01e0, 0xffffffc2)])
    ]
);

owner_golden!(
    owner_disable_modem_clock_device_modem_adc_common_fe,
    |r| {
        r.disable_modem_clock_device(super::ModemClockDevice::ModemAdcCommonFe);
    },
    [(0, &[]), (u32::MAX, &[])]
);

owner_golden!(
    owner_enable_modem_clock_device_modem_adc_common_fe,
    |r| {
        r.enable_modem_clock_device(super::ModemClockDevice::ModemAdcCommonFe);
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0014, 0x0000a000)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_disable_modem_clock_device_modem_private_fe,
    |r| {
        r.disable_modem_clock_device(super::ModemClockDevice::ModemPrivateFe);
    },
    [(0, &[]), (u32::MAX, &[])]
);

owner_golden!(
    owner_enable_modem_clock_device_modem_private_fe,
    |r| {
        r.enable_modem_clock_device(super::ModemClockDevice::ModemPrivateFe);
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0014, 0x00384000)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_disable_modem_clock_device_pll_source_gate,
    |r| {
        r.disable_modem_clock_device(super::ModemClockDevice::PllSourceGate);
    },
    [
        (0, &[("hp_sys_clkrst_radio", 0x01e0, 0x00000025)]),
        (u32::MAX, &[("hp_sys_clkrst_radio", 0x01e0, 0x00000025)])
    ]
);

owner_golden!(
    owner_enable_modem_clock_device_pll_source_gate,
    |r| {
        r.enable_modem_clock_device(super::ModemClockDevice::PllSourceGate);
    },
    [
        (0, &[("hp_sys_clkrst_radio", 0x01e0, 0x0000001d)]),
        (u32::MAX, &[("hp_sys_clkrst_radio", 0x01e0, 0x0000001d)])
    ]
);

owner_golden!(
    owner_disable_modem_clock_device_wifi_apb,
    |r| {
        r.disable_modem_clock_device(super::ModemClockDevice::WifiApb);
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[("modem_syscon_clock_reset", 0x0014, 0xfffffbff)]
        )
    ]
);

owner_golden!(
    owner_enable_modem_clock_device_wifi_apb,
    |r| {
        r.enable_modem_clock_device(super::ModemClockDevice::WifiApb);
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0014, 0x00000400)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_disable_modem_clock_device_wifi_baseband44m,
    |r| {
        r.disable_modem_clock_device(super::ModemClockDevice::WifiBaseband44m);
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[("modem_syscon_clock_reset", 0x0014, 0xfffffffb)]
        )
    ]
);

owner_golden!(
    owner_enable_modem_clock_device_wifi_baseband44m,
    |r| {
        r.enable_modem_clock_device(super::ModemClockDevice::WifiBaseband44m);
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0014, 0x00000004)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_disable_modem_clock_device_wifi_mac,
    |r| {
        r.disable_modem_clock_device(super::ModemClockDevice::WifiMac);
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[("modem_syscon_clock_reset", 0x0014, 0xfffffdff)]
        )
    ]
);

owner_golden!(
    owner_enable_modem_clock_device_wifi_mac,
    |r| {
        r.enable_modem_clock_device(super::ModemClockDevice::WifiMac);
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0014, 0x00000200)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_disable_modem_clock_device_wifi_baseband,
    |r| {
        r.disable_modem_clock_device(super::ModemClockDevice::WifiBaseband);
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[("modem_syscon_clock_reset", 0x0014, 0xfffffe84)]
        )
    ]
);

owner_golden!(
    owner_enable_modem_clock_device_wifi_baseband,
    |r| {
        r.enable_modem_clock_device(super::ModemClockDevice::WifiBaseband);
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0014, 0x0000017b)]),
        (
            u32::MAX,
            &[("modem_syscon_clock_reset", 0x0010, 0xfffffeff)]
        )
    ]
);

owner_golden!(
    owner_disable_modem_clock_device_wifi_baseband_80x1,
    |r| {
        r.disable_modem_clock_device(super::ModemClockDevice::WifiBaseband80x1);
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[("modem_syscon_clock_reset", 0x0014, 0xffffff7f)]
        )
    ]
);

owner_golden!(
    owner_enable_modem_clock_device_wifi_baseband_80x1,
    |r| {
        r.enable_modem_clock_device(super::ModemClockDevice::WifiBaseband80x1);
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0014, 0x00000080)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_disable_modem_clock_device_etm,
    |r| {
        r.disable_modem_clock_device(super::ModemClockDevice::Etm);
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[("modem_syscon_clock_reset", 0x0004, 0xffbfffff)]
        )
    ]
);

owner_golden!(
    owner_enable_modem_clock_device_etm,
    |r| {
        r.enable_modem_clock_device(super::ModemClockDevice::Etm);
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0004, 0x00400000)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_disable_modem_clock_device_bluetooth_mac,
    |r| {
        r.disable_modem_clock_device(super::ModemClockDevice::BluetoothMac);
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[("modem_syscon_clock_reset", 0x0014, 0xfffbffff)]
        )
    ]
);

owner_golden!(
    owner_enable_modem_clock_device_bluetooth_mac,
    |r| {
        r.enable_modem_clock_device(super::ModemClockDevice::BluetoothMac);
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0014, 0x00040000)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_disable_modem_clock_device_bluetooth_peripheral,
    |r| {
        r.disable_modem_clock_device(super::ModemClockDevice::BluetoothPeripheral);
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[("modem_syscon_clock_reset", 0x0004, 0x91ffffff)]
        )
    ]
);

owner_golden!(
    owner_enable_modem_clock_device_bluetooth_peripheral,
    |r| {
        r.enable_modem_clock_device(super::ModemClockDevice::BluetoothPeripheral);
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0004, 0x6e000000)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_disable_modem_clock_device_bluetooth_apb,
    |r| {
        r.disable_modem_clock_device(super::ModemClockDevice::BluetoothApb);
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[
                ("modem_syscon_clock_reset", 0x0004, 0xefffffff),
                ("modem_syscon_clock_reset", 0x0014, 0xfffeffff)
            ]
        )
    ]
);

owner_golden!(
    owner_enable_modem_clock_device_bluetooth_apb,
    |r| {
        r.enable_modem_clock_device(super::ModemClockDevice::BluetoothApb);
    },
    [
        (
            0,
            &[
                ("modem_syscon_clock_reset", 0x0004, 0x10000000),
                ("modem_syscon_clock_reset", 0x0014, 0x00010000)
            ]
        ),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_disable_modem_clock_device_bluetooth_ieee802154_common_baseband,
    |r| {
        r.disable_modem_clock_device(super::ModemClockDevice::BluetoothIeee802154CommonBaseband);
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[("modem_syscon_clock_reset", 0x0014, 0xfffdffff)]
        )
    ]
);

owner_golden!(
    owner_enable_modem_clock_device_bluetooth_ieee802154_common_baseband,
    |r| {
        r.enable_modem_clock_device(super::ModemClockDevice::BluetoothIeee802154CommonBaseband);
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0014, 0x00020000)]),
        (u32::MAX, &[])
    ]
);

owner_golden!(
    owner_disable_modem_clock_device_ieee802154_mac,
    |r| {
        r.disable_modem_clock_device(super::ModemClockDevice::Ieee802154Mac);
    },
    [
        (0, &[]),
        (
            u32::MAX,
            &[("modem_syscon_clock_reset", 0x0004, 0xfe7fffff)]
        )
    ]
);

owner_golden!(
    owner_enable_modem_clock_device_ieee802154_mac,
    |r| {
        r.enable_modem_clock_device(super::ModemClockDevice::Ieee802154Mac);
    },
    [
        (0, &[("modem_syscon_clock_reset", 0x0004, 0x01800000)]),
        (u32::MAX, &[])
    ]
);
