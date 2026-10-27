//! Controller ownership parked at the shared radio actor's lifecycle edges.
//!
//! Branded controller memory stays in this P2 owner. Membership cells carry
//! installation numbers, never an Engine or a controller address.

use oer_esp32s31_hal::{
    bluetooth::engine::{ControllerMemory, Engine},
    bluetooth::{ClockedOwner, InterruptSetupOwner, TaskOwner},
    domain::RadioDomain,
    power::PlatformClockBackend,
};
use oer_esp32s31_phy::{bluetooth_client::BluetoothRfReady, concurrent::ConcurrentPhy};
use oer_espressif_interrupt_table::signal::SignalExecutor;

use crate::{
    ble_phy::{BlePhyRuntime, ControllerBlePhyEngineInitialized},
    controller_hal::{ControllerHalInitializationFailure, ControllerHalInitialized},
    interrupt::{BluetoothInterruptGate, ChipGate, RadioDoorbell},
    low_power::{
        ControllerLowPowerHardwareInitializationFailure, ControllerLowPowerHardwareInitialized,
    },
    runtime_resources::{ControllerEventCells, ControllerRuntimeResources},
    scheduler::SchedulerInitializationFailure,
    shutdown::{ControllerRetired, ControllerShutdownFailure},
};

/// Narrow controller operations performed only by the shared actor.
pub trait RadioController<B: PlatformClockBackend> {
    fn can_prepare(&self) -> bool;
    fn prepare(&mut self, clocked: ClockedOwner) -> Result<u32, PreparationFailure>;
    fn task_owner(&self) -> Option<&TaskOwner>;
    fn initialize_phy(&mut self, ready: BluetoothRfReady<'_, B>) -> Result<(), ControllerFault>;
    fn is_retired(&self, installation: u32) -> bool;
    /// Reset the retired Controller, then take its timer from the timer's
    /// interrupt handler, which gives it up only after the reset.
    fn reset(
        &mut self,
        radio: &mut RadioDomain<B, ConcurrentPhy>,
    ) -> impl core::future::Future<Output = Result<ClockedOwner, ControllerFault>>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControllerFault;

/// Preflight returns the untouched clocked owner; a started failure retains
/// the owner inside the controller lifecycle instead.
pub struct PreparationFailure {
    pub returned: Option<ClockedOwner>,
}

/// A concrete, consuming factory for one branded Controller installation.
/// Its named type can live in a placed async task without opaque closure
/// type cycles; failed construction retains the complete returned inputs.
pub trait ControllerFactory<M: ControllerMemory> {
    type Error;
    fn build(self, clocked: ClockedOwner) -> Result<(Engine<M>, InterruptSetupOwner), Self::Error>;
}

/// One generatively scoped installation, from clock setup through reset.
/// `factory` binds a fresh pool/epoch and the placed arena after clocks exist.
pub struct ControllerLifecycle<
    F,
    E,
    X: SignalExecutor + 'static,
    M: ControllerMemory,
    R,
    const MT: usize,
> {
    factory: Option<F>,
    runtime: Option<ControllerRuntimeResources<'static, MT, X>>,
    cells: &'static ControllerEventCells<X>,
    gate: BluetoothInterruptGate<ChipGate, X, R>,
    address: oer_bluetooth_hci::BluetoothPublicDeviceAddress,
    installation: Option<u32>,
    state: State<E, X, M, MT>,
}

#[allow(
    clippy::large_enum_variant,
    dead_code,
    reason = "each failure retains the complete hardware graph until chip reset"
)]
// An in-place take moves only the active owner, never a maximum-sized
// retained failure through Empty and back on an invalid lifecycle call.
// None is transient while the same exclusive call owns its payload.
enum State<E, X: SignalExecutor, M: ControllerMemory, const MT: usize> {
    Empty,
    Prepared(Option<ControllerLowPowerHardwareInitialized<'static, MT, X, M>>),
    Ready(Option<ControllerBlePhyEngineInitialized<'static, MT, X, M>>),
    Running,
    Retired(Option<ControllerRetired<M>>),
    Reset {
        arena: Option<oer_esp32s31_hal::dma_static::Placed<M::Storage>>,
        pending: oer_esp32s31_hal::bluetooth::BluetoothModemLpTimerPendingObservation,
    },
    FactoryFailed(E),
    HalFailed(ControllerHalInitializationFailure<M>),
    SchedulerFailed(SchedulerInitializationFailure<'static, MT, X, M>),
    LowPowerFailed(ControllerLowPowerHardwareInitializationFailure<'static, MT, X, M>),
    PhyFailed(crate::ble_phy::ControllerBlePhyInitializationFailure<'static, MT, X, M>),
    ResetFailed(ControllerShutdownFailure<X, M>),
}

impl<F, E, X: SignalExecutor + 'static, M: ControllerMemory, R, const MT: usize>
    ControllerLifecycle<F, E, X, M, R, MT>
{
    /// `gate` is the task's end of the Bluetooth interrupt handler's gate;
    /// the lifecycle keeps it for the timer's reunion after reset and lends
    /// it to the composition between.
    pub fn new(
        factory: F,
        runtime: ControllerRuntimeResources<'static, MT, X>,
        cells: &'static ControllerEventCells<X>,
        gate: BluetoothInterruptGate<ChipGate, X, R>,
        address: oer_bluetooth_hci::BluetoothPublicDeviceAddress,
    ) -> Self
    where
        F: ControllerFactory<M, Error = E>,
    {
        Self {
            factory: Some(factory),
            runtime: Some(runtime),
            cells,
            gate,
            address,
            installation: None,
            state: State::Empty,
        }
    }

    /// The task's end of the interrupt handler's gate, which opens and
    /// closes each epoch.
    pub fn gate_mut(&mut self) -> &mut BluetoothInterruptGate<ChipGate, X, R> {
        &mut self.gate
    }

    /// Transfer the initialized graph to the BLE backend once, between actor
    /// run borrows. The shared PHY membership remains in the actor backend.
    #[inline(never)]
    pub fn take_runtime(
        &mut self,
        installation: u32,
    ) -> Result<BlePhyRuntime<'static, MT, X, M>, ControllerFault> {
        if self.installation != Some(installation) {
            return Err(ControllerFault);
        }
        let ready = match &mut self.state {
            State::Ready(ready) => ready.take().ok_or(ControllerFault)?,
            _ => return Err(ControllerFault),
        };
        self.state = State::Running;
        Ok(ready.activate())
    }

    /// A late or foreign completion returns its entire old graph unchanged.
    /// Only the current installation can satisfy the actor's leave request.
    #[allow(clippy::result_large_err)]
    pub fn return_retired(
        &mut self,
        retired: ControllerRetired<M>,
    ) -> Result<(), ControllerRetired<M>> {
        if !matches!(self.state, State::Running)
            || self.installation != Some(retired.installation())
        {
            return Err(retired);
        }
        self.state = State::Retired(Some(retired));
        Ok(())
    }

    /// Reclaim the complete arena and the task's end of the interrupt
    /// handler's gate only after the actor completed leave and the matching
    /// Controller reset. Reinstall uses fresh generative scopes and the same
    /// gate: the handler's part stays bound for every epoch.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn into_reset(
        mut self,
    ) -> Result<
        (
            oer_esp32s31_hal::dma_static::Placed<M::Storage>,
            BluetoothInterruptGate<ChipGate, X, R>,
            oer_esp32s31_hal::bluetooth::BluetoothModemLpTimerPendingObservation,
        ),
        Self,
    > {
        match &mut self.state {
            State::Reset { arena, pending } => match arena.take() {
                Some(arena) => Ok((arena, self.gate, *pending)),
                None => Err(self),
            },
            _ => Err(self),
        }
    }
}

impl<B, F, E, X, M, R, const MT: usize> RadioController<B>
    for ControllerLifecycle<F, E, X, M, R, MT>
where
    B: PlatformClockBackend,
    F: ControllerFactory<M, Error = E>,
    X: SignalExecutor + 'static,
    M: ControllerMemory,
    R: RadioDoorbell,
{
    fn can_prepare(&self) -> bool {
        matches!(self.state, State::Empty)
            && self.factory.is_some()
            && self
                .runtime
                .as_ref()
                .is_some_and(|runtime| matches!(runtime.is_pristine(), Ok(true)))
    }

    #[inline(never)]
    fn prepare(&mut self, clocked: ClockedOwner) -> Result<u32, PreparationFailure> {
        if !<Self as RadioController<B>>::can_prepare(self) {
            return Err(PreparationFailure {
                returned: Some(clocked),
            });
        }
        let factory = self
            .factory
            .take()
            .expect("actor preflight retains the one controller factory");
        let (engine, interrupts) = match factory.build(clocked) {
            Ok(owners) => owners,
            Err(error) => {
                self.state = State::FactoryFailed(error);
                return Err(PreparationFailure { returned: None });
            }
        };
        let installation = engine.installation();
        self.installation = Some(installation);
        let hal = match ControllerHalInitialized::initialize(engine, interrupts) {
            Ok(hal) => hal,
            Err(error) => {
                if let crate::controller_hal::ControllerHalInitializationError::InterruptPolicy(
                    fault,
                ) = error.error
                {
                    self.gate.quarantine_interrupt_policy(fault);
                }
                self.state = State::HalFailed(error);
                return Err(PreparationFailure { returned: None });
            }
        };
        let runtime = self
            .runtime
            .take()
            .expect("actor preflight retains the one pristine runtime");
        let scheduler = match hal.initialize_scheduler(runtime) {
            Ok(scheduler) => scheduler,
            Err(error) => {
                self.state = State::SchedulerFailed(error);
                return Err(PreparationFailure { returned: None });
            }
        };
        match scheduler.initialize_modem_lp_timer_hardware() {
            Ok(controller) => {
                self.state = State::Prepared(Some(controller));
                Ok(installation)
            }
            Err(error) => {
                self.state = State::LowPowerFailed(error);
                Err(PreparationFailure { returned: None })
            }
        }
    }

    fn task_owner(&self) -> Option<&TaskOwner> {
        match &self.state {
            State::Prepared(Some(controller)) => Some(controller.phy_task_owner()),
            State::Retired(Some(controller)) => Some(controller.task_owner()),
            _ => None,
        }
    }

    #[inline(never)]
    fn initialize_phy(&mut self, ready: BluetoothRfReady<'_, B>) -> Result<(), ControllerFault> {
        let controller = match &mut self.state {
            State::Prepared(controller) => controller.take().ok_or(ControllerFault)?,
            _ => return Err(ControllerFault),
        };
        match controller.initialize_ble_phy_engine(ready, self.address) {
            Ok(controller) => {
                self.state = State::Ready(Some(controller));
                Ok(())
            }
            Err(error) => {
                self.state = State::PhyFailed(error);
                Err(ControllerFault)
            }
        }
    }

    fn is_retired(&self, installation: u32) -> bool {
        self.installation == Some(installation) && matches!(self.state, State::Retired(Some(_)))
    }

    async fn reset(
        &mut self,
        radio: &mut RadioDomain<B, ConcurrentPhy>,
    ) -> Result<ClockedOwner, ControllerFault> {
        let retired = match &mut self.state {
            State::Retired(retired) => retired.take().ok_or(ControllerFault)?,
            _ => return Err(ControllerFault),
        };
        let pending = match retired.shut_down(radio, self.cells) {
            Ok(pending) => pending,
            Err(error) => {
                self.state = State::ResetFailed(error);
                return Err(ControllerFault);
            }
        };
        match pending.reunite(&mut self.gate).await {
            Ok(returned) => {
                self.state = State::Reset {
                    arena: Some(returned.arena),
                    pending: returned.pending,
                };
                Ok(returned.clocked)
            }
            Err(error) => {
                self.state = State::ResetFailed(error);
                Err(ControllerFault)
            }
        }
    }
}
