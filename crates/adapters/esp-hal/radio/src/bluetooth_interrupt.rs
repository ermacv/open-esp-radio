//! Typed ESP-HAL routing primitives for all three Bluetooth interrupts.
//!
//! Stable publication is deliberately separate from a live interrupt epoch.
//! Both unique HAL owners are installed atomically before any CPU route can be
//! enabled. A borrowing bind transition joins that stable publication to one
//! full-controller dispatcher. Three adapter-owned handlers supply their
//! fixed semantic roles to that dispatcher, so integration cannot exchange
//! primary, modem-timer and NRT callbacks. The resulting affine epoch is the
//! only value that may keep those routes live.

#![forbid(unsafe_code)]

use core::cell::RefCell;

use crate::bluetooth_route_policy::{
    BluetoothInterruptRouteState, BluetoothModemLpTimerInterruptAdmission,
    BluetoothModemLpTimerStoragePhase, BluetoothModemLpTimerTaskTakeAdmission,
    EspHalBluetoothInterruptRetirementError, EspHalBluetoothInterruptRouteError,
    EspHalBluetoothInterruptStorageError, InterruptStorageReservation,
    classify_modem_lp_timer_interrupt, classify_modem_lp_timer_task_take,
    ready_owner_restore_is_admitted, retire_interrupt_owner, service_stable_owner,
};

use critical_section::Mutex;
use oer_espressif_interrupt_table_esp_hal::{self as interrupt_table, Route};
use oer_interrupt_table::Entry;

use esp_hal::{interrupt::Priority, peripherals::Interrupt, system::Cpu};

use oer_esp32s31_bluetooth::{
    interrupt::{
        InterruptOwnerStorage, NrtDefaultInterruptEpoch, PrimaryInterruptStep,
        SharedInterruptDispatchStorage, step_nrt_default_interrupt, step_primary_interrupt,
    },
    modem_lp_timer_queue::ModemLpTimerStableInterruptStep,
    modem_timer::{
        ModemLpTimerInterruptDispatchStorage, ModemLpTimerRetirementStorage,
        ModemLpTimerSoftwareOwnerStorage,
    },
    scheduler::SchedulerRunInterruptStorage,
};

use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerRunInterruptsPrepared, InterruptRegistersOwner,
    ModemLpTimerHandlerRegisterStep, ModemLpTimerInterruptReadyOwner, ModemLpTimerInterruptStep,
    ModemLpTimerSoftwarePendingOwner,
};

pub(crate) const PRIMARY_INTERRUPT: Interrupt = Interrupt::MODEM_BT_MAC;
pub(crate) const MODEM_LP_TIMER_INTERRUPT: Interrupt = Interrupt::MODEM_LP_TIMER;
pub(crate) const NRT_INTERRUPT: Interrupt = Interrupt::MODEM_BT_MAC_INT1;

/// The routes of the Controller's three interrupt sources in the image's
/// interrupt table, which the adapter keeps for every epoch of the boot.
static ROUTES: Mutex<RefCell<Option<EspHalBluetoothInterruptRoutes>>> =
    Mutex::new(RefCell::new(None));

/// The routes of `MODEM_BT_MAC`, `MODEM_LP_TIMER` and `MODEM_BT_MAC_INT1`,
/// from their tokens in the image's interrupt table, whose entries name
/// `bluetooth_primary_interrupt_handler`,
/// `bluetooth_modem_lp_timer_interrupt_handler` and
/// `bluetooth_nrt_default_interrupt_handler`.
pub struct EspHalBluetoothInterruptRoutes {
    primary: Route,
    modem_lp_timer: Route,
    nrt: Route,
}

impl EspHalBluetoothInterruptRoutes {
    /// Another source's token does not compile.
    pub fn new<P, T, N>(primary: P, modem_lp_timer: T, nrt: N) -> Self
    where
        P: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
        T: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
        N: Entry<Source = Interrupt, Level = Priority, Core = Cpu>,
    {
        const {
            assert!(
                P::SOURCE as u16 == PRIMARY_INTERRUPT as u16,
                "the primary token is not MODEM_BT_MAC's"
            );
            assert!(
                T::SOURCE as u16 == MODEM_LP_TIMER_INTERRUPT as u16,
                "the timer token is not MODEM_LP_TIMER's"
            );
            assert!(
                N::SOURCE as u16 == NRT_INTERRUPT as u16,
                "the NRT token is not MODEM_BT_MAC_INT1's"
            );
        };
        Self {
            primary: Route::new(primary),
            modem_lp_timer: Route::new(modem_lp_timer),
            nrt: Route::new(nrt),
        }
    }

    fn route(&self, source: EspHalBluetoothInterruptSource) -> &Route {
        match source {
            EspHalBluetoothInterruptSource::Primary => &self.primary,
            EspHalBluetoothInterruptSource::ModemLpTimer => &self.modem_lp_timer,
            EspHalBluetoothInterruptSource::NrtDefault => &self.nrt,
        }
    }
}

static INTERRUPT_REGISTERS: Mutex<RefCell<Option<InterruptRegistersOwner>>> =
    Mutex::new(RefCell::new(None));
static MODEM_LP_TIMER: Mutex<RefCell<Option<StoredBluetoothModemLpTimerOwner>>> =
    Mutex::new(RefCell::new(None));
static BOUND_ROUTE_DISPATCH: Mutex<RefCell<Option<BoundRouteDispatch>>> =
    Mutex::new(RefCell::new(None));
static STORAGE_RESERVATION: Mutex<RefCell<InterruptStorageReservation>> =
    Mutex::new(RefCell::new(InterruptStorageReservation::new()));

/// Exact semantic role of the adapter-owned handler that entered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalBluetoothInterruptSource {
    /// Controller primary source 124 (`MODEM_BT_MAC`).
    Primary,
    /// Modem low-power timer source 127 (`MODEM_LP_TIMER`).
    ModemLpTimer,
    /// Controller default NRT source 133 (`MODEM_BT_MAC_INT1`).
    NrtDefault,
}

/// Required route disposition after one full Controller interrupt service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[must_use = "fatal service failure must quarantine its asserted CPU route"]
pub enum EspHalBluetoothInterruptDisposition {
    /// Full service completed and the route remains live.
    Serviced,
    /// A fatal storage invariant failed; disable this asserted route in place.
    Quarantine,
}

#[derive(Clone, Copy)]
struct BoundRouteDispatch {
    core: Cpu,
    dispatch: fn(EspHalBluetoothInterruptSource) -> EspHalBluetoothInterruptDisposition,
    live: bool,
}

pub(crate) fn dispatch_bound_source(source: EspHalBluetoothInterruptSource) {
    let bound = critical_section::with(|critical_section| {
        BOUND_ROUTE_DISPATCH
            .borrow_ref(critical_section)
            .as_ref()
            .filter(|route| route.live)
            .map(|route| (route.core, route.dispatch))
    });
    let Some((core, dispatch)) = bound else {
        return;
    };
    if dispatch(source) == EspHalBluetoothInterruptDisposition::Quarantine {
        critical_section::with(|critical_section| {
            let route = BOUND_ROUTE_DISPATCH.borrow_ref(critical_section);
            if route
                .as_ref()
                .is_some_and(|route| route.live && route.core == core)
            {
                let _ = core;
                if let Some(routes) = ROUTES.borrow_ref(critical_section).as_ref() {
                    interrupt_table::disable_route(routes.route(source));
                }
            }
        });
    }
}

type StoredBluetoothModemLpTimerOwner = crate::bluetooth_route_policy::StoredModemTimerOwner<
    ModemLpTimerInterruptReadyOwner,
    ModemLpTimerSoftwarePendingOwner,
>;

/// Stable process-wide slots for both Bluetooth ISR register owners.
///
/// Constructing this value performs no claim. Publication rejects any second
/// value after the first publication, including after both owners are retired.
#[derive(Clone, Copy, Debug, Default)]
pub struct EspHalBluetoothInterruptStorage;

impl EspHalBluetoothInterruptStorage {
    /// Construct an unclaimed reference to the process-wide storage boundary.
    pub const fn new() -> Self {
        Self
    }

    /// Keep the Controller's interrupt routes for every epoch of this boot.
    ///
    /// # Errors
    ///
    /// The boot installed its routes before; `routes` returns unchanged.
    pub fn install_routes(
        &self,
        routes: EspHalBluetoothInterruptRoutes,
    ) -> Result<(), EspHalBluetoothInterruptRoutes> {
        critical_section::with(|critical_section| {
            let mut slot = ROUTES.borrow_ref_mut(critical_section);
            if slot.is_some() {
                return Err(routes);
            }
            *slot = Some(routes);
            Ok(())
        })
    }
}

/// Affine proof of one reserved Bluetooth ISR storage epoch.
///
/// Dropping the lease leaves its owners retained. Explicit retirement may
/// remove the owners, but the reservation remains claimed because old static
/// Controller borrows cannot yet be reclaimed. Only board reset clears it.
#[must_use = "retain the ISR storage reservation through Controller teardown"]
pub struct PublishedEspHalBluetoothInterruptOwners {
    _private: (),
}

/// Actual shared register partition after removal from inactive ISR storage.
///
/// The Controller composition joins this owner to its retired task for
/// checked output release; no raw register access or route reactivation is
/// exposed.
#[must_use = "retain the recovered interrupt registers through physical shutdown"]
pub struct RetiredEspHalBluetoothInterruptRegisters {
    _registers: oer_esp32s31_hal::bluetooth::InterruptOutputAfterRoutesOwner,
}

impl RetiredEspHalBluetoothInterruptRegisters {
    /// Hand the recovered register partition to the Controller composition
    /// that retains the matching retired task and drained timer.
    pub fn into_output_owner(self) -> oer_esp32s31_hal::bluetooth::InterruptOutputAfterRoutesOwner {
        self._registers
    }
}

impl PublishedEspHalBluetoothInterruptOwners {
    /// Take the actual primary/NRT register partition after route removal.
    ///
    /// The timer slot must already be empty. This excludes ISR service under
    /// the same critical section as binding and extraction; it does not prove
    /// task, timer-worker or dynamic-source quiescence. The Controller shutdown
    /// composition must retain its retired task/HCI and drained timer owners.
    /// No register is accessed and the process-wide reservation stays claimed.
    pub fn retire_interrupt_registers_after_routes_disabled(
        &self,
    ) -> Result<RetiredEspHalBluetoothInterruptRegisters, EspHalBluetoothInterruptRetirementError>
    {
        critical_section::with(|cs| {
            retire_interrupt_owner(
                &mut INTERRUPT_REGISTERS.borrow_ref_mut(cs),
                BOUND_ROUTE_DISPATCH.borrow_ref(cs).is_some(),
                MODEM_LP_TIMER.borrow_ref(cs).is_some(),
            )
            .map(|registers| RetiredEspHalBluetoothInterruptRegisters {
                _registers: registers.deactivate(),
            })
        })
    }
}

/// Result of one finite source-127 register-only hard-handler entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalBluetoothModemLpTimerInterruptStep {
    /// No timer owner is currently available in the stable slot.
    Unavailable,
    /// Software work already owns the timer; no register was touched.
    AwaitingSoftware,
    /// The first status observation was empty and restored the ready owner.
    Spurious,
    /// The common register phase completed without software work.
    Rearmed,
    /// The acknowledged register phase transferred ownership to task work.
    SoftwarePending,
}

/// Result of one finite primary source-124 register/classifier entry.
#[must_use = "a serviced primary epoch must reach fault or scheduler policy"]
pub enum EspHalBluetoothPrimaryInterruptStep {
    /// The shared primary/NRT owner is absent from stable storage.
    Unavailable,
    /// The Controller completed one bounded acknowledged primary disposition.
    Serviced(PrimaryInterruptStep),
}

/// Result of one finite default-profile NRT source-133 entry.
#[must_use = "retain the acknowledged NRT epoch"]
pub enum EspHalBluetoothNrtInterruptStep {
    /// The shared primary/NRT owner is absent from stable storage.
    Unavailable,
    /// The default Controller profile acknowledged one opaque NRT epoch.
    Serviced(NrtDefaultInterruptEpoch),
}

/// Why the shared primary/NRT owner could not service an interrupt entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalBluetoothSharedInterruptDispatchError {
    /// The shared owner is absent from process-wide stable storage.
    Unavailable,
}

/// Why scheduler-start interrupt preparation could not borrow stable storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalBluetoothSchedulerRunInterruptError {
    /// The shared owner is absent from process-wide stable storage.
    Unavailable,
}

/// Why task-side source-127 ownership could not enter or leave stable storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EspHalBluetoothModemLpTimerStorageError {
    /// No timer owner is present in the process-wide slot.
    Missing,
    /// The stable owner is ready for an IRQ and has no pending software work.
    NotSoftwarePending,
    /// Task work attempted to restore an owner into an occupied slot.
    Occupied,
}

/// Failed task-side rearm retaining the unique ready owner.
#[must_use = "a failed source-127 rearm still owns the timer registers"]
pub struct EspHalBluetoothModemLpTimerRestoreFailure {
    /// Exact stable-storage rejection.
    pub error: EspHalBluetoothModemLpTimerStorageError,
    /// Unchanged ISR-ready owner.
    pub owner: ModemLpTimerInterruptReadyOwner,
}

impl PublishedEspHalBluetoothInterruptOwners {
    /// Prepare the exact dynamic interrupt groups required before scheduler
    /// event publication while retaining the shared owner in stable storage.
    pub fn prepare_scheduler_run_interrupts(
        &self,
    ) -> Result<BluetoothSchedulerRunInterruptsPrepared, EspHalBluetoothSchedulerRunInterruptError>
    {
        critical_section::with(|critical_section| {
            let mut slot = INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
            service_stable_owner(&mut slot, |owner| owner.prepare_scheduler_run_interrupts())
                .ok_or(EspHalBluetoothSchedulerRunInterruptError::Unavailable)
        })
    }

    /// Capture, acknowledge and classify one primary source-124 epoch.
    ///
    /// The unique shared register owner remains in its process-wide slot for
    /// later NRT and primary entries. This method publishes no executor wake
    /// and does not make either CPU route live.
    pub fn service_primary_interrupt(&self) -> EspHalBluetoothPrimaryInterruptStep {
        service_bound_bluetooth_primary_interrupt()
    }

    /// Capture and acknowledge one source-133 epoch for the default profile.
    ///
    /// The same shared owner stays published. No synthetic Link-Layer or
    /// executor work is produced by the reviewed default NRT policy.
    pub fn service_nrt_default_interrupt(&self) -> EspHalBluetoothNrtInterruptStep {
        service_bound_bluetooth_nrt_default_interrupt()
    }

    /// Execute at most the source-127 classifier and common register phase.
    ///
    /// A software-pending result leaves the unique owner in stable storage for
    /// task context. Re-entry before task acquisition performs no MMIO.
    pub fn service_modem_lp_timer_interrupt(&self) -> EspHalBluetoothModemLpTimerInterruptStep {
        service_bound_bluetooth_modem_lp_timer_interrupt()
    }

    /// Move source-127 software-pending ownership into task context.
    pub fn take_modem_lp_timer_software_pending(
        &self,
    ) -> Result<ModemLpTimerSoftwarePendingOwner, EspHalBluetoothModemLpTimerStorageError> {
        critical_section::with(|critical_section| {
            let mut slot = MODEM_LP_TIMER.borrow_ref_mut(critical_section);
            let phase = slot
                .as_ref()
                .map_or(BluetoothModemLpTimerStoragePhase::Missing, |owner| {
                    owner.phase()
                });
            match classify_modem_lp_timer_task_take(phase) {
                BluetoothModemLpTimerTaskTakeAdmission::Missing => {
                    Err(EspHalBluetoothModemLpTimerStorageError::Missing)
                }
                BluetoothModemLpTimerTaskTakeAdmission::NotSoftwarePending => {
                    Err(EspHalBluetoothModemLpTimerStorageError::NotSoftwarePending)
                }
                BluetoothModemLpTimerTaskTakeAdmission::Acquire => {
                    let Some(StoredBluetoothModemLpTimerOwner::SoftwarePending(owner)) =
                        slot.take()
                    else {
                        return Err(EspHalBluetoothModemLpTimerStorageError::Missing);
                    };
                    Ok(owner)
                }
            }
        })
    }

    /// Return a fully rearmed source-127 owner to stable ISR storage.
    pub fn restore_modem_lp_timer_ready(
        &self,
        owner: ModemLpTimerInterruptReadyOwner,
    ) -> Result<(), EspHalBluetoothModemLpTimerRestoreFailure> {
        critical_section::with(|critical_section| {
            let mut slot = MODEM_LP_TIMER.borrow_ref_mut(critical_section);
            if !ready_owner_restore_is_admitted(slot.is_some()) {
                return Err(EspHalBluetoothModemLpTimerRestoreFailure {
                    error: EspHalBluetoothModemLpTimerStorageError::Occupied,
                    owner,
                });
            }
            *slot = Some(StoredBluetoothModemLpTimerOwner::Ready(owner));
            Ok(())
        })
    }
}

/// Service one bounded source-124 entry only while a complete route epoch is
/// live.
///
/// This is the narrow entrypoint intended for the installed primary handler.
/// Calling it before the consuming route bind or after successful disable is
/// fail-closed and performs no register access.
fn service_bound_bluetooth_primary_interrupt() -> EspHalBluetoothPrimaryInterruptStep {
    critical_section::with(|critical_section| {
        if BOUND_ROUTE_DISPATCH
            .borrow_ref(critical_section)
            .as_ref()
            .is_none_or(|route| !route.live)
        {
            return EspHalBluetoothPrimaryInterruptStep::Unavailable;
        }
        let mut slot = INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
        service_stable_owner(&mut slot, step_primary_interrupt).map_or(
            EspHalBluetoothPrimaryInterruptStep::Unavailable,
            EspHalBluetoothPrimaryInterruptStep::Serviced,
        )
    })
}

/// Service one bounded source-133 entry only while a complete route epoch is
/// live.
///
/// This is the narrow entrypoint intended for the installed NRT handler. It
/// acknowledges only the reviewed default disposition and publishes no
/// synthetic controller work.
fn service_bound_bluetooth_nrt_default_interrupt() -> EspHalBluetoothNrtInterruptStep {
    critical_section::with(|critical_section| {
        if BOUND_ROUTE_DISPATCH
            .borrow_ref(critical_section)
            .as_ref()
            .is_none_or(|route| !route.live)
        {
            return EspHalBluetoothNrtInterruptStep::Unavailable;
        }
        let mut slot = INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
        service_stable_owner(&mut slot, step_nrt_default_interrupt).map_or(
            EspHalBluetoothNrtInterruptStep::Unavailable,
            EspHalBluetoothNrtInterruptStep::Serviced,
        )
    })
}

/// Service one bounded source-127 register step only while the complete route
/// epoch is live.
///
/// The function performs one finite classifier/register disposition. Pending
/// task ownership remains in stable storage, and executor notification stays
/// the responsibility of the Controller interrupt service.
fn service_bound_bluetooth_modem_lp_timer_interrupt() -> EspHalBluetoothModemLpTimerInterruptStep {
    critical_section::with(|critical_section| {
        if BOUND_ROUTE_DISPATCH
            .borrow_ref(critical_section)
            .as_ref()
            .is_none_or(|route| !route.live)
        {
            return EspHalBluetoothModemLpTimerInterruptStep::Unavailable;
        }
        let mut slot = MODEM_LP_TIMER.borrow_ref_mut(critical_section);
        let phase = slot
            .as_ref()
            .map_or(BluetoothModemLpTimerStoragePhase::Missing, |owner| {
                owner.phase()
            });
        match classify_modem_lp_timer_interrupt(phase) {
            BluetoothModemLpTimerInterruptAdmission::Unavailable => {
                EspHalBluetoothModemLpTimerInterruptStep::Unavailable
            }
            BluetoothModemLpTimerInterruptAdmission::AwaitSoftware => {
                EspHalBluetoothModemLpTimerInterruptStep::AwaitingSoftware
            }
            BluetoothModemLpTimerInterruptAdmission::ServiceRegisters => {
                let Some(StoredBluetoothModemLpTimerOwner::Ready(ready)) = slot.take() else {
                    return EspHalBluetoothModemLpTimerInterruptStep::Unavailable;
                };
                match ready.step() {
                    ModemLpTimerInterruptStep::Spurious(ready) => {
                        *slot = Some(StoredBluetoothModemLpTimerOwner::Ready(ready));
                        EspHalBluetoothModemLpTimerInterruptStep::Spurious
                    }
                    ModemLpTimerInterruptStep::HandlerPending(pending) => {
                        match pending.step_registers() {
                            ModemLpTimerHandlerRegisterStep::Rearmed(ready) => {
                                *slot = Some(StoredBluetoothModemLpTimerOwner::Ready(ready));
                                EspHalBluetoothModemLpTimerInterruptStep::Rearmed
                            }
                            ModemLpTimerHandlerRegisterStep::SoftwarePending(pending) => {
                                *slot = Some(StoredBluetoothModemLpTimerOwner::SoftwarePending(
                                    pending,
                                ));
                                EspHalBluetoothModemLpTimerInterruptStep::SoftwarePending
                            }
                        }
                    }
                }
            }
        }
    })
}

impl SchedulerRunInterruptStorage for PublishedEspHalBluetoothInterruptOwners {
    type Error = EspHalBluetoothSchedulerRunInterruptError;

    fn prepare_scheduler_run_interrupts(
        &self,
    ) -> Result<BluetoothSchedulerRunInterruptsPrepared, Self::Error> {
        PublishedEspHalBluetoothInterruptOwners::prepare_scheduler_run_interrupts(self)
    }

    fn with_interrupt_registers<T, R>(
        &self,
        value: T,
        operation: impl FnOnce(&mut InterruptRegistersOwner, T) -> R,
    ) -> Result<R, T> {
        critical_section::with(|cs| {
            let mut slot = INTERRUPT_REGISTERS.borrow_ref_mut(cs);
            match slot.as_mut() {
                Some(interrupts) => Ok(operation(interrupts, value)),
                None => Err(value),
            }
        })
    }
}

impl InterruptOwnerStorage for EspHalBluetoothInterruptStorage {
    type Published = PublishedEspHalBluetoothInterruptOwners;
    type Error = EspHalBluetoothInterruptStorageError;

    fn publish(
        self,
        interrupts: InterruptRegistersOwner,
        timer: ModemLpTimerInterruptReadyOwner,
    ) -> Result<
        Self::Published,
        (
            Self::Error,
            Self,
            InterruptRegistersOwner,
            ModemLpTimerInterruptReadyOwner,
        ),
    > {
        critical_section::with(|critical_section| {
            let mut interrupt_slot = INTERRUPT_REGISTERS.borrow_ref_mut(critical_section);
            let mut timer_slot = MODEM_LP_TIMER.borrow_ref_mut(critical_section);
            match STORAGE_RESERVATION
                .borrow_ref_mut(critical_section)
                .claim(interrupt_slot.is_some(), timer_slot.is_some())
            {
                Ok(()) => {
                    *interrupt_slot = Some(interrupts);
                    *timer_slot = Some(StoredBluetoothModemLpTimerOwner::Ready(timer));
                    Ok(PublishedEspHalBluetoothInterruptOwners { _private: () })
                }
                Err(error) => Err((error, self, interrupts, timer)),
            }
        })
    }
}

impl ModemLpTimerRetirementStorage for PublishedEspHalBluetoothInterruptOwners {
    type RetireError = crate::bluetooth_route_policy::EspHalBluetoothModemLpTimerRetirementError;

    fn take_modem_lp_timer_ready_after_routes_disabled(
        &self,
    ) -> Result<ModemLpTimerInterruptReadyOwner, Self::RetireError> {
        critical_section::with(|cs| {
            // A retained (even quarantined) dispatch prevents extraction. Full
            // same-core disable removes it only after every route is inactive.
            let routes_bound = BOUND_ROUTE_DISPATCH.borrow_ref(cs).is_some();
            crate::bluetooth_route_policy::retire_ready_timer(
                &mut MODEM_LP_TIMER.borrow_ref_mut(cs),
                routes_bound,
            )
        })
    }
}

impl ModemLpTimerSoftwareOwnerStorage for PublishedEspHalBluetoothInterruptOwners {
    type TakeError = EspHalBluetoothModemLpTimerStorageError;
    type RestoreError = EspHalBluetoothModemLpTimerStorageError;

    fn take_modem_lp_timer_software_pending(
        &self,
    ) -> Result<ModemLpTimerSoftwarePendingOwner, Self::TakeError> {
        PublishedEspHalBluetoothInterruptOwners::take_modem_lp_timer_software_pending(self)
    }

    fn restore_modem_lp_timer_ready(
        &self,
        owner: ModemLpTimerInterruptReadyOwner,
    ) -> Result<(), (Self::RestoreError, ModemLpTimerInterruptReadyOwner)> {
        PublishedEspHalBluetoothInterruptOwners::restore_modem_lp_timer_ready(self, owner)
            .map_err(|failure| (failure.error, failure.owner))
    }
}

impl ModemLpTimerInterruptDispatchStorage for PublishedEspHalBluetoothInterruptOwners {
    type Error = EspHalBluetoothModemLpTimerStorageError;

    fn service_modem_lp_timer_interrupt(
        &self,
    ) -> Result<ModemLpTimerStableInterruptStep, Self::Error> {
        match PublishedEspHalBluetoothInterruptOwners::service_modem_lp_timer_interrupt(self) {
            EspHalBluetoothModemLpTimerInterruptStep::Unavailable => {
                Err(EspHalBluetoothModemLpTimerStorageError::Missing)
            }
            EspHalBluetoothModemLpTimerInterruptStep::AwaitingSoftware => {
                Ok(ModemLpTimerStableInterruptStep::AwaitingSoftware)
            }
            EspHalBluetoothModemLpTimerInterruptStep::Spurious => {
                Ok(ModemLpTimerStableInterruptStep::Spurious)
            }
            EspHalBluetoothModemLpTimerInterruptStep::Rearmed => {
                Ok(ModemLpTimerStableInterruptStep::Rearmed)
            }
            EspHalBluetoothModemLpTimerInterruptStep::SoftwarePending => {
                Ok(ModemLpTimerStableInterruptStep::SoftwarePending)
            }
        }
    }
}

impl SharedInterruptDispatchStorage for PublishedEspHalBluetoothInterruptOwners {
    type Error = EspHalBluetoothSharedInterruptDispatchError;

    fn service_primary_interrupt(&self) -> Result<PrimaryInterruptStep, Self::Error> {
        match PublishedEspHalBluetoothInterruptOwners::service_primary_interrupt(self) {
            EspHalBluetoothPrimaryInterruptStep::Unavailable => {
                Err(EspHalBluetoothSharedInterruptDispatchError::Unavailable)
            }
            EspHalBluetoothPrimaryInterruptStep::Serviced(step) => Ok(step),
        }
    }

    fn service_nrt_default_interrupt(&self) -> Result<NrtDefaultInterruptEpoch, Self::Error> {
        match PublishedEspHalBluetoothInterruptOwners::service_nrt_default_interrupt(self) {
            EspHalBluetoothNrtInterruptStep::Unavailable => {
                Err(EspHalBluetoothSharedInterruptDispatchError::Unavailable)
            }
            EspHalBluetoothNrtInterruptStep::Serviced(epoch) => Ok(epoch),
        }
    }
}

/// Complete live ESP-HAL Bluetooth interrupt epoch.
///
/// This value joins the stable register-owner publication to exactly one
/// primary, modem-LP timer and NRT CPU route on one core. It exposes no
/// individual route owner and must remain retained for the complete handler
/// lifetime. Dropping it is intentionally fail-stop: routes and stable owners
/// remain installed until board reset rather than being silently reminted.
#[must_use = "the complete Bluetooth interrupt epoch owns all three live CPU routes"]
pub struct BoundEspHalBluetoothInterruptEpoch<'published> {
    published: &'published PublishedEspHalBluetoothInterruptOwners,
    core: Cpu,
}

/// Failed complete-route disable retaining the still-live epoch.
#[must_use = "a rejected Bluetooth route disable retains all live route ownership"]
pub struct EspHalBluetoothInterruptRouteDisableFailure<'published> {
    error: EspHalBluetoothInterruptRouteError,
    epoch: BoundEspHalBluetoothInterruptEpoch<'published>,
}

impl<'published> EspHalBluetoothInterruptRouteDisableFailure<'published> {
    /// Inspect the exact route-set rejection.
    pub const fn error(&self) -> EspHalBluetoothInterruptRouteError {
        self.error
    }

    /// Recover the error and unchanged live route epoch.
    pub fn into_parts(
        self,
    ) -> (
        EspHalBluetoothInterruptRouteError,
        BoundEspHalBluetoothInterruptEpoch<'published>,
    ) {
        (self.error, self.epoch)
    }
}

impl PublishedEspHalBluetoothInterruptOwners {
    /// Bind the complete primary/modem-timer/NRT set to one dispatcher.
    ///
    /// The adapter owns all three exact ESP-HAL handlers and maps them to their
    /// non-interchangeable [`EspHalBluetoothInterruptSource`] values. The
    /// caller therefore supplies one full-controller dispatcher, not three
    /// swappable raw handlers.
    /// A successful full service returns `Serviced`; a fatal storage failure
    /// returns `Quarantine`, which disables only the asserted route before the
    /// adapter-owned handler exits.
    ///
    /// The complete chip/Embassy dispatcher state referenced by `dispatch`
    /// must already be in stable storage before this call. The callback and
    /// live marker are published before the first CPU route is enabled, so an
    /// interrupt observed immediately after binding always sees the complete
    /// dispatcher. A rejected bind leaves this borrowed publication unchanged.
    /// Both register owners must be in stable storage; a timer held by task
    /// work or retirement must be restored before reactivation.
    pub fn bind_routes(
        &self,
        dispatch: fn(EspHalBluetoothInterruptSource) -> EspHalBluetoothInterruptDisposition,
    ) -> Result<BoundEspHalBluetoothInterruptEpoch<'_>, EspHalBluetoothInterruptRouteError> {
        let core = Cpu::current();
        critical_section::with(|critical_section| {
            let mut live_route = BOUND_ROUTE_DISPATCH.borrow_ref_mut(critical_section);
            let mut state = BluetoothInterruptRouteState::from_bound_core(
                live_route.as_ref().map(|route| route.core),
            );
            state.bind(core)?;
            crate::bluetooth_route_policy::validate_route_owners(
                INTERRUPT_REGISTERS.borrow_ref(critical_section).is_some(),
                MODEM_LP_TIMER.borrow_ref(critical_section).is_some(),
            )?;
            let routes = ROUTES.borrow_ref(critical_section);
            let routes = routes
                .as_ref()
                .expect("the Controller's interrupt routes are installed before its first bind");
            *live_route = Some(BoundRouteDispatch {
                core,
                dispatch,
                live: true,
            });
            let enabled = [&routes.primary, &routes.modem_lp_timer, &routes.nrt]
                .into_iter()
                .try_for_each(interrupt_table::enable_route);
            if enabled.is_err() {
                for route in [&routes.modem_lp_timer, &routes.nrt, &routes.primary] {
                    interrupt_table::disable_route(route);
                }
                *live_route = None;
                return Err(EspHalBluetoothInterruptRouteError::WrongCore);
            }
            Ok::<(), EspHalBluetoothInterruptRouteError>(())
        })?;
        Ok(BoundEspHalBluetoothInterruptEpoch {
            published: self,
            core,
        })
    }
}

impl<'published> BoundEspHalBluetoothInterruptEpoch<'published> {
    /// Prepare scheduler-run interrupt groups through this live epoch's stable
    /// shared owner.
    pub fn prepare_scheduler_run_interrupts(
        &self,
    ) -> Result<BluetoothSchedulerRunInterruptsPrepared, EspHalBluetoothSchedulerRunInterruptError>
    {
        self.published.prepare_scheduler_run_interrupts()
    }

    /// Move source-127 software-pending ownership into task context.
    pub fn take_modem_lp_timer_software_pending(
        &self,
    ) -> Result<ModemLpTimerSoftwarePendingOwner, EspHalBluetoothModemLpTimerStorageError> {
        self.published.take_modem_lp_timer_software_pending()
    }

    /// Return a fully rearmed source-127 owner to this epoch's stable slot.
    pub fn restore_modem_lp_timer_ready(
        &self,
        owner: ModemLpTimerInterruptReadyOwner,
    ) -> Result<(), EspHalBluetoothModemLpTimerRestoreFailure> {
        self.published.restore_modem_lp_timer_ready(owner)
    }

    /// Disable all three routes on their binding core and end this borrow.
    ///
    /// The timer is closed first so no deadline callback can enter while the
    /// MAC routes are being quiesced. NRT follows because its opaque
    /// acknowledgement path has no controller-side baseline mask. A wrong-core
    /// or inactive-marker rejection returns the complete live epoch unchanged.
    pub fn disable(self) -> Result<(), EspHalBluetoothInterruptRouteDisableFailure<'published>> {
        let current_core = Cpu::current();
        let disabled = critical_section::with(|critical_section| {
            let mut live_route = BOUND_ROUTE_DISPATCH.borrow_ref_mut(critical_section);
            let mut state = BluetoothInterruptRouteState::from_bound_core(
                live_route.as_ref().map(|route| route.core),
            );
            if current_core != self.core {
                return Err(EspHalBluetoothInterruptRouteError::WrongCore);
            }
            state.disable(current_core)?;
            live_route
                .as_mut()
                .expect("a live route state retains its dispatcher")
                .live = false;
            let routes = ROUTES.borrow_ref(critical_section);
            let routes = routes
                .as_ref()
                .expect("a bound epoch's routes stay installed");
            interrupt_table::disable_route(&routes.modem_lp_timer);
            interrupt_table::disable_route(&routes.nrt);
            interrupt_table::disable_route(&routes.primary);
            *live_route = None;
            Ok::<(), EspHalBluetoothInterruptRouteError>(())
        });
        match disabled {
            Ok(()) => Ok(()),
            Err(error) => Err(EspHalBluetoothInterruptRouteDisableFailure { error, epoch: self }),
        }
    }
}

impl SchedulerRunInterruptStorage for BoundEspHalBluetoothInterruptEpoch<'_> {
    type Error = EspHalBluetoothSchedulerRunInterruptError;

    fn prepare_scheduler_run_interrupts(
        &self,
    ) -> Result<BluetoothSchedulerRunInterruptsPrepared, Self::Error> {
        BoundEspHalBluetoothInterruptEpoch::prepare_scheduler_run_interrupts(self)
    }

    fn with_interrupt_registers<T, R>(
        &self,
        value: T,
        operation: impl FnOnce(&mut InterruptRegistersOwner, T) -> R,
    ) -> Result<R, T> {
        self.published.with_interrupt_registers(value, operation)
    }
}

impl ModemLpTimerSoftwareOwnerStorage for BoundEspHalBluetoothInterruptEpoch<'_> {
    type TakeError = EspHalBluetoothModemLpTimerStorageError;
    type RestoreError = EspHalBluetoothModemLpTimerStorageError;

    fn take_modem_lp_timer_software_pending(
        &self,
    ) -> Result<ModemLpTimerSoftwarePendingOwner, Self::TakeError> {
        BoundEspHalBluetoothInterruptEpoch::take_modem_lp_timer_software_pending(self)
    }

    fn restore_modem_lp_timer_ready(
        &self,
        owner: ModemLpTimerInterruptReadyOwner,
    ) -> Result<(), (Self::RestoreError, ModemLpTimerInterruptReadyOwner)> {
        BoundEspHalBluetoothInterruptEpoch::restore_modem_lp_timer_ready(self, owner)
            .map_err(|failure| (failure.error, failure.owner))
    }
}

impl ModemLpTimerInterruptDispatchStorage for BoundEspHalBluetoothInterruptEpoch<'_> {
    type Error = EspHalBluetoothModemLpTimerStorageError;

    fn service_modem_lp_timer_interrupt(
        &self,
    ) -> Result<ModemLpTimerStableInterruptStep, Self::Error> {
        ModemLpTimerInterruptDispatchStorage::service_modem_lp_timer_interrupt(self.published)
    }
}

impl SharedInterruptDispatchStorage for BoundEspHalBluetoothInterruptEpoch<'_> {
    type Error = EspHalBluetoothSharedInterruptDispatchError;

    fn service_primary_interrupt(&self) -> Result<PrimaryInterruptStep, Self::Error> {
        SharedInterruptDispatchStorage::service_primary_interrupt(self.published)
    }

    fn service_nrt_default_interrupt(&self) -> Result<NrtDefaultInterruptEpoch, Self::Error> {
        SharedInterruptDispatchStorage::service_nrt_default_interrupt(self.published)
    }
}

impl oer_esp32s31_bluetooth::interrupt::InterruptOwnerRestartStorage
    for PublishedEspHalBluetoothInterruptOwners
{
    type RestartError = EspHalBluetoothInterruptStorageError;
    fn restore_initialized_interrupt_owners(
        &self,
        interrupts: InterruptRegistersOwner,
        timer: ModemLpTimerInterruptReadyOwner,
    ) -> Result<
        (),
        (
            Self::RestartError,
            InterruptRegistersOwner,
            ModemLpTimerInterruptReadyOwner,
        ),
    > {
        critical_section::with(|cs| {
            let mut irq_slot = INTERRUPT_REGISTERS.borrow_ref_mut(cs);
            let mut timer_slot = MODEM_LP_TIMER.borrow_ref_mut(cs);
            if let Err(error) = STORAGE_RESERVATION.borrow_ref(cs).admit_restore(
                BOUND_ROUTE_DISPATCH.borrow_ref(cs).is_some(),
                irq_slot.is_some(),
                timer_slot.is_some(),
            ) {
                return Err((error, interrupts, timer));
            }
            *irq_slot = Some(interrupts);
            *timer_slot = Some(StoredBluetoothModemLpTimerOwner::Ready(timer));
            Ok(())
        })
    }
}
