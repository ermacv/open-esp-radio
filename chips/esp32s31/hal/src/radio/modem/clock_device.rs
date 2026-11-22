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

use crate::radio::phy::RadioPhyRegisters;

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

use oer_chips_esp32s31_pac::modem_syscon::dictionaries::{ClockGate, Reset};

impl<P: crate::radio::phy::registers::RadioPlacement> RadioPhyRegisters<P> {
    /// Apply one vendor modem clock device's enabling action, in the vendor's
    /// write order.
    #[doc(hidden)]
    pub fn enable_modem_clock_device(&mut self, device: ModemClockDevice) {
        let syscon = &mut self.peripherals.modem_syscon_clock_reset;
        match device {
            ModemClockDevice::ModemAdcCommonFe => {
                super::ops::enable_modem_frontend_apb_clock(syscon);
                super::ops::enable_modem_frontend_80m_clock(syscon);
            }
            ModemClockDevice::ModemPrivateFe => {
                super::ops::enable_modem_frontend_160m_clock(syscon);
                super::ops::enable_modem_frontend_dac_clock(syscon);
                super::ops::enable_modem_frontend_pwdet_clock(syscon);
                super::ops::enable_modem_frontend_adc_clock(syscon);
            }
            ModemClockDevice::PllSourceGate => {
                open_pll_source_gate(&mut PllSourceGate(
                    &mut self.peripherals.hp_sys_clkrst_radio,
                ));
            }
            ModemClockDevice::WifiBaseband => {
                enable_wifi_baseband(&mut BasebandClock(syscon));
            }
            gated => set_gated_device(syscon, gated, ClockGate::Enabled),
        }
    }

    /// Apply one vendor modem clock device's disabling action, in the vendor's
    /// write order. Front-end devices perform no register access, as in the
    /// vendor.
    #[doc(hidden)]
    pub fn disable_modem_clock_device(&mut self, device: ModemClockDevice) {
        let syscon = &mut self.peripherals.modem_syscon_clock_reset;
        match device {
            ModemClockDevice::ModemAdcCommonFe | ModemClockDevice::ModemPrivateFe => {}
            ModemClockDevice::PllSourceGate => {
                close_pll_source_gate(&mut PllSourceGate(
                    &mut self.peripherals.hp_sys_clkrst_radio,
                ));
            }
            ModemClockDevice::WifiBaseband => {
                disable_wifi_baseband(&mut BasebandClock(syscon));
            }
            gated => set_gated_device(syscon, gated, ClockGate::Disabled),
        }
    }
}

/// The devices that are gates alone, written with `gate`: each field is one
/// `ClockGate` the vendor sets on enable and clears on disable.
fn set_gated_device<P: oer_portable_mmio::Placement>(
    syscon: &mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
    device: ModemClockDevice,
    gate: ClockGate,
) {
    match device {
        ModemClockDevice::WifiApb => super::ops::set_modem_wifi_apb_clock(syscon, gate),
        ModemClockDevice::WifiBaseband44m => {
            super::ops::set_modem_wifi_baseband_44m_clock(syscon, gate);
        }
        ModemClockDevice::WifiMac => super::ops::set_modem_wifi_mac_clock(syscon, gate),
        ModemClockDevice::WifiBaseband80x1 => {
            super::ops::set_bluetooth_wifi_baseband_80x1_clock(syscon, gate);
        }
        ModemClockDevice::Etm => super::ops::set_bluetooth_etm_clock(syscon, gate),
        ModemClockDevice::BluetoothMac => super::ops::set_bluetooth_mac_clock(syscon, gate),
        ModemClockDevice::BluetoothPeripheral => {
            super::ops::set_bluetooth_peripheral_clocks(syscon, gate);
        }
        ModemClockDevice::BluetoothApb => {
            super::ops::set_bluetooth_apb_clock(syscon, gate);
            super::ops::set_bluetooth_modem_security_apb_clock(syscon, gate);
        }
        ModemClockDevice::BluetoothIeee802154CommonBaseband => {
            super::ops::set_bluetooth_baseband_clock(syscon, gate);
        }
        ModemClockDevice::Ieee802154Mac => {
            super::ops::set_modem_ieee802154_apb_clock(syscon, gate);
            super::ops::set_modem_ieee802154_mac_clock(syscon, gate);
        }
        ModemClockDevice::ModemAdcCommonFe
        | ModemClockDevice::ModemPrivateFe
        | ModemClockDevice::PllSourceGate
        | ModemClockDevice::WifiBaseband => {}
    }
}

pub(super) trait PllSourceGatePort {
    fn prepare_open(&mut self);
    fn open(&mut self);
    fn prepare_close(&mut self);
    fn close(&mut self);
}

pub(super) struct PllSourceGate<'a, P: oer_portable_mmio::Placement>(
    pub(super) &'a mut oer_chips_esp32s31_pac::hp_sys_clkrst::HpSysClkrstRadio<P>,
);

impl<P: oer_portable_mmio::Placement> PllSourceGatePort for PllSourceGate<'_, P> {
    fn prepare_open(&mut self) {
        self.0.prepare_open_modem_pll_source_gate();
    }
    fn open(&mut self) {
        self.0.open_modem_pll_source_gate();
    }
    fn prepare_close(&mut self) {
        self.0.prepare_close_modem_pll_source_gate();
    }
    fn close(&mut self) {
        self.0.close_modem_pll_source_gate();
    }
}

// SOURCE: ESP_IDF_4D59230D_S31_MODEM_PLL_GATE. Both edges use two whole-word
// stores, with preparation before source selection; no RMW or delay intervenes.
pub(super) fn open_pll_source_gate(port: &mut impl PllSourceGatePort) {
    port.prepare_open();
    port.open();
}
pub(super) fn close_pll_source_gate(port: &mut impl PllSourceGatePort) {
    port.prepare_close();
    port.close();
}

trait BasebandClockPort {
    fn reset(&mut self, reset: Reset);
    fn clock(&mut self, gate: ClockGate);
}
struct BasebandClock<'a, P: oer_portable_mmio::Placement>(
    &'a mut oer_chips_esp32s31_pac::modem_syscon::ModemSysconClockReset<P>,
);
impl<P: oer_portable_mmio::Placement> BasebandClockPort for BasebandClock<'_, P> {
    fn reset(&mut self, reset: Reset) {
        super::ops::set_wifi_baseband_reset(self.0, reset);
    }
    fn clock(&mut self, gate: ClockGate) {
        super::ops::set_modem_wifi_baseband_clocks(self.0, gate);
    }
}
// SOURCE: ESP_IDF_7B9CC1AC_S31_MODEM_CLOCK. Reset precedes the enabling gate
// edge; the disabling edge does not reset.
fn enable_wifi_baseband(port: &mut impl BasebandClockPort) {
    port.reset(Reset::Asserted);
    port.reset(Reset::Released);
    port.clock(ClockGate::Enabled);
}
fn disable_wifi_baseband(port: &mut impl BasebandClockPort) {
    port.clock(ClockGate::Disabled);
}
#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct GatePort(std::vec::Vec<&'static str>);
    impl PllSourceGatePort for GatePort {
        fn prepare_open(&mut self) {
            self.0.push("prepare open");
        }
        fn open(&mut self) {
            self.0.push("open");
        }
        fn prepare_close(&mut self) {
            self.0.push("prepare close");
        }
        fn close(&mut self) {
            self.0.push("close");
        }
    }

    #[test]
    fn pll_gate_prepares_each_edge_before_selecting_its_source() {
        let mut port = GatePort::default();
        open_pll_source_gate(&mut port);
        close_pll_source_gate(&mut port);
        open_pll_source_gate(&mut port);
        assert_eq!(
            port.0,
            [
                "prepare open",
                "open",
                "prepare close",
                "close",
                "prepare open",
                "open"
            ]
        );
    }

    #[test]
    fn pinned_pll_gate_opens_without_xtal_and_closes_on_xtal() {
        use oer_chips_esp32s31_pac::hp_sys_clkrst::{HpSysClkrstRadio, HpSysClkrstRadioWords};
        use oer_portable_mmio::Registers;

        let words = HpSysClkrstRadioWords::WORDS.iter().copied().max().unwrap() / 4 + 1;
        let mut memory = std::vec![u32::MAX; words];
        let mut registers = HpSysClkrstRadio::host(&mut memory).unwrap();
        for enabled in [true, false, true] {
            if enabled {
                open_pll_source_gate(&mut PllSourceGate(&mut registers));
            } else {
                close_pll_source_gate(&mut PllSourceGate(&mut registers));
            }
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
    struct Port(std::vec::Vec<&'static str>);
    impl BasebandClockPort for Port {
        fn reset(&mut self, reset: Reset) {
            self.0.push(match reset {
                Reset::Asserted => "assert reset",
                Reset::Released => "release reset",
            });
        }
        fn clock(&mut self, gate: ClockGate) {
            self.0.push(match gate {
                ClockGate::Enabled => "enable clock",
                ClockGate::Disabled => "disable clock",
            });
        }
    }
    #[test]
    fn wifi_baseband_pulses_reset_before_enabling_clocks_and_never_resets_on_disable() {
        let mut port = Port::default();
        enable_wifi_baseband(&mut port);
        disable_wifi_baseband(&mut port);
        assert_eq!(
            port.0,
            [
                "assert reset",
                "release reset",
                "enable clock",
                "disable clock"
            ]
        );
    }
}
