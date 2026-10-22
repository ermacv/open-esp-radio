//! One placed allocation and its exclusive controller-register capability.

use super::le_rx_chain::RxPlan;
use super::scheduler_pool::PoolPlan;
use super::*;
use crate::{
    bluetooth::{ControllerArenaState, ControllerHal, ControllerHalBorrow, TaskOwner},
    dma_static::Placed,
};

/// Controller-visible backing. Capacities are selected by the composition.
/// Tuple fields split placement without constructing another placement proof.
pub type BluetoothArena<
    const A: usize,
    const C: usize,
    const N: usize,
    const R: usize,
    const O: usize,
> = (
    (
        BlePhyEngineStorage,
        (DirectionFindingWorkspaceStorage, LeDeviceTableStorage),
    ),
    (
        (
            SchedulerRolePoolStorage<DtmStorage, 1>,
            SchedulerRolePoolStorage<LegacyAdvertisingStorage, A>,
        ),
        (
            (
                SchedulerRolePoolStorage<LegacyConnectableAdvertisingStorage, C>,
                SchedulerRolePoolStorage<LegacyScanStorage, 1>,
            ),
            (
                SchedulerRolePoolStorage<PeripheralConnectionStorage, N>,
                (LeRxChainStorage<R>, LeRxChainStorage<O>),
            ),
        ),
    ),
);

/// Why an arena cannot be installed. Rejection returns both input owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArenaError {
    AlreadyInstalled,
    NotInitialized,
    AllocationNumbers,
    Pool(SchedulerPoolPlaceError),
    Receive(LeRxChainPlaceError),
}

/// Unique owner of the controller-memory protocol. Physical links and CPU
/// views stay in its owned components; callers receive semantic values and
/// leases. Dropping it retains static backing even if hardware still walks it.
pub struct Engine<
    'registers,
    'pool,
    'epoch,
    const A: usize,
    const C: usize,
    const N: usize,
    const R: usize,
    const O: usize,
    const Q: usize,
> {
    pub(super) controller: ControllerHal<'registers>,
    pub(super) dtm: DtmPool<'pool, 'epoch, 1>,
    pub(super) advertising: LegacyAdvertisingPool<'pool, 'epoch, A>,
    pub(super) connectable: LegacyConnectableAdvertisingPool<'pool, 'epoch, C>,
    pub(super) scanning: LegacyScanPool<'pool, 'epoch, 1>,
    pub(super) connections: PeripheralConnectionPool<'pool, 'epoch, N>,
    phy: BlePhyEngineCpuOwned,
    direction_finding: DirectionFindingWorkspaceCpuOwned,
    devices: LeDeviceTable,
    pub(super) scanning_rx: LeRxChain<R>,
    pub(super) other_rx: LeRxChain<O>,
    pub(super) scheduler: super::scheduler::SchedulerExecutor<SchedulerItemId, Q>,
    #[cfg(any(target_arch = "riscv32", test))]
    pub(super) hardware:
        super::scheduler::hardware::SchedulerHardware<super::scheduler::hardware::HalPublications>,
    pub(super) scanning_publication: Option<crate::bluetooth::RxMemoryListPublished>,
}

impl<
    'registers,
    'pool,
    'epoch,
    const A: usize,
    const C: usize,
    const N: usize,
    const R: usize,
    const O: usize,
    const Q: usize,
> Engine<'registers, 'pool, 'epoch, A, C, N, R, O, Q>
{
    /// Validate all capacities before consuming either input. Initialization
    /// writes only the CPU-owned arena; it performs no controller publication.
    #[allow(
        clippy::type_complexity,
        reason = "rejection returns the exact register and arena owners"
    )]
    pub fn new<D: ControllerDmaSpace>(
        registers: EngineRegisters<'registers>,
        placed: Placed<BluetoothArena<A, C, N, R, O>>,
        _pool: PoolScope<'pool>,
        _epoch: EpochScope<'epoch>,
        config: SchedulerAllocationConfig,
    ) -> Result<
        Self,
        (
            ArenaError,
            EngineRegisters<'registers>,
            Placed<BluetoothArena<A, C, N, R, O>>,
        ),
    > {
        let prepare = || {
            let a = u16::try_from(A).map_err(|_| ArenaError::AllocationNumbers)?;
            let c = u16::try_from(C).map_err(|_| ArenaError::AllocationNumbers)?;
            let a_numbers = config
                .advertising(0, a)
                .ok_or(ArenaError::AllocationNumbers)?;
            let c_numbers = config
                .advertising(a, c)
                .ok_or(ArenaError::AllocationNumbers)?;
            Ok::<_, ArenaError>((
                PoolPlan::<DtmStorage, 1>::new(config.direct_test_mode())
                    .map_err(ArenaError::Pool)?,
                PoolPlan::<LegacyAdvertisingStorage, A>::new(a_numbers)
                    .map_err(ArenaError::Pool)?,
                PoolPlan::<LegacyConnectableAdvertisingStorage, C>::new(c_numbers)
                    .map_err(ArenaError::Pool)?,
                PoolPlan::<LegacyScanStorage, 1>::new(config.scanning())
                    .map_err(ArenaError::Pool)?,
                PoolPlan::<PeripheralConnectionStorage, N>::new(config.connections())
                    .map_err(ArenaError::Pool)?,
                RxPlan::<R>::new().map_err(ArenaError::Receive)?,
                RxPlan::<O>::new().map_err(ArenaError::Receive)?,
            ))
        };
        let (dtm_plan, adv_plan, conn_adv_plan, scan_plan, conn_plan, rx_plan, other_plan) =
            match prepare() {
                Ok(plans) => plans,
                Err(error) => return Err((error, registers, placed)),
            };
        let EngineRegisters { controller } = registers;
        let controller_epoch = controller.epoch.0;
        *controller.arena_state = ControllerArenaState::Bound;
        let (storage, placement) = Placement::<D>::take(placed);
        let (
            (phy, (df, devices)),
            ((dtm, advertising), ((connectable, scanning), (connections, (scanning_rx, other_rx)))),
        ) = storage;
        Ok(Self {
            controller,
            dtm: SchedulerRolePool::from_parts(dtm, &placement, dtm_plan, controller_epoch),
            advertising: SchedulerRolePool::from_parts(
                advertising,
                &placement,
                adv_plan,
                controller_epoch,
            ),
            connectable: SchedulerRolePool::from_parts(
                connectable,
                &placement,
                conn_adv_plan,
                controller_epoch,
            ),
            scanning: SchedulerRolePool::from_parts(
                scanning,
                &placement,
                scan_plan,
                controller_epoch,
            ),
            connections: SchedulerRolePool::from_parts(
                connections,
                &placement,
                conn_plan,
                controller_epoch,
            ),
            phy: BlePhyEngineStorage::from_parts(phy, &placement),
            direction_finding: DirectionFindingWorkspaceStorage::from_parts(df, &placement),
            devices: LeDeviceTable::from_parts(devices, &placement),
            scanning_rx: LeRxChain::from_parts(
                scanning_rx,
                &placement,
                RxMemoryListClass::Scanning,
                rx_plan,
            ),
            other_rx: LeRxChain::from_parts(
                other_rx,
                &placement,
                RxMemoryListClass::NonScanning,
                other_plan,
            ),
            scheduler: super::scheduler::SchedulerExecutor::new(),
            #[cfg(any(target_arch = "riscv32", test))]
            hardware: super::scheduler::hardware::SchedulerHardware::new(),
            scanning_publication: None,
        })
    }

    #[allow(
        clippy::type_complexity,
        reason = "refusal returns initialized registers and placement without erasing their types"
    )]
    pub fn bind<D: ControllerDmaSpace>(
        task: &'registers mut TaskOwner,
        arena: Placed<BluetoothArena<A, C, N, R, O>>,
        pool: PoolScope<'pool>,
        epoch: EpochScope<'epoch>,
        config: SchedulerAllocationConfig,
    ) -> Result<
        Self,
        EngineBindFailure<EngineRegisters<'registers>, Placed<BluetoothArena<A, C, N, R, O>>>,
    > {
        let registers = match task.claim_engine_registers() {
            Ok(registers) => registers,
            Err(error) => return Err(EngineBindFailure::Registers { error, arena }),
        };
        Self::new::<D>(registers, arena, pool, epoch, config).map_err(
            |(error, registers, arena)| EngineBindFailure::Arena {
                error,
                registers,
                arena,
            },
        )
    }

    pub fn dtm(&mut self) -> &mut DtmPool<'pool, 'epoch, 1> {
        &mut self.dtm
    }
    pub fn advertising(&mut self) -> &mut LegacyAdvertisingPool<'pool, 'epoch, A> {
        &mut self.advertising
    }
    pub fn connectable_advertising(
        &mut self,
    ) -> &mut LegacyConnectableAdvertisingPool<'pool, 'epoch, C> {
        &mut self.connectable
    }
    pub fn scanning(&mut self) -> &mut LegacyScanPool<'pool, 'epoch, 1> {
        &mut self.scanning
    }
    pub fn connections(&mut self) -> &mut PeripheralConnectionPool<'pool, 'epoch, N> {
        &mut self.connections
    }

    pub fn devices(&mut self) -> &mut LeDeviceTable {
        &mut self.devices
    }

    /// Number of events retained by the scheduler mirror.
    pub fn scheduled_count(&self) -> usize {
        self.scheduler.list().len()
    }

    pub fn reset_scanning(
        &mut self,
        lease: &SchedulerRoleInstance<'pool, 'epoch>,
        config: LegacyScanResetConfig,
    ) -> Result<(), LegacyScanError> {
        self.scanning.reset(lease, &self.scanning_rx, config)
    }

    pub fn prepare_connectable_advertising(
        &mut self,
        lease: &SchedulerRoleInstance<'pool, 'epoch>,
        input: LegacyConnectableAdvertisingMemoryInput<'_>,
        power: LeTxPower,
    ) -> Result<(), LegacyConnectableAdvertisingError> {
        self.connectable
            .prepare(lease, input, &self.other_rx, power)
    }

    pub fn prepare_first_connection(
        &mut self,
        lease: &SchedulerRoleInstance<'pool, 'epoch>,
        event: PeripheralConnectionFirstEvent,
    ) -> Result<PeripheralConnectionEvent, PeripheralConnectionError> {
        self.connections
            .prepare_first_event(lease, event, self.direction_finding.binding().link())
    }

    pub fn le_1m_packet_start_calibration(&self) -> BlePhyLe1MPacketStartCalibration {
        self.phy.le_1m_packet_start_calibration()
    }

    pub fn receive(&mut self, source: LeRxSource) -> Result<Option<LeRxOutcome>, LeRxChainError> {
        match source.class() {
            RxMemoryListClass::Scanning => self.scanning_rx.take(source),
            RxMemoryListClass::NonScanning => self.other_rx.take(source),
        }
    }

    /// Publish the address and extent held by this engine, without exporting it.
    pub fn publish_device_table(&mut self) {
        let _ = self
            .controller
            .publish_device_table(&self.devices.publication());
    }

    /// Initialize the controller's disabled-CTE descriptor from retained storage.
    pub fn publish_direction_finding(
        &mut self,
    ) -> crate::bluetooth::DirectionFindingDisabledBaselineOwner {
        self.controller.prepare_direction_finding_disabled_baseline(
            &self.direction_finding.disabled_cte_descriptor(),
        )
    }
}

#[cfg(test)]
mod tests;

/// Exclusive register token whose HAL SRAM-prefix initialization completed.
pub struct EngineRegisters<'registers> {
    controller: ControllerHal<'registers>,
}

/// Rejection retains placement and, once claimed, its initialized registers.
pub enum EngineBindFailure<Registers, Arena> {
    Registers {
        error: ArenaError,
        arena: Arena,
    },
    Arena {
        error: ArenaError,
        registers: Registers,
        arena: Arena,
    },
}

impl<R, A> EngineBindFailure<R, A> {
    pub const fn error(&self) -> ArenaError {
        match self {
            Self::Registers { error, .. } | Self::Arena { error, .. } => *error,
        }
    }
}

impl TaskOwner {
    /// Claim a memory-engine capability after the hardware HAL transaction.
    /// An installed arena reserves this hardware epoch even after Drop.
    pub fn claim_engine_registers(&mut self) -> Result<EngineRegisters<'_>, ArenaError> {
        match self.arena_state {
            ControllerArenaState::Uninitialized => return Err(ArenaError::NotInitialized),
            ControllerArenaState::Bound => return Err(ArenaError::AlreadyInstalled),
            ControllerArenaState::Ready => {}
        }
        Ok(EngineRegisters {
            controller: self.borrow_bluetooth_controller(),
        })
    }
}
