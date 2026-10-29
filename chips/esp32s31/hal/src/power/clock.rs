//! S31 newtype binding of the hardware-free profile to the family clock planner.
#![forbid(unsafe_code)]
use oer_esp32s31_knowledge::modem_clock::Dependency as ChipDependency;
pub(crate) use oer_esp32s31_knowledge::modem_clock::ModemClockModule;
pub(crate) use oer_espressif_modem_clock::ModemClockPlannerIdentity;
use oer_espressif_modem_clock::{DependencySet, ModemClockDependency};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Dependency(ChipDependency);

impl ModemClockDependency for Dependency {
    const LOW_BIT_FIRST: &'static [Self] = &const {
        let mut wrapped =
            [Self(ChipDependency::ModemAdcCommonFe); ChipDependency::LOW_BIT_FIRST.len()];
        let mut source = ChipDependency::LOW_BIT_FIRST.as_slice();
        let mut destination = wrapped.as_mut_slice();
        while let (Some((dependency, remaining)), Some((slot, tail))) =
            (source.split_first(), destination.split_first_mut())
        {
            *slot = Self(*dependency);
            source = remaining;
            destination = tail;
        }
        wrapped
    };

    fn index(self) -> oer_espressif_modem_clock::DependencyIndex {
        use oer_espressif_modem_clock::DependencyIndex;
        match self.0 {
            ChipDependency::ModemAdcCommonFe => DependencyIndex::constant::<0>(),
            ChipDependency::ModemPrivateFe => DependencyIndex::constant::<1>(),
            ChipDependency::Pll160AndModemSource => DependencyIndex::constant::<2>(),
            ChipDependency::Coexistence => DependencyIndex::constant::<3>(),
            ChipDependency::AnalogI2cMaster => DependencyIndex::constant::<4>(),
            ChipDependency::WifiApb => DependencyIndex::constant::<5>(),
            ChipDependency::WifiBb44m => DependencyIndex::constant::<6>(),
            ChipDependency::WifiMac => DependencyIndex::constant::<7>(),
            ChipDependency::WifiBb => DependencyIndex::constant::<8>(),
            ChipDependency::WifiBb80x1 => DependencyIndex::constant::<9>(),
            ChipDependency::Etm => DependencyIndex::constant::<10>(),
            ChipDependency::BtMac => DependencyIndex::constant::<11>(),
            ChipDependency::BtPeripheral => DependencyIndex::constant::<12>(),
            ChipDependency::BtApbAndSecurity => DependencyIndex::constant::<13>(),
            ChipDependency::BtIeee802154CommonBaseband => DependencyIndex::constant::<14>(),
            ChipDependency::Ieee802154ApbAndMac => DependencyIndex::constant::<15>(),
        }
    }

    fn refcounted(self) -> bool {
        self.0.refcounted()
    }
    fn retained_while_wifi_initialized(self) -> bool {
        self.0.retained_while_wifi_initialized()
    }
}

pub(crate) type ModemClockPlanner = oer_espressif_modem_clock::ModemClockPlanner<Dependency>;
pub(crate) type ModemClockLease = oer_espressif_modem_clock::ModemClockLease<Dependency>;
#[cfg(test)]
pub(crate) type PreparedModemClockAcquire =
    oer_espressif_modem_clock::PreparedModemClockAcquire<Dependency>;
#[cfg(test)]
pub(crate) type PreparedModemClockRelease =
    oer_espressif_modem_clock::PreparedModemClockRelease<Dependency>;
pub(crate) type PoisonedModemClockAcquire =
    oer_espressif_modem_clock::PoisonedModemClockAcquire<Dependency>;
pub(crate) type PoisonedModemClockRelease =
    oer_espressif_modem_clock::PoisonedModemClockRelease<Dependency>;

pub(crate) fn module_dependencies(module: ModemClockModule) -> DependencySet<Dependency> {
    DependencySet::of_iter(module.dependencies().iter().copied().map(Dependency))
}

mod executor;
pub(crate) use executor::ModemExecution;
pub(crate) use executor::ModemPlatformClocks;
pub use executor::{
    PlatformClock, PlatformClockAbandonment, PlatformClockBackend, PlatformClockError,
    PlatformClockGuard, PlatformClockHolds, PlatformClockProvider, PlatformClockReleaseRefused,
};

#[cfg(test)]
mod tests;
