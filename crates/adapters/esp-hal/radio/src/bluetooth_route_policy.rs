//! Platform-neutral validation for the three ESP32-S31 Bluetooth CPU routes.

#![forbid(unsafe_code)]

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalBluetoothInterruptRouteError {
    /// A required register owner is currently outside stable ISR storage.
    OwnersUnavailable,
    /// A complete Bluetooth route epoch is already live process-wide.
    AlreadyBound,
    /// No complete Bluetooth route epoch is currently live.
    Inactive,
    /// Disable was attempted away from the route epoch's binding core.
    WrongCore,
}

/// Why both Bluetooth ISR owners could not be published atomically.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalBluetoothInterruptStorageError {
    /// A preceding Controller epoch already reserved this storage.
    AlreadyPublished,
    /// Exactly one slot was occupied, so the process-wide invariant is broken.
    StorageInvariant,
}

/// Validate both process-wide slots before either owner is moved into storage.
pub(crate) const fn validate_interrupt_storage(
    interrupt_occupied: bool,
    timer_occupied: bool,
) -> Result<(), EspHalBluetoothInterruptStorageError> {
    match (interrupt_occupied, timer_occupied) {
        (false, false) => Ok(()),
        (true, true) => Err(EspHalBluetoothInterruptStorageError::AlreadyPublished),
        _ => Err(EspHalBluetoothInterruptStorageError::StorageInvariant),
    }
}

/// Slot emptiness after retirement is not permission to publish a new epoch.
/// Old static Controller borrows remain live until out-of-band board reset.
pub(crate) struct InterruptStorageReservation {
    claimed: bool,
}

impl InterruptStorageReservation {
    pub(crate) const fn new() -> Self {
        Self { claimed: false }
    }

    pub(crate) fn admit_restore(
        &self,
        routes_bound: bool,
        interrupt_occupied: bool,
        timer_occupied: bool,
    ) -> Result<(), EspHalBluetoothInterruptStorageError> {
        if !self.claimed || routes_bound {
            return Err(EspHalBluetoothInterruptStorageError::StorageInvariant);
        }
        validate_interrupt_storage(interrupt_occupied, timer_occupied)
    }

    pub(crate) fn claim(
        &mut self,
        interrupt_occupied: bool,
        timer_occupied: bool,
    ) -> Result<(), EspHalBluetoothInterruptStorageError> {
        if self.claimed {
            return Err(EspHalBluetoothInterruptStorageError::AlreadyPublished);
        }
        validate_interrupt_storage(interrupt_occupied, timer_occupied)?;
        self.claimed = true;
        Ok(())
    }
}

/// Why the shared primary/NRT register owner cannot leave ISR storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalBluetoothInterruptRetirementError {
    /// A complete or quarantined route epoch still retains its dispatch.
    RoutesBound,
    /// The timer must leave stable storage before the shared register owner.
    TimerStillPublished,
    /// The primary/NRT owner is absent or has already been extracted.
    Missing,
}

/// The caller holds the same serialization boundary as route binding and ISR service.
pub(crate) fn retire_interrupt_owner<Owner>(
    slot: &mut Option<Owner>,
    routes_bound: bool,
    timer_published: bool,
) -> Result<Owner, EspHalBluetoothInterruptRetirementError> {
    use EspHalBluetoothInterruptRetirementError as Error;
    if routes_bound {
        return Err(Error::RoutesBound);
    }
    if timer_published {
        return Err(Error::TimerStillPublished);
    }
    slot.take().ok_or(Error::Missing)
}

/// Why an ISR-ready timer cannot leave stable storage for retirement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalBluetoothModemLpTimerRetirementError {
    /// The complete route epoch has not been disabled, including quarantine.
    RoutesBound,
    /// Task work already owns the timer, or the timer has already been removed.
    Missing,
    /// The stable timer still requires its software handler.
    SoftwarePending,
}

pub(crate) enum StoredModemTimerOwner<Ready, Pending> {
    Ready(Ready),
    SoftwarePending(Pending),
}

impl<Ready, Pending> StoredModemTimerOwner<Ready, Pending> {
    #[cfg(target_arch = "riscv32")]
    pub(crate) const fn phase(&self) -> BluetoothModemLpTimerStoragePhase {
        match self {
            Self::Ready(_) => BluetoothModemLpTimerStoragePhase::Ready,
            Self::SoftwarePending(_) => BluetoothModemLpTimerStoragePhase::SoftwarePending,
        }
    }
}

/// The caller serializes this transition with route binding and ISR service.
pub(crate) fn retire_ready_timer<Ready, Pending>(
    slot: &mut Option<StoredModemTimerOwner<Ready, Pending>>,
    routes_bound: bool,
) -> Result<Ready, EspHalBluetoothModemLpTimerRetirementError> {
    use EspHalBluetoothModemLpTimerRetirementError as Error;
    if routes_bound {
        return Err(Error::RoutesBound);
    }
    match slot.as_ref() {
        None => Err(Error::Missing),
        Some(StoredModemTimerOwner::SoftwarePending(_)) => Err(Error::SoftwarePending),
        Some(StoredModemTimerOwner::Ready(_)) => {
            let Some(StoredModemTimerOwner::Ready(owner)) = slot.take() else {
                unreachable!("the serialized ready owner cannot change phase")
            };
            Ok(owner)
        }
    }
}

pub(crate) fn validate_route_owners(
    interrupts_present: bool,
    timer_present: bool,
) -> Result<(), EspHalBluetoothInterruptRouteError> {
    if interrupts_present && timer_present {
        Ok(())
    } else {
        Err(EspHalBluetoothInterruptRouteError::OwnersUnavailable)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothModemLpTimerStoragePhase {
    Missing,
    Ready,
    SoftwarePending,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothModemLpTimerInterruptAdmission {
    Unavailable,
    ServiceRegisters,
    AwaitSoftware,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum BluetoothModemLpTimerTaskTakeAdmission {
    Missing,
    NotSoftwarePending,
    Acquire,
}

pub(crate) const fn classify_modem_lp_timer_interrupt(
    phase: BluetoothModemLpTimerStoragePhase,
) -> BluetoothModemLpTimerInterruptAdmission {
    match phase {
        BluetoothModemLpTimerStoragePhase::Missing => {
            BluetoothModemLpTimerInterruptAdmission::Unavailable
        }
        BluetoothModemLpTimerStoragePhase::Ready => {
            BluetoothModemLpTimerInterruptAdmission::ServiceRegisters
        }
        BluetoothModemLpTimerStoragePhase::SoftwarePending => {
            BluetoothModemLpTimerInterruptAdmission::AwaitSoftware
        }
    }
}

pub(crate) const fn classify_modem_lp_timer_task_take(
    phase: BluetoothModemLpTimerStoragePhase,
) -> BluetoothModemLpTimerTaskTakeAdmission {
    match phase {
        BluetoothModemLpTimerStoragePhase::Missing => {
            BluetoothModemLpTimerTaskTakeAdmission::Missing
        }
        BluetoothModemLpTimerStoragePhase::Ready => {
            BluetoothModemLpTimerTaskTakeAdmission::NotSoftwarePending
        }
        BluetoothModemLpTimerStoragePhase::SoftwarePending => {
            BluetoothModemLpTimerTaskTakeAdmission::Acquire
        }
    }
}

pub(crate) const fn ready_owner_restore_is_admitted(slot_occupied: bool) -> bool {
    !slot_occupied
}

pub(crate) fn service_stable_owner<Owner, Output>(
    slot: &mut Option<Owner>,
    service: impl FnOnce(&mut Owner) -> Output,
) -> Option<Output> {
    slot.as_mut().map(service)
}

/// Host-testable ownership model for one process-wide interrupt-route epoch.
///
/// The target adapter stores the dispatch function beside the same binding
/// core. This model owns the transition policy without depending on ESP-HAL.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct BluetoothInterruptRouteState<Core> {
    bound_core: Option<Core>,
}

impl<Core: Copy + Eq> BluetoothInterruptRouteState<Core> {
    #[cfg(test)]
    pub(crate) const fn inactive() -> Self {
        Self { bound_core: None }
    }

    pub(crate) const fn from_bound_core(bound_core: Option<Core>) -> Self {
        Self { bound_core }
    }

    pub(crate) fn bind(&mut self, core: Core) -> Result<(), EspHalBluetoothInterruptRouteError> {
        if self.bound_core.is_some() {
            return Err(EspHalBluetoothInterruptRouteError::AlreadyBound);
        }
        self.bound_core = Some(core);
        Ok(())
    }

    #[cfg(test)]
    pub(crate) const fn dispatch_is_live(&self) -> bool {
        self.bound_core.is_some()
    }

    pub(crate) fn disable(
        &mut self,
        current_core: Core,
    ) -> Result<(), EspHalBluetoothInterruptRouteError> {
        let Some(bound_core) = self.bound_core else {
            return Err(EspHalBluetoothInterruptRouteError::Inactive);
        };
        if current_core != bound_core {
            return Err(EspHalBluetoothInterruptRouteError::WrongCore);
        }
        self.bound_core = None;
        Ok(())
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod restart_reservation_tests {
    use super::*;
    #[test]
    fn restoration_reuses_only_the_claimed_empty_unrouted_reservation() {
        let mut reservation = InterruptStorageReservation::new();
        assert!(reservation.admit_restore(false, false, false).is_err());
        reservation.claim(false, false).unwrap();
        for (routes, irq, timer) in [
            (true, false, false),
            (false, true, false),
            (false, false, true),
            (false, true, true),
        ] {
            assert!(reservation.admit_restore(routes, irq, timer).is_err());
        }
        reservation.admit_restore(false, false, false).unwrap();
        assert_eq!(
            reservation.claim(false, false),
            Err(EspHalBluetoothInterruptStorageError::AlreadyPublished)
        );
        reservation.admit_restore(false, false, false).unwrap();
    }
}
