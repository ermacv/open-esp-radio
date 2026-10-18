//! ESP32-S31 official singleton witnesses behind a role-neutral platform owner.

use esp_hal::{
    efuse,
    peripherals::{
        HP_SYS_CLKRST, I2C_ANA_MST, LP_AON_CLK_RST, LP_PERI, LP_TSENS, MODEM_LPCON, MODEM_SYSCON,
        PMU,
    },
};
use oer_bluetooth_hci::BluetoothPublicDeviceAddress;
use oer_esp32s31_hal::root::AnalogBusOwnership;
use oer_esp32s31_phy::{PhyCalibrationIdentity, phy_get_rf_cal_version};

use crate::bluetooth_address::bluetooth_public_address_from_base;

/// Sole safe owner of the ESP32-S31 shared radio-platform singletons.
///
/// Construction consumes the official ESP-HAL singleton tokens. The tokens
/// stay private for the owner's whole lifetime; the radio system holds it as
/// the platform its PHY target port borrows. It performs no register access
/// itself: every clock transaction and reference count lives in the custom
/// PAC route.
pub struct EspHalRadioPlatform {
    _modem_syscon: MODEM_SYSCON<'static>,
    _modem_lpcon: MODEM_LPCON<'static>,
    _hp_sys_clkrst: HP_SYS_CLKRST<'static>,
    _pmu: PMU<'static>,
    _lp_aon_clkrst: LP_AON_CLK_RST<'static>,
    _lp_peri: LP_PERI<'static>,
    _lp_tsens: LP_TSENS<'static>,
    _i2c_ana_mst: I2C_ANA_MST<'static>,
    /// Whether the analog bus ownership was handed to the radio root.
    analog_bus_handed_out: bool,
}

impl EspHalRadioPlatform {
    /// Establish the neutral radio-platform owner after `esp_hal::init`.
    #[allow(
        clippy::too_many_arguments,
        reason = "construction consumes the complete non-groupable ESP-HAL singleton set"
    )]
    pub const fn new(
        modem_syscon: MODEM_SYSCON<'static>,
        modem_lpcon: MODEM_LPCON<'static>,
        hp_sys_clkrst: HP_SYS_CLKRST<'static>,
        pmu: PMU<'static>,
        lp_aon_clkrst: LP_AON_CLK_RST<'static>,
        lp_peri: LP_PERI<'static>,
        lp_tsens: LP_TSENS<'static>,
        i2c_ana_mst: I2C_ANA_MST<'static>,
    ) -> Self {
        Self {
            _modem_syscon: modem_syscon,
            _modem_lpcon: modem_lpcon,
            _hp_sys_clkrst: hp_sys_clkrst,
            _pmu: pmu,
            _lp_aon_clkrst: lp_aon_clkrst,
            _lp_peri: lp_peri,
            _lp_tsens: lp_tsens,
            _i2c_ana_mst: i2c_ana_mst,
            analog_bus_handed_out: false,
        }
    }

    /// The analog-I2C bus ownership the radio root requires
    /// (`RadioHardware::take`), handed out once: this platform holds the
    /// `I2C_ANA_MST` singleton for the rest of the program.
    pub fn analog_bus_ownership(&mut self) -> Option<AnalogBusOwnership> {
        if core::mem::replace(&mut self.analog_bus_handed_out, true) {
            return None;
        }
        #[allow(
            unsafe_code,
            reason = "the held I2C_ANA_MST singleton is the analog bus ownership the token asserts"
        )]
        // SAFETY: this platform owns the `I2C_ANA_MST` singleton for the rest
        // of the program and hands the ownership out only this once.
        let ownership = unsafe { AnalogBusOwnership::assume_exclusive() };
        Some(ownership)
    }

    /// Derive the common-PHY calibration identity from the chip.
    ///
    /// The radio-calibration version is source-owned, while both identity
    /// fields come from ESP-HAL's safe eFuse accessors. Applications therefore
    /// do not copy hardware identity into an otherwise generic configuration.
    pub fn phy_calibration_identity(&self) -> PhyCalibrationIdentity {
        let base = efuse::base_mac_address();
        let bytes = base.as_bytes();
        PhyCalibrationIdentity {
            rf_cal_version: phy_get_rf_cal_version(),
            base_mac_address: [bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5]],
            mac_extension: efuse::read_field_le::<u16>(efuse::MAC_EXT),
        }
    }

    /// Read the factory base identity through ESP-HAL's safe eFuse accessor,
    /// apply the ESP32-S31 second-universal-address policy and retain the
    /// result in canonical EUI-48 order.
    ///
    /// The reviewed ESP32-S31 Controller HCI initializer requests the Bluetooth
    /// interface identity, then reverses those six canonical bytes for its HCI
    /// storage.
    /// [`BluetoothPublicDeviceAddress`] owns that protocol conversion, so this
    /// platform boundary neither exposes eFuse fields nor asks callers to
    /// hand-author HCI byte order.
    pub fn bluetooth_public_address(&self) -> BluetoothPublicDeviceAddress {
        let base = efuse::base_mac_address();
        let bytes = base.as_bytes();
        bluetooth_public_address_from_base([
            bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5],
        ])
    }
}
