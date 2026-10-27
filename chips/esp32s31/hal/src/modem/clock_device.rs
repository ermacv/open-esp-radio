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
//! `modem_syscon_ll.h`), `ESP_IDF_4D59230D_S31_MODEM_PLL_GATE` for the PLL
//! gate sequence, and `S31_MODEM_LPCON_STRUCT` for the coexistence gate.

#![forbid(unsafe_code)]

use crate::{
    modem::values::{ModemSysconClockGateState, ModemSysconResetState},
    phy::RadioPhyRegisters,
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

impl<P: crate::phy::registers::RadioPlacement> RadioPhyRegisters<P> {
    /// Apply one vendor modem clock device action.
    ///
    /// The write order within each action is the vendor order. Front-end
    /// devices perform no register access when disabled, as in the vendor.
    #[doc(hidden)]
    pub fn configure_modem_clock_device(&mut self, device: ModemClockDevice, enable: bool) {
        let syscon = &mut self.peripherals.modem_syscon_clock_reset;
        match device {
            ModemClockDevice::ModemAdcCommonFe => {
                if enable {
                    super::ops::enable_modem_frontend_apb_clock(syscon);
                    super::ops::enable_modem_frontend_80m_clock(syscon);
                }
            }
            ModemClockDevice::ModemPrivateFe => {
                if enable {
                    super::ops::enable_modem_frontend_160m_clock(syscon);
                    super::ops::enable_modem_frontend_dac_clock(syscon);
                    super::ops::enable_modem_frontend_pwdet_clock(syscon);
                    super::ops::enable_modem_frontend_adc_clock(syscon);
                }
            }
            ModemClockDevice::PllSourceGate => {
                configure_pll_source_gate(
                    &mut PllSourceGate(&mut self.peripherals.hp_sys_clkrst_radio),
                    enable,
                );
            }
            ModemClockDevice::WifiApb => super::ops::set_modem_wifi_apb_clock(syscon, gate(enable)),
            ModemClockDevice::WifiBaseband44m => {
                super::ops::set_modem_wifi_baseband_44m_clock(syscon, gate(enable));
            }
            ModemClockDevice::WifiMac => super::ops::set_modem_wifi_mac_clock(syscon, gate(enable)),
            ModemClockDevice::WifiBaseband => {
                configure_wifi_baseband(&mut BasebandClock(syscon), enable);
            }
            ModemClockDevice::WifiBaseband80x1 => {
                super::ops::set_bluetooth_wifi_baseband_80x1_clock(syscon, gate(enable));
            }
            ModemClockDevice::Etm => super::ops::set_bluetooth_etm_clock(syscon, gate(enable)),
            ModemClockDevice::BluetoothMac => {
                super::ops::set_bluetooth_mac_clock(syscon, gate(enable));
            }
            ModemClockDevice::BluetoothPeripheral => {
                super::ops::set_bluetooth_peripheral_clocks(syscon, gate(enable));
            }
            ModemClockDevice::BluetoothApb => {
                super::ops::set_bluetooth_apb_clock(syscon, gate(enable));
                super::ops::set_bluetooth_modem_security_apb_clock(syscon, gate(enable));
            }
            ModemClockDevice::BluetoothIeee802154CommonBaseband => {
                super::ops::set_bluetooth_baseband_clock(syscon, gate(enable));
            }
            ModemClockDevice::Ieee802154Mac => {
                super::ops::set_modem_ieee802154_apb_clock(syscon, gate(enable));
                super::ops::set_modem_ieee802154_mac_clock(syscon, gate(enable));
            }
        }
    }
}

pub(super) trait PllSourceGatePort {
    fn prepare(&mut self, enabled: bool);
    fn select(&mut self, enabled: bool);
}

pub(super) struct PllSourceGate<'a, P: oer_mmio::Placement>(
    pub(super) &'a mut oer_esp32s31_pac::hp_sys_clkrst::HpSysClkrstRadio<P>,
);

impl<P: oer_mmio::Placement> PllSourceGatePort for PllSourceGate<'_, P> {
    fn prepare(&mut self, enabled: bool) {
        if enabled {
            self.0.prepare_open_modem_pll_source_gate();
        } else {
            self.0.prepare_close_modem_pll_source_gate();
        }
    }

    fn select(&mut self, enabled: bool) {
        if enabled {
            self.0.open_modem_pll_source_gate();
        } else {
            self.0.close_modem_pll_source_gate();
        }
    }
}

// SOURCE: ESP_IDF_4D59230D_S31_MODEM_PLL_GATE. Both edges use two whole-word
// stores, with preparation before source selection; no RMW or delay intervenes.
pub(super) fn configure_pll_source_gate(port: &mut impl PllSourceGatePort, enabled: bool) {
    port.prepare(enabled);
    port.select(enabled);
}

trait BasebandClockPort {
    fn reset(&mut self, asserted: bool);
    fn clock(&mut self, enabled: bool);
}
struct BasebandClock<'a, P: oer_mmio::Placement>(
    &'a mut oer_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
);
impl<P: oer_mmio::Placement> BasebandClockPort for BasebandClock<'_, P> {
    fn reset(&mut self, asserted: bool) {
        super::ops::set_wifi_baseband_reset(
            self.0,
            if asserted {
                ModemSysconResetState::Asserted
            } else {
                ModemSysconResetState::Released
            },
        );
    }
    fn clock(&mut self, enabled: bool) {
        super::ops::set_modem_wifi_baseband_clocks(self.0, gate(enabled));
    }
}
// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK. Reset precedes the enabling gate edge.
fn configure_wifi_baseband(port: &mut impl BasebandClockPort, enabled: bool) {
    if enabled {
        port.reset(true);
        port.reset(false);
    }
    port.clock(enabled);
}
#[cfg(test)]
mod tests {
    use super::*;

    impl PllSourceGatePort for Port {
        fn prepare(&mut self, enabled: bool) {
            self.0.push(("prepare", enabled));
        }
        fn select(&mut self, enabled: bool) {
            self.0.push(("select", enabled));
        }
    }

    #[test]
    fn pll_gate_prepares_each_edge_before_selecting_its_source() {
        let mut port = Port::default();
        for enabled in [true, false, true] {
            configure_pll_source_gate(&mut port, enabled);
        }
        assert_eq!(
            port.0,
            [
                ("prepare", true),
                ("select", true),
                ("prepare", false),
                ("select", false),
                ("prepare", true),
                ("select", true),
            ]
        );
    }

    #[test]
    fn pinned_pll_gate_opens_without_xtal_and_closes_on_xtal() {
        use oer_esp32s31_pac::hp_sys_clkrst::{HpSysClkrstRadio, HpSysClkrstRadioWords};
        use oer_mmio::Registers;

        let words = HpSysClkrstRadioWords::WORDS.iter().copied().max().unwrap() / 4 + 1;
        let mut memory = std::vec![u32::MAX; words];
        let mut registers = HpSysClkrstRadio::host(&mut memory).unwrap();
        for enabled in [true, false, true] {
            configure_pll_source_gate(&mut PllSourceGate(&mut registers), enabled);
            let observed = registers.read_modem_conf();
            assert!(observed.modem_apb_clk_en());
            assert!(!observed.modem_rst_en());
            assert!(observed.modem_clk_en());
            assert_eq!(observed.modem_clk_source_sel(), enabled);
            assert_eq!(observed.modem_pll_clk_en(), enabled);
            assert_eq!(observed.modem_xtal_clk_en(), !enabled);
        }
    }
    #[derive(Default)]
    struct Port(std::vec::Vec<(&'static str, bool)>);
    impl BasebandClockPort for Port {
        fn reset(&mut self, asserted: bool) {
            self.0.push(("reset", asserted));
        }
        fn clock(&mut self, enabled: bool) {
            self.0.push(("clock", enabled));
        }
    }
    #[test]
    fn wifi_baseband_pulses_reset_before_enabling_clocks_and_never_resets_on_disable() {
        let mut port = Port::default();
        configure_wifi_baseband(&mut port, true);
        configure_wifi_baseband(&mut port, false);
        assert_eq!(
            port.0,
            [
                ("reset", true),
                ("reset", false),
                ("clock", true),
                ("clock", false)
            ]
        );
    }
}
