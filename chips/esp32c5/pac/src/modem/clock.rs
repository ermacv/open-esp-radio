//! Vendor modem clock devices and the IEEE 802.15.4 MAC reset.
//!
//! Each [`ModemClockDevice`] is one ESP-IDF `modem_clock_*_configure(enable)`
//! action for the ESP32-C5, applied by the HAL at a zero-to-one or
//! one-to-zero dependency edge. Only the devices of the IEEE 802.15.4 module
//! are published so far.
//!
//! SOURCE: reviewed evidence `ESP_IDF_4D59230D_C5_MODEM_CLOCK`
//! (`modem_clock_impl.c` configure and check actions, `modem_syscon_ll.h`,
//! `modem_lpcon_ll.h`, `modem_clock.c` `modem_clock_module_mac_reset`).

#![forbid(unsafe_code)]

use crate::generated::{
    ModemClockGateState, ModemResetState, peripheral_ownership::ModemClockPeripherals,
};

/// One modem clock device whose gates the PAC drives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModemClockDevice {
    /// The MODEM_LPCON coexistence clock.
    Coexistence,
    /// The modem event-task matrix clock.
    Etm,
    /// Bluetooth APB and modem-security APB clocks.
    BluetoothApb,
    /// The baseband clock shared by Bluetooth LE and IEEE 802.15.4.
    BluetoothIeee802154CommonBaseband,
    /// IEEE 802.15.4 APB and MAC clocks.
    Ieee802154Mac,
}

/// Released state of the two IEEE 802.15.4 reset lines, sampled once.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Ieee802154ResetObservation {
    pub mac_released: bool,
    pub apb_released: bool,
}

const fn gate(enable: bool) -> ModemClockGateState {
    if enable {
        ModemClockGateState::Enabled
    } else {
        ModemClockGateState::Disabled
    }
}

/// Unique owner of the modem clock gates and the IEEE 802.15.4 MAC resets.
#[must_use = "dropping the modem clock owner permanently loses its register authority"]
pub struct ModemClockRegisters {
    peripherals: ModemClockPeripherals,
}

impl ModemClockRegisters {
    pub(crate) const fn new(peripherals: ModemClockPeripherals) -> Self {
        Self { peripherals }
    }

    /// Apply one vendor modem clock device action.
    ///
    /// The write order within each action is the vendor order; every write
    /// preserves the other fields of its register.
    pub fn configure_modem_clock_device(&mut self, device: ModemClockDevice, enable: bool) {
        let syscon = &mut self.peripherals.modem_syscon_radio;
        match device {
            ModemClockDevice::Coexistence => self
                .peripherals
                .modem_lpcon_shared_clock
                .set_coexistence_clock(gate(enable)),
            ModemClockDevice::Etm => syscon.set_etm_clock(gate(enable)),
            ModemClockDevice::BluetoothApb => {
                syscon.set_bluetooth_apb_clock(gate(enable));
                syscon.set_modem_security_apb_clock(gate(enable));
            }
            ModemClockDevice::BluetoothIeee802154CommonBaseband => {
                syscon.set_bluetooth_ieee802154_baseband_clock(gate(enable));
            }
            ModemClockDevice::Ieee802154Mac => {
                syscon.set_ieee802154_apb_clock(gate(enable));
                syscon.set_ieee802154_mac_clock(gate(enable));
            }
        }
    }

    /// Whether every gate of one device is enabled, as the vendor
    /// `modem_clock_*_check_enable` action reports it.
    #[cfg(oer_probe)]
    pub fn modem_clock_device_enabled(&self, device: ModemClockDevice) -> bool {
        let syscon = &self.peripherals.modem_syscon_radio;
        let conf = syscon.read_clk_conf();
        let conf1 = syscon.read_clk_conf1();
        match device {
            ModemClockDevice::Coexistence => self
                .peripherals
                .modem_lpcon_shared_clock
                .read_clk_conf()
                .clk_coex_en(),
            ModemClockDevice::Etm => conf.clk_etm_en(),
            ModemClockDevice::BluetoothApb => conf1.clk_bt_apb_en() && conf.clk_modem_sec_apb_en(),
            ModemClockDevice::BluetoothIeee802154CommonBaseband => conf1.clk_btbb_en(),
            ModemClockDevice::Ieee802154Mac => conf.clk_zb_apb_en() && conf.clk_zbmac_en(),
        }
    }

    /// Pulse the IEEE 802.15.4 MAC reset, then its APB reset, as
    /// `modem_clock_module_mac_reset` does: each line is written asserted and
    /// then released.
    pub fn pulse_ieee802154_mac_reset(&mut self) {
        let syscon = &mut self.peripherals.modem_syscon_radio;
        syscon.set_ieee802154_mac_reset(ModemResetState::Asserted);
        syscon.set_ieee802154_mac_reset(ModemResetState::Released);
        syscon.set_ieee802154_apb_reset(ModemResetState::Asserted);
        syscon.set_ieee802154_apb_reset(ModemResetState::Released);
    }

    /// Whether the analog register I2C master clock is enabled. The platform
    /// owns that gate and its reference count.
    #[cfg(oer_probe)]
    pub fn analog_i2c_master_clock_enabled(&self) -> bool {
        self.peripherals
            .modem_lpcon_shared_clock
            .read_clk_conf()
            .clk_i2c_mst_en()
    }

    /// Sample both IEEE 802.15.4 reset lines.
    pub fn ieee802154_reset_observation(&self) -> Ieee802154ResetObservation {
        let resets = self.peripherals.modem_syscon_radio.read_modem_rst_conf();
        Ieee802154ResetObservation {
            mac_released: !resets.rst_zbmac(),
            apb_released: !resets.rst_zbmac_apb(),
        }
    }
}
