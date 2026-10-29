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
use oer_esp32s31_radio_client::bluetooth::{ControllerFault, PreparationFailure, RadioController};
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
    cells: &'static ControllerEventCells<X>,
    gate: BluetoothInterruptGate<ChipGate, X, R>,
    address: oer_bluetooth_le_wire::LePublicDeviceAddress,
    installation: Option<u32>,
    state: State<F, E, X, M, MT>,
}

#[allow(
    clippy::large_enum_variant,
    dead_code,
    reason = "each failure retains the complete hardware graph until chip reset"
)]
// Every stable phase holds its required owners directly. Transition is
// private to the exclusive consuming step; rejected calls restore the exact
// previous state. A cancelled reset keeps its in-flight owners in its future.
enum State<F, E, X: SignalExecutor, M: ControllerMemory, const MT: usize> {
    Fresh {
        factory: F,
        runtime: ControllerRuntimeResources<'static, MT, X>,
    },
    Transition,
    Prepared(ControllerLowPowerHardwareInitialized<'static, MT, X, M>),
    Ready(ControllerBlePhyEngineInitialized<'static, MT, X, M>),
    Running,
    Retired(ControllerRetired<M>),
    Reset {
        arena: oer_esp32s31_hal::memory::dma_static::Placed<M::Storage>,
        pending: oer_esp32s31_hal::bluetooth::BluetoothModemLpTimerPendingObservation,
    },
    FactoryFailed {
        _error: E,
        _runtime: ControllerRuntimeResources<'static, MT, X>,
    },
    HalFailed {
        _error: ControllerHalInitializationFailure<M>,
        _runtime: ControllerRuntimeResources<'static, MT, X>,
    },
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
        address: oer_bluetooth_le_wire::LePublicDeviceAddress,
    ) -> Self
    where
        F: ControllerFactory<M, Error = E>,
    {
        Self {
            cells,
            gate,
            address,
            installation: None,
            state: State::Fresh { factory, runtime },
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
        let ready = match core::mem::replace(&mut self.state, State::Transition) {
            State::Ready(ready) => ready,
            state => {
                self.state = state;
                return Err(ControllerFault);
            }
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
        self.state = State::Retired(retired);
        Ok(())
    }

    /// Reclaim the complete arena and the task's end of the interrupt
    /// handler's gate only after the actor completed leave and the matching
    /// Controller reset. Reinstall uses fresh generative scopes and the same
    /// gate: the handler's part stays bound for every epoch.
    #[allow(clippy::result_large_err, clippy::type_complexity)]
    pub fn into_reset(
        self,
    ) -> Result<
        (
            oer_esp32s31_hal::memory::dma_static::Placed<M::Storage>,
            BluetoothInterruptGate<ChipGate, X, R>,
            oer_esp32s31_hal::bluetooth::BluetoothModemLpTimerPendingObservation,
        ),
        Self,
    > {
        match self {
            Self {
                state: State::Reset { arena, pending },
                gate,
                ..
            } => Ok((arena, gate, pending)),
            owner => Err(owner),
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
        matches!(&self.state, State::Fresh { runtime, .. } if matches!(runtime.is_pristine(), Ok(true)))
    }

    #[inline(never)]
    fn prepare(&mut self, clocked: ClockedOwner) -> Result<u32, PreparationFailure> {
        if !<Self as RadioController<B>>::can_prepare(self) {
            return Err(PreparationFailure {
                returned: Some(clocked),
            });
        }
        let (factory, runtime) = match core::mem::replace(&mut self.state, State::Transition) {
            State::Fresh { factory, runtime } => (factory, runtime),
            state => {
                self.state = state;
                return Err(PreparationFailure {
                    returned: Some(clocked),
                });
            }
        };
        let (engine, interrupts) = match factory.build(clocked) {
            Ok(owners) => owners,
            Err(error) => {
                self.state = State::FactoryFailed {
                    _error: error,
                    _runtime: runtime,
                };
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
                self.state = State::HalFailed {
                    _error: error,
                    _runtime: runtime,
                };
                return Err(PreparationFailure { returned: None });
            }
        };
        let scheduler = match hal.initialize_scheduler(runtime) {
            Ok(scheduler) => scheduler,
            Err(error) => {
                self.state = State::SchedulerFailed(error);
                return Err(PreparationFailure { returned: None });
            }
        };
        match scheduler.initialize_modem_lp_timer_hardware() {
            Ok(controller) => {
                self.state = State::Prepared(controller);
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
            State::Prepared(controller) => Some(controller.phy_task_owner()),
            State::Retired(controller) => Some(controller.task_owner()),
            _ => None,
        }
    }

    #[inline(never)]
    fn initialize_phy(&mut self, ready: BluetoothRfReady<'_, B>) -> Result<(), ControllerFault> {
        let controller = match core::mem::replace(&mut self.state, State::Transition) {
            State::Prepared(controller) => controller,
            state => {
                self.state = state;
                return Err(ControllerFault);
            }
        };
        match controller.initialize_ble_phy_engine(ready, self.address) {
            Ok(controller) => {
                self.state = State::Ready(controller);
                Ok(())
            }
            Err(error) => {
                self.state = State::PhyFailed(error);
                Err(ControllerFault)
            }
        }
    }

    fn is_retired(&self, installation: u32) -> bool {
        self.installation == Some(installation) && matches!(self.state, State::Retired(_))
    }

    async fn reset(
        &mut self,
        radio: &mut RadioDomain<B, ConcurrentPhy>,
    ) -> Result<ClockedOwner, ControllerFault> {
        let retired = match core::mem::replace(&mut self.state, State::Transition) {
            State::Retired(retired) => retired,
            state => {
                self.state = state;
                return Err(ControllerFault);
            }
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
                    arena: returned.arena,
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
