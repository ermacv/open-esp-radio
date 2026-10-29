//! One register owner and one placed controller graph for an entire epoch.

#![allow(
    private_interfaces,
    reason = "the sealed memory trait never exposes its private component owners"
)]

use super::{le_rx_chain::RxPlan, scheduler_pool::PoolPlan, *};
use crate::{bluetooth::TaskOwner, memory::dma_static::Placed};

/// Complete controller backing, with capacities chosen by the composition.
///
/// `S = 0` selects the DTM-only profile: `A`, `C`, `N`, `R` and `O` must
/// also be zero. It contains neither scanner nor global RX graphs, including
/// their packetless cursors. `S = 1` includes those graphs and enables the
/// other role APIs. Each global RX chain then requires at least two packets.
/// The generated graph layouts and their placement protocol are identical
/// in both profiles.
pub type BluetoothArena<
    const A: usize,
    const C: usize,
    const N: usize,
    const S: usize,
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
                SchedulerRolePoolStorage<LegacyScanStorage, S>,
            ),
            (
                SchedulerRolePoolStorage<PeripheralConnectionStorage, N>,
                ([LeRxChainStorage<R>; S], [LeRxChainStorage<O>; S]),
            ),
        ),
    ),
);

/// Constructor rejection, before any register or allocation mutation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArenaError {
    AllocationNumbers,
    Pool(SchedulerPoolPlaceError),
    Receive(LeRxChainPlaceError),
    SchedulerCapacity,
    EpochExhausted,
    Placement(crate::memory::dma_address::ProjectionError),
}

use oer_espressif_bluetooth_engine::phase::EngineEpoch;

/// Hardware reachability became uncertain; the arena remains quarantined.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EngineFault;

pub(super) mod sealed {
    use super::*;
    pub trait Memory: Sized {
        type Storage: 'static;
        fn phy(&self) -> &BlePhyEngineCpuOwned;
        fn direction_finding(&self) -> &DirectionFindingWorkspaceCpuOwned;
        fn retirement_ready(&self) -> bool;
        fn scheduler_stopped(&self) -> bool;
        fn reclaim(
            self,
            reset: &crate::bluetooth::BluetoothControllerReset,
        ) -> Placed<Self::Storage>;
    }
}

/// Sealed, capacity-specific custody of a complete controller arena.
/// Drivers carry this type through lifecycle states without erasing its brands.
#[allow(private_bounds)]
pub trait ControllerMemory: sealed::Memory {}

/// The arena's private graph owners, carried only inside its owning Engine.
pub struct ArenaMemory<
    'pool,
    'epoch,
    const A: usize,
    const C: usize,
    const N: usize,
    const S: usize,
    const R: usize,
    const O: usize,
    const Q: usize,
> {
    root: core::ptr::NonNull<BluetoothArena<A, C, N, S, R, O>>,
    pub(super) dtm: DtmPool<'pool, 'epoch, 1>,
    pub(super) advertising: LegacyAdvertisingPool<'pool, 'epoch, A>,
    pub(super) connectable: LegacyConnectableAdvertisingPool<'pool, 'epoch, C>,
    pub(super) scanning: LegacyScanPool<'pool, 'epoch, S>,
    pub(super) connections: PeripheralConnectionPool<'pool, 'epoch, N>,
    phy: BlePhyEngineCpuOwned,
    direction_finding: DirectionFindingWorkspaceCpuOwned,
    pub(super) devices: LeDeviceTable,
    pub(super) scanning_rx: [LeRxChain<R>; S],
    pub(super) other_rx: [LeRxChain<O>; S],
    pub(super) scheduler: scheduler::SchedulerExecutor<SchedulerItemId, Q>,
    #[cfg(any(target_arch = "riscv32", test))]
    pub(super) hardware:
        scheduler::hardware::SchedulerHardware<scheduler::hardware::HalPublications>,
    pub(super) scanning_publication: Option<crate::bluetooth::RxMemoryListPublished>,
}

/// The unique controller-register token and all memory it can publish.
/// There is no extraction of registers or individual PHY/DF owners.
#[must_use = "the engine owns the controller and arena until verified reset"]
pub struct Engine<M: ControllerMemory> {
    pub(super) epoch: EngineEpoch,
    pub(super) registers: TaskOwner,
    pub(super) memory: M,
    pub(super) scheduler_interrupts: Option<scheduler::interrupt::SchedulerTaskHalf>,
    pub(super) direction_finding_publication:
        Option<crate::bluetooth::DirectionFindingDisabledBaselineOwner>,
    pub(super) faulted: bool,
    pub(super) initialized: bool,
    pub(super) phy_initialized: bool,
}

impl<
    'pool,
    'epoch,
    const A: usize,
    const C: usize,
    const N: usize,
    const S: usize,
    const R: usize,
    const O: usize,
    const Q: usize,
> Engine<ArenaMemory<'pool, 'epoch, A, C, N, S, R, O, Q>>
{
    /// Unscoped brands for explicit register-model fixtures only.
    #[cfg(any(test, feature = "test-support"))]
    #[doc(hidden)]
    #[allow(clippy::type_complexity, clippy::result_large_err)]
    pub fn for_host_test<D: ControllerDmaSpace>(
        clocked: crate::bluetooth::ClockedOwner,
        placed: Placed<BluetoothArena<A, C, N, S, R, O>>,
        config: SchedulerAllocationConfig,
    ) -> Result<
        (Self, crate::bluetooth::InterruptSetupOwner),
        (
            ArenaError,
            crate::bluetooth::ClockedOwner,
            Placed<BluetoothArena<A, C, N, S, R, O>>,
        ),
    > {
        Self::new::<D>(
            clocked,
            placed,
            PoolScope::for_host_test(),
            EpochScope::for_host_test(),
            config,
        )
    }

    /// Validate all capacities before taking either input owner. Only a fresh
    /// task partition is admitted; HAL initialization runs through this owner.
    ///
    /// Raw static storage does not provide placement proof for the PHY engine
    /// or the other controller objects:
    ///
    /// ```compile_fail,E0308
    /// use oer_esp32s31_hal::bluetooth::{ClockedOwner, engine::{
    ///     ArenaMemory, BluetoothArena, ControllerDmaSpace, Engine,
    ///     EpochScope, PoolScope, SchedulerAllocationConfig,
    /// }};
    /// fn from_raw<'pool, 'epoch, D: ControllerDmaSpace>(
    ///     clocked: ClockedOwner,
    ///     storage: &'static mut BluetoothArena<0, 0, 0, 0, 0, 0>,
    ///     pool: PoolScope<'pool>, epoch: EpochScope<'epoch>,
    ///     config: SchedulerAllocationConfig,
    /// ) {
    ///     let _ = Engine::<ArenaMemory<'pool, 'epoch, 0, 0, 0, 0, 0, 0, 1>>::new::<D>(
    ///         clocked, storage, pool, epoch, config,
    ///     );
    /// }
    /// ```
    #[allow(clippy::type_complexity, clippy::result_large_err)]
    pub fn new<D: ControllerDmaSpace>(
        clocked: crate::bluetooth::ClockedOwner,
        placed: Placed<BluetoothArena<A, C, N, S, R, O>>,
        _pool: PoolScope<'pool>,
        _epoch: EpochScope<'epoch>,
        config: SchedulerAllocationConfig,
    ) -> Result<
        (Self, crate::bluetooth::InterruptSetupOwner),
        (
            ArenaError,
            crate::bluetooth::ClockedOwner,
            Placed<BluetoothArena<A, C, N, S, R, O>>,
        ),
    > {
        const {
            assert!(
                S <= 1,
                "the scanner/global-RX graph capacity is zero or one"
            );
            assert!(
                S == 1 || (A == 0 && C == 0 && N == 0 && R == 0 && O == 0),
                "the DTM-only profile contains no other role or global RX graphs"
            );
        }
        let prepare = || {
            if Q == 0 {
                return Err(ArenaError::SchedulerCapacity);
            }
            let a = u16::try_from(A).map_err(|_| ArenaError::AllocationNumbers)?;
            let c = u16::try_from(C).map_err(|_| ArenaError::AllocationNumbers)?;
            let adv = config
                .advertising(0, a)
                .ok_or(ArenaError::AllocationNumbers)?;
            let conn_adv = config
                .advertising(a, c)
                .ok_or(ArenaError::AllocationNumbers)?;
            Ok((
                PoolPlan::<DtmStorage, 1>::new(config.direct_test_mode())
                    .map_err(ArenaError::Pool)?,
                PoolPlan::<LegacyAdvertisingStorage, A>::new(adv).map_err(ArenaError::Pool)?,
                PoolPlan::<LegacyConnectableAdvertisingStorage, C>::new(conn_adv)
                    .map_err(ArenaError::Pool)?,
                PoolPlan::<LegacyScanStorage, S>::new(config.scanning())
                    .map_err(ArenaError::Pool)?,
                PoolPlan::<PeripheralConnectionStorage, N>::new(config.connections())
                    .map_err(ArenaError::Pool)?,
                if S == 0 {
                    None
                } else {
                    Some(RxPlan::<R>::new().map_err(ArenaError::Receive)?)
                },
                if S == 0 {
                    None
                } else {
                    Some(RxPlan::<O>::new().map_err(ArenaError::Receive)?)
                },
            ))
        };
        let (dtm_plan, adv_plan, conn_adv_plan, scan_plan, conn_plan, mut rx_plan, mut other_plan) =
            match prepare() {
                Ok(plans) => plans,
                Err(error) => return Err((error, clocked, placed)),
            };
        static EPOCHS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
        let Some(installation) = EngineEpoch::allocate(&EPOCHS) else {
            return Err((ArenaError::EpochExhausted, clocked, placed));
        };
        let projected = crate::memory::dma_address::PlacementProjection::try_with_controller::<
            D,
            _,
            _,
        >(placed, |storage, projection| {
            direction_finding_workspace::DirectionFindingWorkspaceBinding::new(
                &storage.0.1.0,
                projection,
            )
        });
        let (storage, projection, df_binding) = match projected {
            Ok(parts) => parts,
            Err((error, placed)) => return Err((ArenaError::Placement(error), clocked, placed)),
        };
        let placement = Placement::<D>::from_projection(projection);
        let (registers, interrupts) = clocked.separate_interrupt_owner();
        let epoch = installation.sequence();
        let root = core::ptr::NonNull::from(storage);
        #[allow(
            unsafe_code,
            reason = "all field borrows descend from the retained allocation pointer"
        )]
        // SAFETY: root consumes the allocation's only mutable reference. It
        // is retained without dereferencing until these field owners end.
        let storage = unsafe { &mut *root.as_ptr() };
        let (
            (phy, (df, devices)),
            ((dtm, advertising), ((connectable, scanning), (connections, (scanning_rx, other_rx)))),
        ) = storage;
        Ok((
            Self {
                epoch: installation,
                scheduler_interrupts: None,
                registers,
                direction_finding_publication: None,
                faulted: false,
                initialized: false,
                phy_initialized: false,
                memory: ArenaMemory {
                    root,
                    dtm: SchedulerRolePool::from_parts(dtm, &placement, dtm_plan, epoch),
                    advertising: SchedulerRolePool::from_parts(
                        advertising,
                        &placement,
                        adv_plan,
                        epoch,
                    ),
                    connectable: SchedulerRolePool::from_parts(
                        connectable,
                        &placement,
                        conn_adv_plan,
                        epoch,
                    ),
                    scanning: SchedulerRolePool::from_parts(scanning, &placement, scan_plan, epoch),
                    connections: SchedulerRolePool::from_parts(
                        connections,
                        &placement,
                        conn_plan,
                        epoch,
                    ),
                    phy: BlePhyEngineStorage::from_parts(phy, &placement),
                    direction_finding: DirectionFindingWorkspaceStorage::from_parts(df, df_binding),
                    devices: LeDeviceTable::from_parts(devices, &placement),
                    scanning_rx: scanning_rx.each_mut().map(|storage| {
                        LeRxChain::from_parts(
                            storage,
                            &placement,
                            RxMemoryListClass::Scanning,
                            rx_plan.take().expect("one validated scanning RX graph"),
                        )
                    }),
                    other_rx: other_rx.each_mut().map(|storage| {
                        LeRxChain::from_parts(
                            storage,
                            &placement,
                            RxMemoryListClass::NonScanning,
                            other_plan
                                .take()
                                .expect("one validated non-scanning RX graph"),
                        )
                    }),
                    scheduler: scheduler::SchedulerExecutor::new(),
                    #[cfg(any(target_arch = "riscv32", test))]
                    hardware: scheduler::hardware::SchedulerHardware::new(),
                    scanning_publication: None,
                },
            },
            interrupts,
        ))
    }
}

impl<
    'pool,
    'epoch,
    const A: usize,
    const C: usize,
    const N: usize,
    const S: usize,
    const R: usize,
    const O: usize,
    const Q: usize,
> sealed::Memory for ArenaMemory<'pool, 'epoch, A, C, N, S, R, O, Q>
{
    type Storage = BluetoothArena<A, C, N, S, R, O>;
    fn phy(&self) -> &BlePhyEngineCpuOwned {
        &self.phy
    }
    fn direction_finding(&self) -> &DirectionFindingWorkspaceCpuOwned {
        &self.direction_finding
    }
    fn retirement_ready(&self) -> bool {
        #[cfg(any(target_arch = "riscv32", test))]
        if !self.hardware.is_quiet() {
            return false;
        }
        self.scheduler.retirement_ready()
    }
    fn scheduler_stopped(&self) -> bool {
        self.scheduler.stopped().is_some()
    }
    #[allow(
        unsafe_code,
        clippy::drop_non_drop,
        reason = "rejoin the unique allocation only after controller reset"
    )]
    fn reclaim(self, _reset: &crate::bluetooth::BluetoothControllerReset) -> Placed<Self::Storage> {
        let root = self.root;
        drop(self);
        // SAFETY: root came from the consumed Placed. All field owners and
        // their borrows have ended above; no layout reference escapes them.
        // Verified reset ends every hardware reader. Drop the pinned backing
        // in place before releasing its pin contract, then initialize a new
        // zeroed value in the same allocation. Every arena field is Zeroable.
        // The original placement proof still covers the reunited tuple.
        unsafe {
            core::ptr::drop_in_place(root.as_ptr());
            core::ptr::write_bytes(root.as_ptr(), 0, 1);
            Placed::from_retained(&mut *root.as_ptr())
        }
    }
}
impl<
    'pool,
    'epoch,
    const A: usize,
    const C: usize,
    const N: usize,
    const S: usize,
    const R: usize,
    const O: usize,
    const Q: usize,
> ControllerMemory for ArenaMemory<'pool, 'epoch, A, C, N, S, R, O, Q>
{
}

#[cfg(test)]
impl<
    'pool,
    'epoch,
    const A: usize,
    const C: usize,
    const N: usize,
    const S: usize,
    const R: usize,
    const O: usize,
    const Q: usize,
> ArenaMemory<'pool, 'epoch, A, C, N, S, R, O, Q>
{
    pub(super) fn root_address_for_test(&self) -> usize {
        self.root.as_ptr().addr()
    }
}
