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

use crate::{
    generated::{self, ModemClockGateState, ModemResetState},
    svd,
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
    peripherals: svd::peripheral_ownership::ModemClockPeripherals,
}

impl ModemClockRegisters {
    pub(crate) const fn new(peripherals: svd::peripheral_ownership::ModemClockPeripherals) -> Self {
        Self { peripherals }
    }

    /// Apply one vendor modem clock device action.
    ///
    /// The write order within each action is the vendor order; every write
    /// preserves the other fields of its register.
    pub fn configure_modem_clock_device(&mut self, device: ModemClockDevice, enable: bool) {
        let syscon = &self.peripherals.modem_syscon_radio;
        match device {
            ModemClockDevice::Coexistence => generated::set_coexistence_clock(
                &self.peripherals.modem_lpcon_shared_clock,
                gate(enable),
            ),
            ModemClockDevice::Etm => generated::set_etm_clock(syscon, gate(enable)),
            ModemClockDevice::BluetoothApb => {
                generated::set_bluetooth_apb_clock(syscon, gate(enable));
                generated::set_modem_security_apb_clock(syscon, gate(enable));
            }
            ModemClockDevice::BluetoothIeee802154CommonBaseband => {
                generated::set_bluetooth_ieee802154_baseband_clock(syscon, gate(enable));
            }
            ModemClockDevice::Ieee802154Mac => {
                generated::set_ieee802154_apb_clock(syscon, gate(enable));
                generated::set_ieee802154_mac_clock(syscon, gate(enable));
            }
        }
    }

    /// Whether every gate of one device is enabled, as the vendor
    /// `modem_clock_*_check_enable` action reports it.
    pub fn modem_clock_device_enabled(&self, device: ModemClockDevice) -> bool {
        let syscon = &self.peripherals.modem_syscon_radio;
        let (etm, zb_apb, zbmac, security_apb) =
            svd::field_snapshot_read::observe_modem_clock_conf(syscon);
        let (bt_apb, btbb) = svd::field_snapshot_read::observe_modem_clock_conf1(syscon);
        match device {
            ModemClockDevice::Coexistence => svd::field_read::observe_coexistence_clock(
                &self.peripherals.modem_lpcon_shared_clock,
            ),
            ModemClockDevice::Etm => etm,
            ModemClockDevice::BluetoothApb => bt_apb && security_apb,
            ModemClockDevice::BluetoothIeee802154CommonBaseband => btbb,
            ModemClockDevice::Ieee802154Mac => zb_apb && zbmac,
        }
    }

    /// Pulse the IEEE 802.15.4 MAC reset, then its APB reset, as
    /// `modem_clock_module_mac_reset` does: each line is written asserted and
    /// then released.
    pub fn pulse_ieee802154_mac_reset(&mut self) {
        let syscon = &self.peripherals.modem_syscon_radio;
        generated::set_ieee802154_mac_reset(syscon, ModemResetState::Asserted);
        generated::set_ieee802154_mac_reset(syscon, ModemResetState::Released);
        generated::set_ieee802154_apb_reset(syscon, ModemResetState::Asserted);
        generated::set_ieee802154_apb_reset(syscon, ModemResetState::Released);
    }

    /// Whether the analog register I2C master clock is enabled. The platform
    /// owns that gate and its reference count.
    pub fn analog_i2c_master_clock_enabled(&self) -> bool {
        svd::field_read::observe_analog_i2c_master_clock(&self.peripherals.modem_lpcon_shared_clock)
    }

    /// Sample both IEEE 802.15.4 reset lines.
    pub fn ieee802154_reset_observation(&self) -> Ieee802154ResetObservation {
        let (apb_asserted, mac_asserted) = svd::field_snapshot_read::observe_ieee802154_resets(
            &self.peripherals.modem_syscon_radio,
        );
        Ieee802154ResetObservation {
            mac_released: !mac_asserted,
            apb_released: !apb_asserted,
        }
    }
}
