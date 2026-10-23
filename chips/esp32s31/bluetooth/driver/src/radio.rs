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
use oer_interrupt_table::signal::SignalExecutor;

use crate::{
    ble_phy::{BlePhyRuntime, ControllerBlePhyEngineInitialized},
    controller_hal::{ControllerHalInitializationFailure, ControllerHalInitialized},
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
    fn reset(
        &mut self,
        radio: &mut RadioDomain<B, ConcurrentPhy>,
    ) -> Result<ClockedOwner, ControllerFault>;
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
pub struct ControllerLifecycle<F, E, X: SignalExecutor, M: ControllerMemory, const MT: usize> {
    factory: Option<F>,
    runtime: Option<ControllerRuntimeResources<'static, MT, X>>,
    cells: &'static ControllerEventCells<X>,
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
    Reset(Option<oer_esp32s31_hal::dma_static::Placed<M::Storage>>),
    FactoryFailed(E),
    HalFailed(ControllerHalInitializationFailure<M>),
    SchedulerFailed(SchedulerInitializationFailure<'static, MT, X, M>),
    LowPowerFailed(ControllerLowPowerHardwareInitializationFailure<'static, MT, X, M>),
    PhyFailed(crate::ble_phy::ControllerBlePhyInitializationFailure<'static, MT, X, M>),
    ResetFailed(ControllerShutdownFailure<X, M>),
}

impl<F, E, X: SignalExecutor, M: ControllerMemory, const MT: usize>
    ControllerLifecycle<F, E, X, M, MT>
{
    pub fn new(
        factory: F,
        runtime: ControllerRuntimeResources<'static, MT, X>,
        cells: &'static ControllerEventCells<X>,
        address: oer_bluetooth_hci::BluetoothPublicDeviceAddress,
    ) -> Self
    where
        F: ControllerFactory<M, Error = E>,
    {
        Self {
            factory: Some(factory),
            runtime: Some(runtime),
            cells,
            address,
            installation: None,
            state: State::Empty,
        }
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

    /// Reclaim the complete arena only after the actor completed leave and
    /// the matching Controller reset. Reinstall uses fresh generative scopes.
    #[allow(clippy::result_large_err)]
    pub fn into_reset(mut self) -> Result<oer_esp32s31_hal::dma_static::Placed<M::Storage>, Self> {
        let arena = match &mut self.state {
            State::Reset(arena) => arena.take(),
            _ => None,
        };
        match arena {
            Some(arena) => Ok(arena),
            None => Err(self),
        }
    }
}

impl<B, F, E, X, M, const MT: usize> RadioController<B> for ControllerLifecycle<F, E, X, M, MT>
where
    B: PlatformClockBackend,
    F: ControllerFactory<M, Error = E>,
    X: SignalExecutor,
    M: ControllerMemory,
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

    #[inline(never)]
    fn reset(
        &mut self,
        radio: &mut RadioDomain<B, ConcurrentPhy>,
    ) -> Result<ClockedOwner, ControllerFault> {
        let retired = match &mut self.state {
            State::Retired(retired) => retired.take().ok_or(ControllerFault)?,
            _ => return Err(ControllerFault),
        };
        match retired.shut_down(radio, self.cells) {
            Ok(returned) => {
                self.state = State::Reset(Some(returned.arena));
                Ok(returned.clocked)
            }
            Err(error) => {
                self.state = State::ResetFailed(error);
                Err(ControllerFault)
            }
        }
    }
}
