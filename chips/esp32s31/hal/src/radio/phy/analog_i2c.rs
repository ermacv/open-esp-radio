//! Owned PMU atoms and the family analog-I2C power port.

use crate::system::owner::SharedPhyAccess;
use oer_chips_family_radio_phy::analog_i2c::AnalogI2cPowerAccess;

/// Exclusive borrowed PMU atoms; no register or restore authority escapes.
pub struct PowerPort<'a, T: ?Sized>(&'a mut T);

/// Borrow the current PHY owner's power atoms for a family operation.
pub fn power_port<T: SharedPhyAccess + ?Sized>(owner: &mut T) -> PowerPort<'_, T> {
    PowerPort(owner)
}

/// The analog I2C's power as read: `ANA_PERI_PWR_CTRL.XPD_PERIF_I2C`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AnalogI2cPower {
    PoweredOff,
    PoweredOn,
}

/// The analog I2C's reset as read: `ANA_PERI_PWR_CTRL.RSTB_PERIF_I2C`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum AnalogI2cReset {
    Asserted,
    Released,
}

impl AnalogI2cPower {
    const fn from_read(powered: bool) -> Self {
        if powered {
            Self::PoweredOn
        } else {
            Self::PoweredOff
        }
    }
}

impl AnalogI2cReset {
    const fn from_read(released: bool) -> Self {
        if released {
            Self::Released
        } else {
            Self::Asserted
        }
    }
}

/// The PMU atoms and reads the family port's translation calls: every shared
/// PHY owner has them, and a host test lends the PHY registers alone.
pub(crate) trait AnalogI2cPowerAtoms {
    fn power_on_rf_circuits(&mut self);
    fn power_off_rf_circuits(&mut self);
    fn tie_baseband_i2c_power(&mut self);
    fn analog_i2c_power(&self) -> AnalogI2cPower;
    fn power_on_analog_i2c(&mut self);
    fn power_off_analog_i2c(&mut self);
    fn analog_i2c_reset(&self) -> AnalogI2cReset;
    fn release_analog_i2c_reset(&mut self);
    fn assert_analog_i2c_reset(&mut self);
}

impl<T: SharedPhyAccess + ?Sized> AnalogI2cPowerAtoms for T {
    fn power_on_rf_circuits(&mut self) {
        SharedPhyAccess::power_on_rf_circuits(self);
    }
    fn power_off_rf_circuits(&mut self) {
        SharedPhyAccess::power_off_rf_circuits(self);
    }
    fn tie_baseband_i2c_power(&mut self) {
        SharedPhyAccess::tie_baseband_i2c_power(self);
    }
    fn analog_i2c_power(&self) -> AnalogI2cPower {
        AnalogI2cPower::from_read(SharedPhyAccess::analog_i2c_is_powered(self))
    }
    fn power_on_analog_i2c(&mut self) {
        SharedPhyAccess::power_on_analog_i2c(self);
    }
    fn power_off_analog_i2c(&mut self) {
        SharedPhyAccess::power_off_analog_i2c(self);
    }
    fn analog_i2c_reset(&self) -> AnalogI2cReset {
        AnalogI2cReset::from_read(SharedPhyAccess::analog_i2c_reset_is_released(self))
    }
    fn release_analog_i2c_reset(&mut self) {
        SharedPhyAccess::release_analog_i2c_reset(self);
    }
    fn assert_analog_i2c_reset(&mut self) {
        SharedPhyAccess::assert_analog_i2c_reset(self);
    }
}

#[cfg(test)]
impl<P: crate::radio::phy::registers::RadioPlacement> AnalogI2cPowerAtoms
    for crate::radio::phy::RadioPhyRegisters<P>
{
    fn power_on_rf_circuits(&mut self) {
        Self::power_on_rf_circuits(self);
    }
    fn power_off_rf_circuits(&mut self) {
        Self::power_off_rf_circuits(self);
    }
    fn tie_baseband_i2c_power(&mut self) {
        Self::tie_baseband_i2c_power(self);
    }
    fn analog_i2c_power(&self) -> AnalogI2cPower {
        AnalogI2cPower::from_read(Self::analog_i2c_is_powered(self))
    }
    fn power_on_analog_i2c(&mut self) {
        Self::power_on_analog_i2c(self);
    }
    fn power_off_analog_i2c(&mut self) {
        Self::power_off_analog_i2c(self);
    }
    fn analog_i2c_reset(&self) -> AnalogI2cReset {
        AnalogI2cReset::from_read(Self::analog_i2c_reset_is_released(self))
    }
    fn release_analog_i2c_reset(&mut self) {
        Self::release_analog_i2c_reset(self);
    }
    fn assert_analog_i2c_reset(&mut self) {
        Self::assert_analog_i2c_reset(self);
    }
}

// The family port's one translation of its flags into the atoms.
impl<T: AnalogI2cPowerAtoms + ?Sized> AnalogI2cPowerAccess for PowerPort<'_, T> {
    fn set_rf_circuit_power(&mut self, enabled: bool) {
        if enabled {
            self.0.power_on_rf_circuits();
        } else {
            self.0.power_off_rf_circuits();
        }
    }

    /// The tie is a write trigger: its release writes nothing (the vendor's
    /// clearing RMW has no counterpart in that access class).
    fn set_bb_i2c_power_tie(&mut self, enabled: bool) {
        if enabled {
            self.0.tie_baseband_i2c_power();
        }
    }

    fn analog_i2c_is_powered(&self) -> bool {
        self.0.analog_i2c_power() == AnalogI2cPower::PoweredOn
    }

    fn set_analog_i2c_power(&mut self, enabled: bool) {
        if enabled {
            self.0.power_on_analog_i2c();
        } else {
            self.0.power_off_analog_i2c();
        }
    }

    fn analog_i2c_reset_is_released(&self) -> bool {
        self.0.analog_i2c_reset() == AnalogI2cReset::Released
    }

    fn set_analog_i2c_reset_released(&mut self, released: bool) {
        if released {
            self.0.release_analog_i2c_reset();
        } else {
            self.0.assert_analog_i2c_reset();
        }
    }
}

/// Clear the RF-circuit power image inside complete `phy_xpd_rf_new`.
///
/// The surrounding close transaction owns the adjacent baseband, I2C and
/// clock operations; this leaf performs only the instruction-evidenced PMU
/// update.
pub fn power_off_rf_circuits(registers: &mut impl SharedPhyAccess) {
    SharedPhyAccess::power_off_rf_circuits(registers);
}

/// Complete the ROM frontend/baseband clock leaf after internal radio gates.
///
/// SOURCE\[complete rev0 ROM `phy_open_fe_bb_clk`]. The PMU update is the
/// fourth and final operation; the first three undocumented radio gates are
/// executed by the recovered radio PAC before this call.
pub fn enable_frontend_baseband_power(registers: &mut impl SharedPhyAccess) {
    SharedPhyAccess::enable_frontend_baseband_power(registers);
}

#[cfg(test)]
mod tests {
    use super::{AnalogI2cPowerAccess, PowerPort};
    use crate::radio::phy::RadioPhyRegisters;
    use crate::radio::phy::registers::{HostPlacement, host::with_registers_memory};

    type Registers<'a> = RadioPhyRegisters<HostPlacement<'a>>;

    /// The memory one run leaves, from `fill`.
    fn image(
        fill: u32,
        run: impl FnOnce(&mut Registers<'_>),
    ) -> std::vec::Vec<(&'static str, std::vec::Vec<u32>)> {
        with_registers_memory(fill, run).1
    }

    /// Each flag of the family port writes what its atom writes.
    #[test]
    fn each_flag_of_the_family_port_selects_its_atom() {
        let cases: [(fn(&mut Registers<'_>), fn(&mut Registers<'_>)); 7] = [
            (
                |r| PowerPort(r).set_rf_circuit_power(true),
                |r| r.power_on_rf_circuits(),
            ),
            (
                |r| PowerPort(r).set_rf_circuit_power(false),
                |r| r.power_off_rf_circuits(),
            ),
            (
                |r| PowerPort(r).set_bb_i2c_power_tie(true),
                |r| r.tie_baseband_i2c_power(),
            ),
            (
                |r| PowerPort(r).set_analog_i2c_power(true),
                |r| r.power_on_analog_i2c(),
            ),
            (
                |r| PowerPort(r).set_analog_i2c_power(false),
                |r| r.power_off_analog_i2c(),
            ),
            (
                |r| PowerPort(r).set_analog_i2c_reset_released(true),
                |r| r.release_analog_i2c_reset(),
            ),
            (
                |r| PowerPort(r).set_analog_i2c_reset_released(false),
                |r| r.assert_analog_i2c_reset(),
            ),
        ];
        for fill in [0, u32::MAX] {
            let images: std::vec::Vec<_> = cases
                .iter()
                .map(|(port, atom)| (image(fill, port), image(fill, atom)))
                .collect();
            for (index, (by_port, by_atom)) in images.iter().enumerate() {
                assert!(by_port == by_atom, "case {index}, fill {fill:#x}");
            }
            // Each switch's two flags write different images.
            for (on, off) in [(0, 1), (3, 4), (5, 6)] {
                assert!(
                    images[on].0 != images[off].0,
                    "cases {on} and {off}, fill {fill:#x}"
                );
            }
        }
    }

    /// The tie's release writes nothing; the reads report the atoms' writes.
    #[test]
    fn the_tie_release_writes_nothing_and_the_reads_follow_the_writes() {
        for fill in [0, u32::MAX] {
            assert_eq!(
                image(fill, |r| PowerPort(r).set_bb_i2c_power_tie(false)),
                image(fill, |_| {})
            );
        }
        let ((), _) = with_registers_memory(0, |r| {
            let mut port = PowerPort(r);
            port.set_analog_i2c_power(true);
            assert!(port.analog_i2c_is_powered());
            port.set_analog_i2c_power(false);
            assert!(!port.analog_i2c_is_powered());
            port.set_analog_i2c_reset_released(true);
            assert!(port.analog_i2c_reset_is_released());
            port.set_analog_i2c_reset_released(false);
            assert!(!port.analog_i2c_reset_is_released());
        });
    }
}
