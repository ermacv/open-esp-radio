//! Route-owned MODEM_SYSCON radio clock, reset and baseband control.
//!
//! Register geometry remains in the generated PAC. This module publishes only
//! semantic operations through the affine radio route and preserves every
//! vendor RMW edge and protocol-specific order.

#![forbid(unsafe_code)]

use oer_mmio::Placement;

use crate::{
    RadioPhyRegisters,
    generated::{
        ModemSysconClockGateState, ModemSysconEnableState, ModemSysconResetState,
        WifiBaseband40MhzState, WifiBasebandAgcUpdateMode, modem_syscon_radio::ModemSysconRadio,
    },
};

#[cfg(test)]
mod tests;

/// The eighteen PHY-calibration clock gates of `CLK_CONF1`, from one read,
/// in the order [`restore_calibration_clocks`] writes them.
fn calibration_clocks<P: Placement>(registers: &ModemSysconRadio<P>) -> [bool; 18] {
    let clk_conf1 = registers.read_clk_conf1();
    [
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
    ]
}

/// One fresh-read RMW of the eighteen captured gates; every other bit of
/// `CLK_CONF1` keeps the value read.
fn restore_calibration_clocks<P: Placement>(
    registers: &mut ModemSysconRadio<P>,
    clocks: [bool; 18],
) {
    registers.modify_clk_conf1(|w| {
        w.set_clk_wifibb_22m_en(clocks[0])
            .set_clk_wifibb_40m_en(clocks[1])
            .set_clk_wifibb_44m_en(clocks[2])
            .set_clk_wifibb_80m_en(clocks[3])
            .set_clk_wifibb_40x_en(clocks[4])
            .set_clk_wifibb_80x_en(clocks[5])
            .set_clk_wifibb_40x1_en(clocks[6])
            .set_clk_wifibb_80x1_en(clocks[7])
            .set_clk_wifibb_160x1_en(clocks[8])
            .set_clk_wifi_apb_en(clocks[9])
            .set_clk_fe_80m_en(clocks[10])
            .set_clk_fe_160m_en(clocks[11])
            .set_clk_fe_apb_en(clocks[12])
            .set_clk_bt_apb_en(clocks[13])
            .set_clk_btbb_en(clocks[14])
            .set_clk_fe_pwdet_adc_en(clocks[15])
            .set_clk_fe_adc_en(clocks[16])
            .set_clk_fe_dac_en(clocks[17]);
    });
}

/// One fresh-read RMW of the five Bluetooth peripheral gates of
/// `CLK_CONF`: modem security, ECB, CCM, BAH and the BLE timer.
fn restore_bluetooth_peripheral_clocks<P: Placement>(
    registers: &mut ModemSysconRadio<P>,
    [security, ecb, ccm, bah, ble_timer]: [bool; 5],
) {
    registers.modify_clk_conf(|w| {
        w.set_clk_modem_sec_en(security)
            .set_clk_modem_sec_ecb_en(ecb)
            .set_clk_modem_sec_ccm_en(ccm)
            .set_clk_modem_sec_bah_en(bah)
            .set_clk_ble_timer_en(ble_timer);
    });
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ModemSysconPowerObservation {
    pub wifi_reset_released: bool,
    pub active_clock_map_configured: bool,
    pub phy_calibration_clocks_enabled: bool,
    pub phy_i2c_160mhz_selected: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ModemSysconPowerBaseline {
    phy_calibration_clocks: [bool; 18],
    phy_i2c_160mhz_selected: bool,
}

impl ModemSysconPowerBaseline {
    pub(super) const BIT_COUNT: u32 = 19;

    pub(super) fn bits(self) -> u32 {
        let mut bits = 0;
        let mut clock = 0;
        while clock < self.phy_calibration_clocks.len() {
            bits |= u32::from(self.phy_calibration_clocks[clock]) << clock;
            clock += 1;
        }
        bits | (u32::from(self.phy_i2c_160mhz_selected) << self.phy_calibration_clocks.len())
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
            phy_i2c_160mhz_selected: bits & (1 << phy_calibration_clocks.len()) != 0,
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

const fn modem_syscon_clock_gate_state(enabled: bool) -> ModemSysconClockGateState {
    if enabled {
        ModemSysconClockGateState::Enabled
    } else {
        ModemSysconClockGateState::Disabled
    }
}

const fn modem_syscon_enable_state(enabled: bool) -> ModemSysconEnableState {
    if enabled {
        ModemSysconEnableState::Enabled
    } else {
        ModemSysconEnableState::Disabled
    }
}

const fn modem_syscon_reset_state(asserted: bool) -> ModemSysconResetState {
    if asserted {
        ModemSysconResetState::Asserted
    } else {
        ModemSysconResetState::Released
    }
}

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

impl RadioPhyRegisters {
    pub(crate) fn modem_syscon_power_baseline(&self) -> ModemSysconPowerBaseline {
        ModemSysconPowerBaseline {
            phy_calibration_clocks: calibration_clocks(&self.peripherals.modem_syscon_radio),
            phy_i2c_160mhz_selected: self
                .peripherals
                .modem_syscon_radio
                .read_clk_conf()
                .clk_i2c_mst_sel_160m(),
        }
    }

    pub(crate) fn restore_modem_syscon_power_baseline(
        &mut self,
        baseline: ModemSysconPowerBaseline,
    ) {
        let registers = &mut self.peripherals.modem_syscon_radio;
        registers.restore_phy_i2c_160mhz_source(modem_syscon_clock_gate_state(
            baseline.phy_i2c_160mhz_selected,
        ));
        restore_calibration_clocks(registers, baseline.phy_calibration_clocks);
    }

    #[doc(hidden)]
    pub fn prepare_modem_syscon_clock_map(&mut self) {
        self.peripherals
            .modem_syscon_radio
            .prepare_modem_syscon_clock_map();
    }

    #[doc(hidden)]
    pub fn configure_wifi_power_clock_map(&mut self) {
        self.prepare_modem_syscon_clock_map();
    }

    fn modem_syscon_clock_map_configured(&self) -> bool {
        let clk_conf_power_st = self.peripherals.modem_syscon_radio.read_clk_conf_power_st();
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
            clk_conf_power_st.clk_zb_st_map_bit_two(),
            clk_conf_power_st.clk_fe_st_map_bit_one(),
            clk_conf_power_st.clk_fe_st_map_bit_two(),
            clk_conf_power_st.clk_bt_st_map_bit_two(),
            clk_conf_power_st.clk_wifi_st_map_bit_one(),
            clk_conf_power_st.clk_wifi_st_map_bit_two(),
            clk_conf_power_st.clk_modem_peri_st_map_bit_two(),
            clk_conf_power_st.clk_modem_apb_st_map_bit_one(),
            clk_conf_power_st.clk_modem_apb_st_map_bit_two(),
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
    pub fn set_wifi_baseband_and_mac_reset(&mut self, asserted: bool) {
        self.peripherals
            .modem_syscon_radio
            .set_wifi_baseband_and_mac_reset(modem_syscon_reset_state(asserted));
    }

    #[doc(hidden)]
    pub fn set_wifi_baseband_reset(&mut self, asserted: bool) {
        self.peripherals
            .modem_syscon_radio
            .set_wifi_baseband_reset(modem_syscon_reset_state(asserted));
    }

    #[doc(hidden)]
    pub fn enable_phy_calibration_clocks(&mut self) {
        self.peripherals
            .modem_syscon_radio
            .enable_phy_calibration_clocks();
    }

    #[doc(hidden)]
    pub fn select_phy_i2c_160mhz_source(&mut self) {
        self.peripherals
            .modem_syscon_radio
            .select_phy_i2c_160mhz_source();
    }

    #[doc(hidden)]
    pub fn modem_syscon_power_observation(&self) -> ModemSysconPowerObservation {
        let modem_rst_conf = self.peripherals.modem_syscon_radio.read_modem_rst_conf();
        let (wifi_baseband_reset, wifi_mac_reset) =
            (modem_rst_conf.rst_wifibb(), modem_rst_conf.rst_wifimac());
        let clk_conf1 = self.peripherals.modem_syscon_radio.read_clk_conf1();
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
            wifi_reset_released: !wifi_baseband_reset && !wifi_mac_reset,
            active_clock_map_configured: self.modem_syscon_clock_map_configured(),
            phy_calibration_clocks_enabled: wifi_bb_22m
                && wifi_bb_40m
                && wifi_bb_44m
                && wifi_bb_80m
                && wifi_bb_40x
                && wifi_bb_80x
                && wifi_bb_40x1
                && wifi_bb_80x1
                && wifi_bb_160x1
                && wifi_apb
                && frontend_80m
                && frontend_160m
                && frontend_apb
                && bluetooth_apb
                && bluetooth_baseband
                && frontend_power_detector_adc
                && frontend_adc
                && frontend_dac,
            phy_i2c_160mhz_selected: self
                .peripherals
                .modem_syscon_radio
                .read_clk_conf()
                .clk_i2c_mst_sel_160m(),
        }
    }

    #[doc(hidden)]
    pub fn enable_wifi_mac_clocks(&mut self) {
        self.peripherals.modem_syscon_radio.enable_wifi_mac_clocks();
    }

    #[doc(hidden)]
    pub fn set_wifi_mac_reset(&mut self, asserted: bool) {
        self.peripherals
            .modem_syscon_radio
            .set_wifi_mac_reset(modem_syscon_reset_state(asserted));
    }

    pub fn clear_cold_start_wifi_control(&mut self) {
        self.peripherals
            .modem_syscon_radio
            .clear_cold_start_wifi_control();
    }

    pub fn wifi_baseband_is_enabled(&self) -> bool {
        self.peripherals
            .modem_syscon_radio
            .read_wifi_bb_cfg()
            .wifi_enable()
    }

    pub fn set_wifi_baseband_enabled(&mut self, enabled: bool) {
        self.peripherals
            .modem_syscon_radio
            .set_wifi_baseband_enable(modem_syscon_enable_state(enabled));
    }

    pub fn set_bss_cbw_40_digital(&mut self, enabled: bool) {
        let state = if enabled {
            WifiBaseband40MhzState::Enabled
        } else {
            WifiBaseband40MhzState::Disabled
        };
        self.peripherals
            .modem_syscon_radio
            .set_wifi_baseband_40mhz_state(state);
    }

    pub fn set_bb_agc_update_mode(&mut self, mode: WifiBasebandAgcUpdate) {
        let mode = match mode {
            WifiBasebandAgcUpdate::Initialization => WifiBasebandAgcUpdateMode::Initialization,
            WifiBasebandAgcUpdate::RegisterUpdatesEnabled => {
                WifiBasebandAgcUpdateMode::RegisterUpdatesEnabled
            }
        };
        self.peripherals
            .modem_syscon_radio
            .set_wifi_baseband_agc_update_mode(mode);
    }

    pub fn set_mac_baseband_enabled(&mut self, enabled: bool) {
        self.peripherals
            .modem_syscon_radio
            .set_mac_baseband_enable(modem_syscon_enable_state(enabled));
    }

    pub fn enable_mac_baseband(&mut self) {
        self.set_mac_baseband_enabled(true);
        self.set_wifi_baseband_enabled(false);
        self.set_wifi_baseband_enabled(true);
    }

    #[doc(hidden)]
    pub fn set_ieee802154_mac_reset(&mut self, asserted: bool) {
        self.peripherals
            .modem_syscon_radio
            .set_ieee802154_mac_reset(modem_syscon_reset_state(asserted));
    }

    #[doc(hidden)]
    pub fn set_ieee802154_apb_reset(&mut self, asserted: bool) {
        self.peripherals
            .modem_syscon_radio
            .set_ieee802154_apb_reset(modem_syscon_reset_state(asserted));
    }

    #[doc(hidden)]
    pub fn ieee802154_reset_observation(&self) -> ModemSysconIeee802154ResetObservation {
        let modem_rst_conf = self.peripherals.modem_syscon_radio.read_modem_rst_conf();
        let (mac_reset, apb_reset) = (modem_rst_conf.rst_zbmac(), modem_rst_conf.rst_zbmac_apb());
        ModemSysconIeee802154ResetObservation {
            mac_reset_released: !mac_reset,
            apb_reset_released: !apb_reset,
        }
    }

    /// Sample every Bluetooth clock group in one readback.
    #[doc(hidden)]
    pub fn bluetooth_clock_baselines(&self) -> [BluetoothClockBaseline; BLUETOOTH_CLOCK_COUNT] {
        let clk_conf = self.peripherals.modem_syscon_radio.read_clk_conf();
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
        let clk_conf1 = self.peripherals.modem_syscon_radio.read_clk_conf1();
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
            wifi_baseband_80x1_enabled,
        );
        baseline.record(BluetoothPhysicalClock::Etm, etm_enabled);
        baseline.record(BluetoothPhysicalClock::BluetoothMac, bluetooth_mac_enabled);
        baseline.record(
            BluetoothPhysicalClock::ModemSecurity,
            modem_security_enabled,
        );
        baseline.record(
            BluetoothPhysicalClock::ModemSecurityEcb,
            modem_security_ecb_enabled,
        );
        baseline.record(
            BluetoothPhysicalClock::ModemSecurityCcm,
            modem_security_ccm_enabled,
        );
        baseline.record(
            BluetoothPhysicalClock::ModemSecurityBah,
            modem_security_bah_enabled,
        );
        baseline.record(BluetoothPhysicalClock::BleTimer, ble_timer_enabled);
        baseline.record(BluetoothPhysicalClock::BluetoothApb, bluetooth_apb_enabled);
        baseline.record(
            BluetoothPhysicalClock::ModemSecurityApb,
            modem_security_apb_enabled,
        );
        baseline.record(
            BluetoothPhysicalClock::BluetoothBaseband,
            bluetooth_baseband_enabled,
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
        self.set_bluetooth_clock_enabled(device, true);
    }

    fn set_bluetooth_clock_enabled(&mut self, device: ModemSysconBluetoothClock, enabled: bool) {
        let state = modem_syscon_clock_gate_state(enabled);
        let registers = &mut self.peripherals.modem_syscon_radio;
        match device {
            ModemSysconBluetoothClock::WifiBaseband80x1 => {
                registers.set_bluetooth_wifi_baseband_80x1_clock(state);
            }
            ModemSysconBluetoothClock::Etm => {
                registers.set_bluetooth_etm_clock(state);
            }
            ModemSysconBluetoothClock::BluetoothMac => {
                registers.set_bluetooth_mac_clock(state);
            }
            ModemSysconBluetoothClock::BluetoothPeripheral => {
                registers.set_bluetooth_peripheral_clocks(state);
            }
            ModemSysconBluetoothClock::BluetoothApb => {
                registers.set_bluetooth_apb_clock(state);
                registers.set_bluetooth_modem_security_apb_clock(state);
            }
            ModemSysconBluetoothClock::BluetoothBaseband => {
                registers.set_bluetooth_baseband_clock(state);
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
                    &mut self.peripherals.modem_syscon_radio,
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
                self.peripherals.modem_syscon_radio.set_bluetooth_apb_clock(
                    modem_syscon_clock_gate_state(
                        baseline.contains(BluetoothPhysicalClock::BluetoothApb),
                    ),
                );
                self.peripherals
                    .modem_syscon_radio
                    .set_bluetooth_modem_security_apb_clock(modem_syscon_clock_gate_state(
                        baseline.contains(BluetoothPhysicalClock::ModemSecurityApb),
                    ));
            }
            _ => self.set_bluetooth_clock_enabled(device, baseline.all_enabled(device)),
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
        let registers = &mut self.peripherals.modem_syscon_radio;
        registers.set_bluetooth_mac_reset(ModemSysconResetState::Asserted);
        registers.set_bluetooth_mac_reset(ModemSysconResetState::Released);
        registers.set_bluetooth_mac_apb_reset(ModemSysconResetState::Asserted);
        registers.set_bluetooth_mac_apb_reset(ModemSysconResetState::Released);
        registers.set_bluetooth_timer_reset(ModemSysconResetState::Asserted);
        registers.set_bluetooth_timer_reset(ModemSysconResetState::Released);
        registers.set_bluetooth_modem_ecb_reset(ModemSysconResetState::Asserted);
        registers.set_bluetooth_modem_ccm_reset(ModemSysconResetState::Asserted);
        registers.set_bluetooth_modem_bah_reset(ModemSysconResetState::Asserted);
        registers.set_bluetooth_modem_security_reset(ModemSysconResetState::Asserted);
        registers.set_bluetooth_modem_ecb_reset(ModemSysconResetState::Released);
        registers.set_bluetooth_modem_ccm_reset(ModemSysconResetState::Released);
        registers.set_bluetooth_modem_bah_reset(ModemSysconResetState::Released);
        registers.set_bluetooth_modem_security_reset(ModemSysconResetState::Released);
    }

    pub(crate) fn bluetooth_controller_resets_released(&self) -> bool {
        let modem_rst_conf = self.peripherals.modem_syscon_radio.read_modem_rst_conf();
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
        !bluetooth_mac_reset
            && !bluetooth_mac_apb_reset
            && !bluetooth_timer_reset
            && !modem_ecb_reset
            && !modem_ccm_reset
            && !modem_bah_reset
            && !modem_security_reset
    }
}
