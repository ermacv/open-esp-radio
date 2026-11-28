//! Target executor for the shared modem clock planner.
//!
//! The planner decides which dependency edges a module request needs; this
//! executor performs each edge and only then records it complete. Modem gates
//! go to the radio PAC. The upstream 160 MHz source, the analog-I2C master
//! clock and the coexistence clock are the radio domain's platform clocks
//! ([`PlatformClocks`]), which count their holders.
//!
//! For the modem PLL-source dependency the executor takes the 160 MHz
//! source before opening the modem gate and gives it back after closing the
//! gate, in the vendor order. Each platform hold lives in
//! [`ModemPlatformClocks`] while its dependency is enabled.

use crate::{
    power::platform::{PlatformClock, PlatformGates, PlatformHold},
    radio::modem::ModemClockDevice,
    radio::phy::RadioPhyRegisters,
};

use super::Dependency;

/// The platform holds the modem clock planner's dependency edges keep, one
/// per platform-owned dependency while it is enabled.
#[derive(Debug, Default)]
pub(crate) struct ModemPlatformClocks {
    pll_f160m: Option<PlatformHold>,
    analog_i2c: Option<PlatformHold>,
    coexistence: Option<PlatformHold>,
}

impl ModemPlatformClocks {
    fn slot(&mut self, clock: PlatformClock) -> &mut Option<PlatformHold> {
        match clock {
            PlatformClock::Pll160m => &mut self.pll_f160m,
            PlatformClock::AnalogI2cMaster => &mut self.analog_i2c,
            PlatformClock::ModemCoexistence => &mut self.coexistence,
        }
    }

    /// Take `clock` for its dependency, which the planner enables once.
    fn acquire(&mut self, clock: PlatformClock, platform: &mut impl PlatformGates) {
        let slot = self.slot(clock);
        if slot.is_none() {
            *slot = Some(platform.acquire(clock));
        }
    }

    /// Give back the hold `clock`'s dependency keeps.
    fn release(&mut self, clock: PlatformClock, platform: &mut impl PlatformGates) {
        if let Some(hold) = self.slot(clock).take() {
            platform.release(hold);
        }
    }
}

/// Modem clock gates driven by the radio PAC.
pub(crate) trait ModemClockPort {
    /// Publish the active modem state and its clock maps before any module
    /// gate is opened, including the PHY's pre-client registration clocks.
    fn prepare_power_state(&mut self);
    fn enable_device(&mut self, device: ModemClockDevice);
    fn disable_device(&mut self, device: ModemClockDevice);
}

impl<P: crate::radio::phy::registers::RadioPlacement> ModemClockPort for RadioPhyRegisters<P> {
    fn prepare_power_state(&mut self) {
        self.prepare_modem_syscon_clock_map();
        self.prepare_shared_modem_clock_map();
        self.publish_modem_power_state();
    }

    fn enable_device(&mut self, device: ModemClockDevice) {
        self.enable_modem_clock_device(device);
    }

    fn disable_device(&mut self, device: ModemClockDevice) {
        self.disable_modem_clock_device(device);
    }
}

/// Borrowed chip binding; this owner lends no register handle to the family.
pub(crate) struct ModemExecution<'a, H, G> {
    pub(crate) port: &'a mut H,
    pub(crate) held: &'a mut ModemPlatformClocks,
    pub(crate) platform: &'a mut G,
}

impl<H: ModemClockPort, G: PlatformGates>
    oer_chips_family_radio_modem_clock::execution::RegisterPort<Dependency>
    for ModemExecution<'_, H, G>
{
    type Device = ModemClockDevice;
    type Clock = PlatformClock;
    type Error = core::convert::Infallible;
    fn action(
        dependency: Dependency,
    ) -> oer_chips_family_radio_modem_clock::execution::Action<ModemClockDevice, PlatformClock>
    {
        use super::ChipDependency as D;
        use oer_chips_family_radio_modem_clock::execution::Action;
        Action::Device(match dependency.0 {
            D::ModemAdcCommonFe => ModemClockDevice::ModemAdcCommonFe,
            D::ModemPrivateFe => ModemClockDevice::ModemPrivateFe,
            D::WifiApb => ModemClockDevice::WifiApb,
            D::WifiBb44m => ModemClockDevice::WifiBaseband44m,
            D::WifiMac => ModemClockDevice::WifiMac,
            D::WifiBb => ModemClockDevice::WifiBaseband,
            D::WifiBb80x1 => ModemClockDevice::WifiBaseband80x1,
            D::Etm => ModemClockDevice::Etm,
            D::BtMac => ModemClockDevice::BluetoothMac,
            D::BtPeripheral => ModemClockDevice::BluetoothPeripheral,
            D::BtApbAndSecurity => ModemClockDevice::BluetoothApb,
            D::BtIeee802154CommonBaseband => ModemClockDevice::BluetoothIeee802154CommonBaseband,
            D::Ieee802154ApbAndMac => ModemClockDevice::Ieee802154Mac,
            D::Pll160AndModemSource => {
                return Action::Source {
                    clock: PlatformClock::Pll160m,
                    device: ModemClockDevice::PllSourceGate,
                };
            }
            D::AnalogI2cMaster => return Action::Platform(PlatformClock::AnalogI2cMaster),
            D::Coexistence => return Action::Platform(PlatformClock::ModemCoexistence),
        })
    }
    // The family port's one translation of its flag into the two actions.
    fn configure_device(&mut self, device: ModemClockDevice, enable: bool) {
        if enable {
            self.port.enable_device(device);
        } else {
            self.port.disable_device(device);
        }
    }
    fn release_clock(&mut self, clock: PlatformClock) -> Result<(), core::convert::Infallible> {
        self.held.release(clock, self.platform);
        Ok(())
    }
}
impl<H: ModemClockPort, G: PlatformGates>
    oer_chips_family_radio_modem_clock::execution::AcquirePort<Dependency>
    for ModemExecution<'_, H, G>
{
    fn prepare_power_state(&mut self) {
        self.port.prepare_power_state();
    }
    fn acquire_clock(&mut self, clock: PlatformClock) -> Result<(), core::convert::Infallible> {
        self.held.acquire(clock, self.platform);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
