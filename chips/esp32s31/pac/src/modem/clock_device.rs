//! Vendor modem clock devices driven on one reference-count edge.
//!
//! Each variant is one ESP-IDF `modem_clock_*_configure(enable)` action for
//! ESP32-S31, applied only at a zero-to-one or one-to-zero dependency edge by
//! the HAL clock executor. The platform-owned halves of two devices stay
//! outside the PAC: the upstream 160 MHz source acquired around the PLL-source
//! gate, and the analog-I2C master clock.
//!
//! SOURCE: reviewed evidence `ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK`
//! (`modem_clock_impl.c` configure actions, `modem_clock_hal.c`,
//! `modem_syscon_ll.h`) and `S31_MODEM_LPCON_STRUCT` for the coexistence gate.

#![forbid(unsafe_code)]

use crate::{
    RadioPhyRegisters,
    generated::{self, ModemSysconClockGateState, ModemSysconResetState},
};

/// One modem clock device whose gate the PAC drives.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModemClockDevice {
    /// Front-end APB and 80 MHz clocks; the vendor never disables them.
    ModemAdcCommonFe,
    /// Front-end 160 MHz, DAC, power-detector and ADC clocks; never disabled.
    ModemPrivateFe,
    /// Modem PLL-source gate (`HP_SYS_CLKRST.MODEM_CONF` images).
    PllSourceGate,
    WifiApb,
    WifiBaseband44m,
    WifiMac,
    /// Wi-Fi baseband clock group; enabling first pulses the baseband reset.
    WifiBaseband,
    WifiBaseband80x1,
    Etm,
    BluetoothMac,
    /// Modem security and BLE timer clocks.
    BluetoothPeripheral,
    /// Bluetooth APB and modem security APB clocks.
    BluetoothApb,
    BluetoothIeee802154CommonBaseband,
    /// IEEE 802.15.4 APB and MAC clocks.
    Ieee802154Mac,
}

const fn gate(enable: bool) -> ModemSysconClockGateState {
    if enable {
        ModemSysconClockGateState::Enabled
    } else {
        ModemSysconClockGateState::Disabled
    }
}

impl RadioPhyRegisters {
    /// Apply one vendor modem clock device action.
    ///
    /// The write order within each action is the vendor order. Front-end
    /// devices perform no register access when disabled, as in the vendor.
    #[doc(hidden)]
    pub fn configure_modem_clock_device(&mut self, device: ModemClockDevice, enable: bool) {
        let syscon = &self.peripherals.modem_syscon_radio;
        match device {
            ModemClockDevice::ModemAdcCommonFe => {
                if enable {
                    generated::enable_modem_frontend_apb_clock(syscon);
                    generated::enable_modem_frontend_80m_clock(syscon);
                }
            }
            ModemClockDevice::ModemPrivateFe => {
                if enable {
                    generated::enable_modem_frontend_160m_clock(syscon);
                    generated::enable_modem_frontend_dac_clock(syscon);
                    generated::enable_modem_frontend_pwdet_clock(syscon);
                    generated::enable_modem_frontend_adc_clock(syscon);
                }
            }
            ModemClockDevice::PllSourceGate => {
                let clkrst = &self.peripherals.hp_sys_clkrst_radio;
                if enable {
                    crate::svd::fixed_register_image::open_modem_pll_source_gate(clkrst);
                } else {
                    crate::svd::fixed_register_image::close_modem_pll_source_gate(clkrst);
                }
            }
            ModemClockDevice::WifiApb => generated::set_modem_wifi_apb_clock(syscon, gate(enable)),
            ModemClockDevice::WifiBaseband44m => {
                generated::set_modem_wifi_baseband_44m_clock(syscon, gate(enable));
            }
            ModemClockDevice::WifiMac => generated::set_modem_wifi_mac_clock(syscon, gate(enable)),
            ModemClockDevice::WifiBaseband => {
                if enable {
                    generated::set_wifi_baseband_reset(syscon, ModemSysconResetState::Asserted);
                    generated::set_wifi_baseband_reset(syscon, ModemSysconResetState::Released);
                }
                generated::set_modem_wifi_baseband_clocks(syscon, gate(enable));
            }
            ModemClockDevice::WifiBaseband80x1 => {
                generated::set_bluetooth_wifi_baseband_80x1_clock(syscon, gate(enable));
            }
            ModemClockDevice::Etm => generated::set_bluetooth_etm_clock(syscon, gate(enable)),
            ModemClockDevice::BluetoothMac => {
                generated::set_bluetooth_mac_clock(syscon, gate(enable));
            }
            ModemClockDevice::BluetoothPeripheral => {
                generated::set_bluetooth_peripheral_clocks(syscon, gate(enable));
            }
            ModemClockDevice::BluetoothApb => {
                generated::set_bluetooth_apb_clock(syscon, gate(enable));
                generated::set_bluetooth_modem_security_apb_clock(syscon, gate(enable));
            }
            ModemClockDevice::BluetoothIeee802154CommonBaseband => {
                generated::set_bluetooth_baseband_clock(syscon, gate(enable));
            }
            ModemClockDevice::Ieee802154Mac => {
                generated::set_modem_ieee802154_apb_clock(syscon, gate(enable));
                generated::set_modem_ieee802154_mac_clock(syscon, gate(enable));
            }
        }
    }
}
