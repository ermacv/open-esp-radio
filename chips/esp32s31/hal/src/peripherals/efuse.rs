//! The eFuse controller's factory identity: the read words of the factory
//! MAC, which the controller mirrors after reset.

pub use oer_espressif_efuse::{FactoryMac, Interface, MacAddress};
use oer_mmio::{Fixed, Placement};

use oer_esp32s31_pac::{efuse::Efuse, instances, ownership::EfusePeripherals};

#[cfg(test)]
mod tests;

/// The eFuse controller's read words of the factory MAC.
#[must_use = "dropping a register owner permanently loses its register authority"]
pub struct EfuseRegisters<P: Placement = Fixed<{ instances::efuse::BASE }>>(Efuse<P>);

impl EfuseRegisters {
    /// Own the root's eFuse partition.
    pub fn new(peripherals: EfusePeripherals) -> Self {
        Self(peripherals.efuse)
    }
}

/// The chip's factory identity, read once from eFuse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FactoryIdentity {
    mac: FactoryMac,
}

impl FactoryIdentity {
    /// Read the factory MAC fields of block 1, which the controller mirrors
    /// after reset; reading needs no command, and the eFuse owner is not
    /// needed again.
    pub fn read<P: Placement>(efuse: EfuseRegisters<P>) -> Self {
        let low = efuse.0.read_rd_mac_sys0();
        let high = efuse.0.read_rd_mac_sys1();
        Self {
            mac: FactoryMac {
                mac_0: low.mac_0(),
                mac_1: high.mac_1(),
                mac_ext: high.mac_ext(),
            },
        }
    }

    /// The factory (universally administered) MAC address.
    pub const fn base_mac(self) -> MacAddress {
        self.mac.base_mac()
    }

    /// The MAC extension bits, as the PHY calibration identity records them.
    pub const fn mac_extension(self) -> u16 {
        self.mac.mac_extension()
    }

    /// The address `interface` uses.
    pub const fn address(self, interface: Interface) -> MacAddress {
        interface.address(self.base_mac())
    }
}

#[cfg(any(test, feature = "test-support"))]
impl<'a> EfuseRegisters<oer_mmio::Host<'a>> {
    /// The eFuse words over host memory from the instance's base.
    pub fn host(words: &'a mut [u32]) -> Result<Self, oer_mmio::HostTooSmall> {
        Efuse::host(words).map(Self)
    }
}
