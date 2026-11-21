//! Route-owned MODEM_SYSCON radio clock, reset and baseband control.
//!
//! Register geometry remains in the generated PAC. This module publishes only
//! semantic operations through the affine radio route and preserves every
//! vendor RMW edge and protocol-specific order.

#![forbid(unsafe_code)]

use oer_chips_esp32s31_pac::modem_syscon::{
    ModemSysconClockReset,
    clk_conf::ClkI2cMstSel160m,
    dictionaries::{ClockGate, Reset},
    wifi_bb_cfg::{Bit28Opaque, WifiEnable},
};
use oer_portable_mmio::Placement;

use crate::{
    radio::modem::values::{WifiBaseband40MhzState, WifiBasebandAgcUpdateMode},
    radio::phy::RadioPhyRegisters,
};

/// The eighteen PHY-calibration clock gates of `CLK_CONF1`, from one read,
/// in the order [`restore_calibration_clocks`] writes them.
fn calibration_clocks<P: Placement>(registers: &ModemSysconClockReset<P>) -> [bool; 18] {
    let clk_conf1 = registers.read_clk_conf1();
    [
        matches!(
            clk_conf1.clk_wifibb_22m_en(),
            oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
        ),
        matches!(
            clk_conf1.clk_wifibb_40m_en(),
            oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
        ),
        matches!(
            clk_conf1.clk_wifibb_44m_en(),
            oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
        ),
        matches!(
            clk_conf1.clk_wifibb_80m_en(),
            oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
        ),
        matches!(
            clk_conf1.clk_wifibb_40x_en(),
            oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
        ),
        matches!(
            clk_conf1.clk_wifibb_80x_en(),
            oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
        ),
        matches!(
            clk_conf1.clk_wifibb_40x1_en(),
            oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
        ),
        matches!(
            clk_conf1.clk_wifibb_80x1_en(),
            oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
        ),
        matches!(
            clk_conf1.clk_wifibb_160x1_en(),
            oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
        ),
        matches!(
            clk_conf1.clk_wifi_apb_en(),
            oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
        ),
        clk_conf1.clk_fe_80m_en(),
        clk_conf1.clk_fe_160m_en(),
        clk_conf1.clk_fe_apb_en(),
        matches!(
            clk_conf1.clk_bt_apb_en(),
            oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
        ),
        matches!(
            clk_conf1.clk_btbb_en(),
            oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
        ),
        clk_conf1.clk_fe_pwdet_adc_en(),
        clk_conf1.clk_fe_adc_en(),
        clk_conf1.clk_fe_dac_en(),
    ]
}

/// One fresh-read RMW of the eighteen captured gates; every other bit of
/// `CLK_CONF1` keeps the value read.
fn restore_calibration_clocks<P: Placement>(
    registers: &mut ModemSysconClockReset<P>,
    clocks: [bool; 18],
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_wifibb_22m_en(if clocks[0] {
            ClockGate::Enabled
        } else {
            ClockGate::Disabled
        })
        .set_clk_wifibb_40m_en(if clocks[1] {
            ClockGate::Enabled
        } else {
            ClockGate::Disabled
        })
        .set_clk_wifibb_44m_en(if clocks[2] {
            ClockGate::Enabled
        } else {
            ClockGate::Disabled
        })
        .set_clk_wifibb_80m_en(if clocks[3] {
            ClockGate::Enabled
        } else {
            ClockGate::Disabled
        })
        .set_clk_wifibb_40x_en(if clocks[4] {
            ClockGate::Enabled
        } else {
            ClockGate::Disabled
        })
        .set_clk_wifibb_80x_en(if clocks[5] {
            ClockGate::Enabled
        } else {
            ClockGate::Disabled
        })
        .set_clk_wifibb_40x1_en(if clocks[6] {
            ClockGate::Enabled
        } else {
            ClockGate::Disabled
        })
        .set_clk_wifibb_80x1_en(if clocks[7] {
            ClockGate::Enabled
        } else {
            ClockGate::Disabled
        })
        .set_clk_wifibb_160x1_en(if clocks[8] {
            ClockGate::Enabled
        } else {
            ClockGate::Disabled
        })
        .set_clk_wifi_apb_en(if clocks[9] {
            ClockGate::Enabled
        } else {
            ClockGate::Disabled
        })
        .set_clk_fe_80m_en(clocks[10])
        .set_clk_fe_160m_en(clocks[11])
        .set_clk_fe_apb_en(clocks[12])
        .set_clk_bt_apb_en(if clocks[13] {
            ClockGate::Enabled
        } else {
            ClockGate::Disabled
        })
        .set_clk_btbb_en(if clocks[14] {
            ClockGate::Enabled
        } else {
            ClockGate::Disabled
        })
        .set_clk_fe_pwdet_adc_en(clocks[15])
        .set_clk_fe_adc_en(clocks[16])
        .set_clk_fe_dac_en(clocks[17]);
    });
}

/// One fresh-read RMW of the five Bluetooth peripheral gates of
/// `CLK_CONF`: modem security, ECB, CCM, BAH and the BLE timer.
fn restore_bluetooth_peripheral_clocks<P: Placement>(
    registers: &mut ModemSysconClockReset<P>,
    [security, ecb, ccm, bah, ble_timer]: [bool; 5],
) {
    registers.modify_clk_conf(|w| {
        w.set_clk_modem_sec_en(if security {
            ClockGate::Enabled
        } else {
            ClockGate::Disabled
        })
        .set_clk_modem_sec_ecb_en(if ecb {
            ClockGate::Enabled
        } else {
            ClockGate::Disabled
        })
        .set_clk_modem_sec_ccm_en(if ccm {
            ClockGate::Enabled
        } else {
            ClockGate::Disabled
        })
        .set_clk_modem_sec_bah_en(if bah {
            ClockGate::Enabled
        } else {
            ClockGate::Disabled
        })
        .set_clk_ble_timer_en(if ble_timer {
            ClockGate::Enabled
        } else {
            ClockGate::Disabled
        });
    });
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ModemSysconPowerObservation {
    pub wifi_reset_released: bool,
    pub active_clock_map_configured: bool,
    pub phy_calibration_clocks_enabled: bool,
    pub phy_i2c_160mhz_selected: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ModemSysconPowerBaseline {
    phy_calibration_clocks: [bool; 18],
    phy_i2c_160mhz: ClkI2cMstSel160m,
}

impl ModemSysconPowerBaseline {
    pub(super) const BIT_COUNT: u32 = 19;
    /// Every gate disabled: the baseline of a host test.
    pub(super) const DISABLED: Self = Self {
        phy_calibration_clocks: [false; 18],
        phy_i2c_160mhz: ClkI2cMstSel160m::Default,
    };

    pub(super) fn bits(self) -> u32 {
        let mut bits = 0;
        let mut clock = 0;
        while clock < self.phy_calibration_clocks.len() {
            bits |= u32::from(self.phy_calibration_clocks[clock]) << clock;
            clock += 1;
        }
        bits | (u32::from(matches!(self.phy_i2c_160mhz, ClkI2cMstSel160m::I2cMst160m))
            << self.phy_calibration_clocks.len())
    }

    pub(super) fn from_bits(bits: u32) -> Self {
        let mut phy_calibration_clocks = [false; 18];
        let mut clock = 0;
        while clock < phy_calibration_clocks.len() {
            phy_calibration_clocks[clock] = bits & (1 << clock) != 0;
            clock += 1;
        }
        Self {
            phy_calibration_clocks,
            phy_i2c_160mhz: ClkI2cMstSel160m::from_field(bits >> phy_calibration_clocks.len()),
        }
    }
}

/// Release state of the two private IEEE 802.15.4 MAC resets.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ModemSysconIeee802154ResetObservation {
    pub mac_reset_released: bool,
    pub apb_reset_released: bool,
}

/// One logical Bluetooth MODEM_SYSCON clock group.
///
/// Each group owns a disjoint set of physical clock gates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModemSysconBluetoothClock {
    WifiBaseband80x1,
    Etm,
    BluetoothMac,
    BluetoothPeripheral,
    BluetoothApb,
    BluetoothBaseband,
}

/// Number of logical Bluetooth clock groups.
pub const BLUETOOTH_CLOCK_COUNT: usize = 6;
const BLUETOOTH_PHYSICAL_CLOCK_COUNT: usize = 11;
/// Logical groups reported by the controller-clock observation.
pub const BLUETOOTH_CONTROLLER_CLOCKS: [ModemSysconBluetoothClock; 6] = [
    ModemSysconBluetoothClock::WifiBaseband80x1,
    ModemSysconBluetoothClock::Etm,
    ModemSysconBluetoothClock::BluetoothMac,
    ModemSysconBluetoothClock::BluetoothPeripheral,
    ModemSysconBluetoothClock::BluetoothApb,
    ModemSysconBluetoothClock::BluetoothBaseband,
];
/// Logical groups reported by the APB-clock observation.
pub const BLUETOOTH_APB_CLOCKS: [ModemSysconBluetoothClock; 3] = [
    ModemSysconBluetoothClock::Etm,
    ModemSysconBluetoothClock::BluetoothMac,
    ModemSysconBluetoothClock::BluetoothApb,
];

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ModemSysconBluetoothObservation {
    pub controller_clocks_enabled: bool,
    pub apb_clocks_enabled: bool,
    pub controller_resets_released: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiBasebandAgcUpdate {
    Initialization,
    RegisterUpdatesEnabled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BluetoothPhysicalClock {
    WifiBaseband80x1,
    Etm,
    BluetoothMac,
    ModemSecurity,
    ModemSecurityEcb,
    ModemSecurityCcm,
    ModemSecurityBah,
    BleTimer,
    BluetoothApb,
    ModemSecurityApb,
    BluetoothBaseband,
}

/// Opaque readback of the physical gates of Bluetooth clock groups.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BluetoothClockBaseline([bool; BLUETOOTH_PHYSICAL_CLOCK_COUNT]);

impl Default for BluetoothClockBaseline {
    fn default() -> Self {
        Self([false; BLUETOOTH_PHYSICAL_CLOCK_COUNT])
    }
}

impl BluetoothClockBaseline {
    const fn contains(self, clock: BluetoothPhysicalClock) -> bool {
        self.0[clock as usize]
    }

    fn record(&mut self, clock: BluetoothPhysicalClock, enabled: bool) {
        self.0[clock as usize] = enabled;
    }

    /// A baseline with every physical gate enabled, for host tests.
    #[cfg(test)]
    #[doc(hidden)]
    pub const fn all_enabled_for_host_test() -> Self {
        Self([true; BLUETOOTH_PHYSICAL_CLOCK_COUNT])
    }

    /// Whether every physical gate of `clock` was enabled.
    pub const fn all_enabled(self, clock: ModemSysconBluetoothClock) -> bool {
        let clocks = clock.physical_clocks();
        let mut index = 0;
        while index < clocks.len() {
            if !self.contains(clocks[index]) {
                return false;
            }
            index += 1;
        }
        true
    }
}

impl ModemSysconBluetoothClock {
    /// Position of this group in [`RadioPhyRegisters::bluetooth_clock_baselines`].
    pub const fn index(self) -> usize {
        match self {
            Self::WifiBaseband80x1 => 0,
            Self::Etm => 1,
            Self::BluetoothMac => 2,
            Self::BluetoothPeripheral => 3,
            Self::BluetoothApb => 4,
            Self::BluetoothBaseband => 5,
        }
    }

    const fn physical_clocks(self) -> &'static [BluetoothPhysicalClock] {
        use BluetoothPhysicalClock as Physical;
        match self {
            Self::WifiBaseband80x1 => &[Physical::WifiBaseband80x1],
            Self::Etm => &[Physical::Etm],
            Self::BluetoothMac => &[Physical::BluetoothMac],
            Self::BluetoothPeripheral => &[
                Physical::ModemSecurity,
                Physical::ModemSecurityEcb,
                Physical::ModemSecurityCcm,
                Physical::ModemSecurityBah,
                Physical::BleTimer,
            ],
            Self::BluetoothApb => &[Physical::BluetoothApb, Physical::ModemSecurityApb],
            Self::BluetoothBaseband => &[Physical::BluetoothBaseband],
        }
    }
}

impl<P: crate::radio::phy::registers::RadioPlacement> RadioPhyRegisters<P> {
    pub(crate) fn modem_syscon_power_baseline(&self) -> ModemSysconPowerBaseline {
        ModemSysconPowerBaseline {
            phy_calibration_clocks: calibration_clocks(&self.peripherals.modem_syscon_clock_reset),
            phy_i2c_160mhz: self
                .peripherals
                .modem_syscon_clock_reset
                .read_clk_conf()
                .clk_i2c_mst_sel_160m(),
        }
    }

    pub(crate) fn restore_modem_syscon_power_baseline(
        &mut self,
        baseline: ModemSysconPowerBaseline,
    ) {
        let registers = &mut self.peripherals.modem_syscon_clock_reset;
        super::ops::restore_phy_i2c_160mhz_source(registers, baseline.phy_i2c_160mhz);
        restore_calibration_clocks(registers, baseline.phy_calibration_clocks);
    }

    #[doc(hidden)]
    pub fn prepare_modem_syscon_clock_map(&mut self) {
        super::ops::prepare_modem_syscon_clock_map(&mut self.peripherals.modem_syscon_clock_reset);
    }

    #[doc(hidden)]
    pub fn configure_wifi_power_clock_map(&mut self) {
        self.prepare_modem_syscon_clock_map();
    }

    fn modem_syscon_clock_map_configured(&self) -> bool {
        let clk_conf_power_st = self
            .peripherals
            .modem_syscon_clock_reset
            .read_clk_conf_power_st();
        let (
            zb_map_bit_two,
            frontend_map_bit_one,
            frontend_map_bit_two,
            bluetooth_map_bit_two,
            wifi_map_bit_one,
            wifi_map_bit_two,
            modem_peripheral_map_bit_two,
            modem_apb_map_bit_one,
            modem_apb_map_bit_two,
        ) = (
            super::map_has(clk_conf_power_st.clk_zb_st_map(), 4),
            super::map_has(clk_conf_power_st.clk_fe_st_map(), 2),
            super::map_has(clk_conf_power_st.clk_fe_st_map(), 4),
            super::map_has(clk_conf_power_st.clk_bt_st_map(), 4),
            super::map_has(clk_conf_power_st.clk_wifi_st_map(), 2),
            super::map_has(clk_conf_power_st.clk_wifi_st_map(), 4),
            super::map_has(clk_conf_power_st.clk_modem_peri_st_map(), 4),
            super::map_has(clk_conf_power_st.clk_modem_apb_st_map(), 2),
            super::map_has(clk_conf_power_st.clk_modem_apb_st_map(), 4),
        );
        zb_map_bit_two
            && frontend_map_bit_one
            && frontend_map_bit_two
            && bluetooth_map_bit_two
            && wifi_map_bit_one
            && wifi_map_bit_two
            && modem_peripheral_map_bit_two
            && modem_apb_map_bit_one
            && modem_apb_map_bit_two
    }

    #[doc(hidden)]
    pub fn set_wifi_baseband_and_mac_reset(&mut self, reset: Reset) {
        super::ops::set_wifi_baseband_and_mac_reset(
            &mut self.peripherals.modem_syscon_clock_reset,
            reset,
        );
    }

    #[doc(hidden)]
    pub fn set_wifi_baseband_reset(&mut self, reset: Reset) {
        super::ops::set_wifi_baseband_reset(&mut self.peripherals.modem_syscon_clock_reset, reset);
    }

    #[doc(hidden)]
    pub fn enable_phy_calibration_clocks(&mut self) {
        super::ops::enable_phy_calibration_clocks(&mut self.peripherals.modem_syscon_clock_reset);
    }

    #[doc(hidden)]
    pub fn select_phy_i2c_160mhz_source(&mut self) {
        super::ops::select_phy_i2c_160mhz_source(&mut self.peripherals.modem_syscon_clock_reset);
    }

    #[doc(hidden)]
    pub fn modem_syscon_power_observation(&self) -> ModemSysconPowerObservation {
        let modem_rst_conf = self
            .peripherals
            .modem_syscon_clock_reset
            .read_modem_rst_conf();
        let (wifi_baseband_reset, wifi_mac_reset) =
            (modem_rst_conf.rst_wifibb(), modem_rst_conf.rst_wifimac());
        let clk_conf1 = self.peripherals.modem_syscon_clock_reset.read_clk_conf1();
        let (
            wifi_bb_22m,
            wifi_bb_40m,
            wifi_bb_44m,
            wifi_bb_80m,
            wifi_bb_40x,
            wifi_bb_80x,
            wifi_bb_40x1,
            wifi_bb_80x1,
            wifi_bb_160x1,
            wifi_apb,
            frontend_80m,
            frontend_160m,
            frontend_apb,
            bluetooth_apb,
            bluetooth_baseband,
            frontend_power_detector_adc,
            frontend_adc,
            frontend_dac,
        ) = (
            clk_conf1.clk_wifibb_22m_en(),
            clk_conf1.clk_wifibb_40m_en(),
            clk_conf1.clk_wifibb_44m_en(),
            clk_conf1.clk_wifibb_80m_en(),
            clk_conf1.clk_wifibb_40x_en(),
            clk_conf1.clk_wifibb_80x_en(),
            clk_conf1.clk_wifibb_40x1_en(),
            clk_conf1.clk_wifibb_80x1_en(),
            clk_conf1.clk_wifibb_160x1_en(),
            clk_conf1.clk_wifi_apb_en(),
            clk_conf1.clk_fe_80m_en(),
            clk_conf1.clk_fe_160m_en(),
            clk_conf1.clk_fe_apb_en(),
            clk_conf1.clk_bt_apb_en(),
            clk_conf1.clk_btbb_en(),
            clk_conf1.clk_fe_pwdet_adc_en(),
            clk_conf1.clk_fe_adc_en(),
            clk_conf1.clk_fe_dac_en(),
        );
        ModemSysconPowerObservation {
            wifi_reset_released: matches!(
                wifi_baseband_reset,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::Reset::Released
            ) && matches!(
                wifi_mac_reset,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::Reset::Released
            ),
            active_clock_map_configured: self.modem_syscon_clock_map_configured(),
            phy_calibration_clocks_enabled: matches!(
                wifi_bb_22m,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ) && matches!(
                wifi_bb_40m,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ) && matches!(
                wifi_bb_44m,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ) && matches!(
                wifi_bb_80m,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ) && matches!(
                wifi_bb_40x,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ) && matches!(
                wifi_bb_80x,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ) && matches!(
                wifi_bb_40x1,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ) && matches!(
                wifi_bb_80x1,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ) && matches!(
                wifi_bb_160x1,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ) && matches!(
                wifi_apb,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ) && frontend_80m
                && frontend_160m
                && frontend_apb
                && matches!(
                    bluetooth_apb,
                    oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
                )
                && matches!(
                    bluetooth_baseband,
                    oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
                )
                && frontend_power_detector_adc
                && frontend_adc
                && frontend_dac,
            phy_i2c_160mhz_selected: matches!(
                self.peripherals
                    .modem_syscon_clock_reset
                    .read_clk_conf()
                    .clk_i2c_mst_sel_160m(),
                ClkI2cMstSel160m::I2cMst160m
            ),
        }
    }

    #[doc(hidden)]
    pub fn enable_wifi_mac_clocks(&mut self) {
        super::ops::enable_wifi_mac_clocks(&mut self.peripherals.modem_syscon_clock_reset);
    }

    #[doc(hidden)]
    pub fn set_wifi_mac_reset(&mut self, reset: Reset) {
        super::ops::set_wifi_mac_reset(&mut self.peripherals.modem_syscon_clock_reset, reset);
    }

    pub fn clear_cold_start_wifi_control(&mut self) {
        super::ops::clear_cold_start_wifi_control(&mut self.peripherals.modem_syscon_radio);
    }

    pub fn wifi_baseband_is_enabled(&self) -> bool {
        matches!(
            self.peripherals
                .modem_syscon_radio
                .read_wifi_bb_cfg()
                .wifi_enable(),
            oer_chips_esp32s31_pac::modem_syscon::wifi_bb_cfg::WifiEnable::Enabled
        )
    }

    pub fn set_wifi_baseband_enabled(&mut self, enable: WifiEnable) {
        super::ops::set_wifi_baseband_enable(&mut self.peripherals.modem_syscon_radio, enable);
    }

    pub fn set_bss_cbw_40_digital(&mut self, enabled: bool) {
        let state = if enabled {
            WifiBaseband40MhzState::Enabled
        } else {
            WifiBaseband40MhzState::Disabled
        };
        super::ops::set_wifi_baseband_40mhz_state(&mut self.peripherals.modem_syscon_radio, state);
    }

    pub fn set_bb_agc_update_mode(&mut self, mode: WifiBasebandAgcUpdate) {
        let mode = match mode {
            WifiBasebandAgcUpdate::Initialization => WifiBasebandAgcUpdateMode::Initialization,
            WifiBasebandAgcUpdate::RegisterUpdatesEnabled => {
                WifiBasebandAgcUpdateMode::RegisterUpdatesEnabled
            }
        };
        super::ops::set_wifi_baseband_agc_update_mode(
            &mut self.peripherals.modem_syscon_radio,
            mode,
        );
    }

    pub fn set_mac_baseband_enabled(&mut self, enable: Bit28Opaque) {
        super::ops::set_mac_baseband_enable(&mut self.peripherals.modem_syscon_radio, enable);
    }

    pub fn enable_mac_baseband(&mut self) {
        self.set_mac_baseband_enabled(Bit28Opaque::Enabled);
        self.set_wifi_baseband_enabled(WifiEnable::Disabled);
        self.set_wifi_baseband_enabled(WifiEnable::Enabled);
    }

    #[doc(hidden)]
    pub fn set_ieee802154_mac_reset(&mut self, reset: Reset) {
        super::ops::set_ieee802154_mac_reset(&mut self.peripherals.modem_syscon_clock_reset, reset);
    }

    #[doc(hidden)]
    pub fn set_ieee802154_apb_reset(&mut self, reset: Reset) {
        super::ops::set_ieee802154_apb_reset(&mut self.peripherals.modem_syscon_clock_reset, reset);
    }

    #[doc(hidden)]
    pub fn ieee802154_reset_observation(&self) -> ModemSysconIeee802154ResetObservation {
        let modem_rst_conf = self
            .peripherals
            .modem_syscon_clock_reset
            .read_modem_rst_conf();
        let (mac_reset, apb_reset) = (modem_rst_conf.rst_zbmac(), modem_rst_conf.rst_zbmac_apb());
        ModemSysconIeee802154ResetObservation {
            mac_reset_released: matches!(
                mac_reset,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::Reset::Released
            ),
            apb_reset_released: matches!(
                apb_reset,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::Reset::Released
            ),
        }
    }

    /// Sample every Bluetooth clock group in one readback.
    #[doc(hidden)]
    pub fn bluetooth_clock_baselines(&self) -> [BluetoothClockBaseline; BLUETOOTH_CLOCK_COUNT] {
        let clk_conf = self.peripherals.modem_syscon_clock_reset.read_clk_conf();
        let (
            etm_enabled,
            modem_security_enabled,
            modem_security_ecb_enabled,
            modem_security_ccm_enabled,
            modem_security_bah_enabled,
            ble_timer_enabled,
            modem_security_apb_enabled,
        ) = (
            clk_conf.clk_etm_en(),
            clk_conf.clk_modem_sec_en(),
            clk_conf.clk_modem_sec_ecb_en(),
            clk_conf.clk_modem_sec_ccm_en(),
            clk_conf.clk_modem_sec_bah_en(),
            clk_conf.clk_ble_timer_en(),
            clk_conf.clk_modem_sec_apb_en(),
        );
        let clk_conf1 = self.peripherals.modem_syscon_clock_reset.read_clk_conf1();
        let (
            wifi_baseband_80x1_enabled,
            bluetooth_mac_enabled,
            bluetooth_apb_enabled,
            bluetooth_baseband_enabled,
        ) = (
            clk_conf1.clk_wifibb_80x1_en(),
            clk_conf1.clk_btmac_en(),
            clk_conf1.clk_bt_apb_en(),
            clk_conf1.clk_btbb_en(),
        );
        let mut baseline = BluetoothClockBaseline::default();
        baseline.record(
            BluetoothPhysicalClock::WifiBaseband80x1,
            matches!(
                wifi_baseband_80x1_enabled,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ),
        );
        baseline.record(
            BluetoothPhysicalClock::Etm,
            matches!(
                etm_enabled,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ),
        );
        baseline.record(
            BluetoothPhysicalClock::BluetoothMac,
            matches!(
                bluetooth_mac_enabled,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ),
        );
        baseline.record(
            BluetoothPhysicalClock::ModemSecurity,
            matches!(
                modem_security_enabled,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ),
        );
        baseline.record(
            BluetoothPhysicalClock::ModemSecurityEcb,
            matches!(
                modem_security_ecb_enabled,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ),
        );
        baseline.record(
            BluetoothPhysicalClock::ModemSecurityCcm,
            matches!(
                modem_security_ccm_enabled,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ),
        );
        baseline.record(
            BluetoothPhysicalClock::ModemSecurityBah,
            matches!(
                modem_security_bah_enabled,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ),
        );
        baseline.record(
            BluetoothPhysicalClock::BleTimer,
            matches!(
                ble_timer_enabled,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ),
        );
        baseline.record(
            BluetoothPhysicalClock::BluetoothApb,
            matches!(
                bluetooth_apb_enabled,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ),
        );
        baseline.record(
            BluetoothPhysicalClock::ModemSecurityApb,
            matches!(
                modem_security_apb_enabled,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ),
        );
        baseline.record(
            BluetoothPhysicalClock::BluetoothBaseband,
            matches!(
                bluetooth_baseband_enabled,
                oer_chips_esp32s31_pac::modem_syscon::dictionaries::ClockGate::Enabled
            ),
        );
        let mut baselines = [BluetoothClockBaseline::default(); BLUETOOTH_CLOCK_COUNT];
        for logical in BLUETOOTH_CONTROLLER_CLOCKS {
            for &physical in logical.physical_clocks() {
                baselines[logical.index()].record(physical, baseline.contains(physical));
            }
        }
        baselines
    }

    /// Enable every physical gate of one Bluetooth clock group.
    #[doc(hidden)]
    pub fn enable_bluetooth_clock(&mut self, device: ModemSysconBluetoothClock) {
        self.set_bluetooth_clock_enabled(device, ClockGate::Enabled);
    }

    fn set_bluetooth_clock_enabled(&mut self, device: ModemSysconBluetoothClock, state: ClockGate) {
        let registers = &mut self.peripherals.modem_syscon_clock_reset;
        match device {
            ModemSysconBluetoothClock::WifiBaseband80x1 => {
                super::ops::set_bluetooth_wifi_baseband_80x1_clock(registers, state);
            }
            ModemSysconBluetoothClock::Etm => {
                super::ops::set_bluetooth_etm_clock(registers, state);
            }
            ModemSysconBluetoothClock::BluetoothMac => {
                super::ops::set_bluetooth_mac_clock(registers, state);
            }
            ModemSysconBluetoothClock::BluetoothPeripheral => {
                super::ops::set_bluetooth_peripheral_clocks(registers, state);
            }
            ModemSysconBluetoothClock::BluetoothApb => {
                super::ops::set_bluetooth_apb_clock(registers, state);
                super::ops::set_bluetooth_modem_security_apb_clock(registers, state);
            }
            ModemSysconBluetoothClock::BluetoothBaseband => {
                super::ops::set_bluetooth_baseband_clock(registers, state);
            }
        }
    }

    /// Restore the physical gates of one group from a captured baseline.
    #[doc(hidden)]
    pub fn restore_bluetooth_clock(
        &mut self,
        device: ModemSysconBluetoothClock,
        baseline: BluetoothClockBaseline,
    ) {
        match device {
            ModemSysconBluetoothClock::BluetoothPeripheral => {
                restore_bluetooth_peripheral_clocks(
                    &mut self.peripherals.modem_syscon_clock_reset,
                    [
                        baseline.contains(BluetoothPhysicalClock::ModemSecurity),
                        baseline.contains(BluetoothPhysicalClock::ModemSecurityEcb),
                        baseline.contains(BluetoothPhysicalClock::ModemSecurityCcm),
                        baseline.contains(BluetoothPhysicalClock::ModemSecurityBah),
                        baseline.contains(BluetoothPhysicalClock::BleTimer),
                    ],
                );
            }
            ModemSysconBluetoothClock::BluetoothApb => {
                super::ops::set_bluetooth_apb_clock(
                    &mut self.peripherals.modem_syscon_clock_reset,
                    ClockGate::from_field(u32::from(
                        baseline.contains(BluetoothPhysicalClock::BluetoothApb),
                    )),
                );
                super::ops::set_bluetooth_modem_security_apb_clock(
                    &mut self.peripherals.modem_syscon_clock_reset,
                    ClockGate::from_field(u32::from(
                        baseline.contains(BluetoothPhysicalClock::ModemSecurityApb),
                    )),
                );
            }
            _ => self.set_bluetooth_clock_enabled(
                device,
                ClockGate::from_field(u32::from(baseline.all_enabled(device))),
            ),
        }
    }

    #[doc(hidden)]
    pub fn bluetooth_clock_observation(&self) -> ModemSysconBluetoothObservation {
        let clocks = self.bluetooth_clock_baselines();
        ModemSysconBluetoothObservation {
            controller_clocks_enabled: BLUETOOTH_CONTROLLER_CLOCKS
                .into_iter()
                .all(|clock| clocks[clock.index()].all_enabled(clock)),
            apb_clocks_enabled: BLUETOOTH_APB_CLOCKS
                .into_iter()
                .all(|clock| clocks[clock.index()].all_enabled(clock)),
            controller_resets_released: self.bluetooth_controller_resets_released(),
        }
    }

    #[doc(hidden)]
    pub fn reset_bluetooth_controller_domains(&mut self) {
        let registers = &mut self.peripherals.modem_syscon_clock_reset;
        super::ops::set_bluetooth_mac_reset(registers, Reset::Asserted);
        super::ops::set_bluetooth_mac_reset(registers, Reset::Released);
        super::ops::set_bluetooth_mac_apb_reset(registers, Reset::Asserted);
        super::ops::set_bluetooth_mac_apb_reset(registers, Reset::Released);
        super::ops::set_bluetooth_timer_reset(registers, Reset::Asserted);
        super::ops::set_bluetooth_timer_reset(registers, Reset::Released);
        super::ops::set_bluetooth_modem_ecb_reset(registers, Reset::Asserted);
        super::ops::set_bluetooth_modem_ccm_reset(registers, Reset::Asserted);
        super::ops::set_bluetooth_modem_bah_reset(registers, Reset::Asserted);
        super::ops::set_bluetooth_modem_security_reset(registers, Reset::Asserted);
        super::ops::set_bluetooth_modem_ecb_reset(registers, Reset::Released);
        super::ops::set_bluetooth_modem_ccm_reset(registers, Reset::Released);
        super::ops::set_bluetooth_modem_bah_reset(registers, Reset::Released);
        super::ops::set_bluetooth_modem_security_reset(registers, Reset::Released);
    }

    pub(crate) fn bluetooth_controller_resets_released(&self) -> bool {
        let modem_rst_conf = self
            .peripherals
            .modem_syscon_clock_reset
            .read_modem_rst_conf();
        let (
            bluetooth_mac_reset,
            bluetooth_mac_apb_reset,
            bluetooth_timer_reset,
            modem_ecb_reset,
            modem_ccm_reset,
            modem_bah_reset,
            modem_security_reset,
        ) = (
            modem_rst_conf.rst_btmac(),
            modem_rst_conf.rst_btmac_apb(),
            modem_rst_conf.rst_ble_timer(),
            modem_rst_conf.rst_modem_ecb(),
            modem_rst_conf.rst_modem_ccm(),
            modem_rst_conf.rst_modem_bah(),
            modem_rst_conf.rst_modem_sec(),
        );
        matches!(
            bluetooth_mac_reset,
            oer_chips_esp32s31_pac::modem_syscon::dictionaries::Reset::Released
        ) && matches!(
            bluetooth_mac_apb_reset,
            oer_chips_esp32s31_pac::modem_syscon::dictionaries::Reset::Released
        ) && matches!(
            bluetooth_timer_reset,
            oer_chips_esp32s31_pac::modem_syscon::dictionaries::Reset::Released
        ) && matches!(
            modem_ecb_reset,
            oer_chips_esp32s31_pac::modem_syscon::dictionaries::Reset::Released
        ) && matches!(
            modem_ccm_reset,
            oer_chips_esp32s31_pac::modem_syscon::dictionaries::Reset::Released
        ) && matches!(
            modem_bah_reset,
            oer_chips_esp32s31_pac::modem_syscon::dictionaries::Reset::Released
        ) && matches!(
            modem_security_reset,
            oer_chips_esp32s31_pac::modem_syscon::dictionaries::Reset::Released
        )
    }
}
