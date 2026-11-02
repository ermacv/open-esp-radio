//! Module lease orchestration over the existing modem edge executor.
use oer_espressif_modem_clock::{self as clock, ModemClockDependency as Dependency};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    AlreadyEnabled,
    NotEnabled,
    Rejected,
    Poisoned,
}

/// The exact prepared/poisoned owners remain inline and travel with custody.
#[allow(
    clippy::large_enum_variant,
    reason = "retain the planner or poisoned transaction without allocation"
)]
pub enum ModemClocks<D: Dependency, const N: usize> {
    Ready {
        planner: clock::ModemClockPlanner<D>,
        leases: [Option<clock::ModemClockLease<D>>; N],
    },
    AcquirePoisoned {
        owner: clock::PoisonedModemClockAcquire<D>,
    },
    ReleasePoisoned {
        owner: clock::PoisonedModemClockRelease<D>,
    },
    InFlight,
}
impl<D: Dependency, const N: usize> ModemClocks<D, N> {
    pub fn new(identity: clock::ModemClockPlannerIdentity) -> Self {
        Self::Ready {
            planner: clock::ModemClockPlanner::managed(identity),
            leases: [const { None }; N],
        }
    }
    pub fn any_held(&self) -> bool {
        match self {
            Self::Ready { leases, .. } => leases.iter().any(Option::is_some),
            Self::AcquirePoisoned { .. } | Self::ReleasePoisoned { .. } | Self::InFlight => true,
        }
    }
    pub fn set_wifi_initialized(&mut self, initialized: bool) -> Result<(), Error> {
        match self {
            Self::Ready { planner, .. } => {
                planner.set_wifi_initialized(initialized);
                Ok(())
            }
            _ => Err(Error::Poisoned),
        }
    }
    pub fn enable<P: clock::execution::AcquirePort<D>>(
        &mut self,
        index: usize,
        dependencies: clock::DependencySet<D>,
        port: &mut P,
    ) -> Result<(), Error> {
        let (planner, mut leases) = match core::mem::replace(self, Self::InFlight) {
            Self::Ready { planner, leases } => (planner, leases),
            other => {
                *self = other;
                return Err(Error::Poisoned);
            }
        };
        let Some(slot) = leases.get_mut(index) else {
            *self = Self::Ready { planner, leases };
            return Err(Error::Rejected);
        };
        if slot.is_some() {
            *self = Self::Ready { planner, leases };
            return Err(Error::AlreadyEnabled);
        }
        let prepared = match planner.prepare_acquire(dependencies) {
            Ok(prepared) => prepared,
            Err(failure) => {
                *self = Self::Ready {
                    planner: failure.into_planner(),
                    leases,
                };
                return Err(Error::Rejected);
            }
        };
        match clock::execution::acquire(prepared, port) {
            Ok((planner, lease)) => {
                *slot = Some(lease);
                *self = Self::Ready { planner, leases };
                Ok(())
            }
            Err(owner) => {
                *self = Self::AcquirePoisoned { owner };
                Err(Error::Poisoned)
            }
        }
    }
    pub fn disable<P: clock::execution::RegisterPort<D>>(
        &mut self,
        index: usize,
        port: &mut P,
    ) -> Result<(), Error> {
        let (planner, mut leases) = match core::mem::replace(self, Self::InFlight) {
            Self::Ready { planner, leases } => (planner, leases),
            other => {
                *self = other;
                return Err(Error::Poisoned);
            }
        };
        let Some(slot) = leases.get_mut(index) else {
            *self = Self::Ready { planner, leases };
            return Err(Error::Rejected);
        };
        let Some(lease) = slot.take() else {
            *self = Self::Ready { planner, leases };
            return Err(Error::NotEnabled);
        };
        let prepared = match planner.prepare_release(lease) {
            Ok(prepared) => prepared,
            Err(failure) => {
                let (planner, lease) = failure.into_owners();
                *slot = Some(lease);
                *self = Self::Ready { planner, leases };
                return Err(Error::Rejected);
            }
        };
        match clock::execution::release(prepared, port) {
            Ok(planner) => {
                *self = Self::Ready { planner, leases };
                Ok(())
            }
            Err(owner) => {
                *self = Self::ReleasePoisoned { owner };
                Err(Error::Poisoned)
            }
        }
    }
}
