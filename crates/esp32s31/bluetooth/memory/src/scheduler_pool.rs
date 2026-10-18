//! Fixed pools of role instances whose scheduler items share list zero.
//!
//! Every role kind reserves a fixed number of pinned instances. An instance
//! holds the role's controller-SRAM graph: its scheduler items and the link
//! state, packets and context they reference. The scheduler executor inserts
//! and removes individual items; a role prepares an instance only while none
//! of its items is listed.
//!
//! Item states follow the executor. An item becomes listed when the role
//! submits it and returns to its instance when the executor releases it. An
//! item that the executor could not release safely is withheld: hardware may
//! still reach it, so its instance never returns to the pool.
//!
//! Item memory is reached through [`SchedulerItemSpace`], which joins the
//! pools of every kind for the executor. It performs volatile word accesses
//! only; the ownership contract stays with the pools and the executor.

#![forbid(unsafe_code)]

use core::{marker::PhantomPinned, pin::Pin};

use oer_esp32s31_hal::bluetooth::BluetoothControllerReset;
use oer_esp32s31_hal::types::{
    BluetoothControllerSramAddress, BluetoothControllerSramAddressError,
};
use pin_project::pin_project;
use vcell::VolatileCell;

use crate::{
    scheduler_item::{SchedulerItemCompletionStatus, SchedulerItemHeader},
    sram_link::{
        BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH, BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW,
        ControllerSramLinkAddress,
    },
};

/// Role kinds whose items share scheduler list zero.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerRoleKind {
    DirectTestMode,
    LegacyAdvertising,
    ConnectableAdvertising,
    LegacyScanning,
    PeripheralConnection,
}

impl SchedulerRoleKind {
    const COUNT: usize = 5;

    const fn index(self) -> usize {
        match self {
            Self::DirectTestMode => 0,
            Self::LegacyAdvertising => 1,
            Self::ConnectableAdvertising => 2,
            Self::LegacyScanning => 3,
            Self::PeripheralConnection => 4,
        }
    }
}

/// Product limits that the vendor allocators turn into item numbers.
///
/// Scheduler item word `+0x20[11:0]` carries a number per state machine.
/// Hardware records it in every packet that the item receives, and each role
/// takes the packets with its own number from the shared receive chain. The
/// vendor allocators number advertising instances from zero, connections
/// after one reserved number, then the scanner's items, and place DTM after
/// five private and four further numbers plus one per periodic
/// synchronization.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerAllocationConfig {
    extended_advertising_instances: u16,
    connections: u16,
    periodic_syncs: u8,
}

/// Numbers that one pool may assign.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerAllocationNumbers {
    first: u16,
    count: u16,
}

impl SchedulerAllocationNumbers {
    pub const fn first(self) -> u16 {
        self.first
    }

    pub const fn count(self) -> u16 {
        self.count
    }
}

/// Items of the vendor scanner, numbered one after another.
const SCANNER_NUMBERS: u16 = 3;
/// Numbers between the scanner and DTM that the vendor reserves privately.
const PRIVATE_NUMBERS_BEFORE_DTM: u16 = 5 + 4;
/// Largest number that fits item word `+0x20[11:0]`.
const MAX_ALLOCATION_NUMBER: u32 = 0x0fff;

impl SchedulerAllocationConfig {
    /// Capture the product limits. `None` when the DTM number would not fit
    /// its twelve-bit field.
    pub const fn new(
        extended_advertising_instances: u16,
        connections: u16,
        periodic_syncs: u8,
    ) -> Option<Self> {
        let dtm = extended_advertising_instances as u32
            + 1
            + connections as u32
            + PRIVATE_NUMBERS_BEFORE_DTM as u32
            + periodic_syncs as u32;
        if dtm > MAX_ALLOCATION_NUMBER {
            return None;
        }
        Some(Self {
            extended_advertising_instances,
            connections,
            periodic_syncs,
        })
    }

    /// Advertising instances `first..first + count` of the shared
    /// advertising numbering.
    pub const fn advertising(self, first: u16, count: u16) -> Option<SchedulerAllocationNumbers> {
        if first as u32 + count as u32 > self.extended_advertising_instances as u32 {
            return None;
        }
        Some(SchedulerAllocationNumbers { first, count })
    }

    pub const fn connections(self) -> SchedulerAllocationNumbers {
        SchedulerAllocationNumbers {
            first: self.extended_advertising_instances + 1,
            count: self.connections,
        }
    }

    pub const fn scanning(self) -> SchedulerAllocationNumbers {
        SchedulerAllocationNumbers {
            first: self.extended_advertising_instances + 1 + self.connections,
            count: SCANNER_NUMBERS,
        }
    }

    pub const fn direct_test_mode(self) -> SchedulerAllocationNumbers {
        SchedulerAllocationNumbers {
            first: self.extended_advertising_instances
                + 1
                + self.connections
                + PRIVATE_NUMBERS_BEFORE_DTM
                + self.periodic_syncs as u16,
            count: 1,
        }
    }
}

/// One scheduler item of one role instance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerItemId {
    kind: SchedulerRoleKind,
    instance: u8,
    item: u8,
}

impl SchedulerItemId {
    pub const fn kind(self) -> SchedulerRoleKind {
        self.kind
    }

    /// Index of the instance within its pool.
    pub const fn instance(self) -> usize {
        self.instance as usize
    }

    /// Index of the item within its instance.
    pub const fn item(self) -> usize {
        self.item as usize
    }
}

/// Exclusive handle to one acquired role instance.
///
/// The handle is affine: releasing the instance consumes it, so no stale
/// handle can name a reused instance.
#[derive(Debug, Eq, PartialEq)]
#[must_use = "an acquired instance must be released to its pool"]
pub struct SchedulerRoleInstance {
    kind: SchedulerRoleKind,
    index: u8,
}

impl SchedulerRoleInstance {
    pub const fn kind(&self) -> SchedulerRoleKind {
        self.kind
    }

    /// Index of the instance within its pool.
    pub const fn index(&self) -> usize {
        self.index as usize
    }

    /// Identity of one item of this instance.
    pub const fn item(&self, item: usize) -> SchedulerItemId {
        SchedulerItemId {
            kind: self.kind,
            instance: self.index,
            item: item as u8,
        }
    }
}

pub(crate) mod sealed {
    pub trait Sealed {}
}

/// Controller-SRAM graph of one role instance.
///
/// Implemented only by the role storages of this crate.
pub trait SchedulerRoleStorage: sealed::Sealed + Sized + 'static {
    #[doc(hidden)]
    const KIND: SchedulerRoleKind;
    /// Scheduler items of one instance.
    #[doc(hidden)]
    const ITEMS: usize;
    /// Allocation numbers that one instance uses.
    #[doc(hidden)]
    const NUMBERS: usize;
    #[doc(hidden)]
    const NEW: Self;
    /// Addresses of the graph parts, derived once when the pool binds.
    #[doc(hidden)]
    type Binding: Copy;
    /// CPU-side role state kept beside the graph.
    #[doc(hidden)]
    type State: Copy;
    #[doc(hidden)]
    const INITIAL_STATE: Self::State;

    #[doc(hidden)]
    fn bind(base: u32, first_number: u16) -> Result<Self::Binding, SchedulerPoolBindError>;
    #[doc(hidden)]
    fn item_words(&self, item: usize) -> &[VolatileCell<u32>];
    #[doc(hidden)]
    fn item_link(binding: &Self::Binding, item: usize) -> ControllerSramLinkAddress;
    /// Return the graph to its allocation-time image and report the
    /// matching role state.
    #[doc(hidden)]
    fn reinitialize(&mut self, binding: &Self::Binding) -> Self::State;
    /// Whether the role state holds a prepared event for `item`.
    #[doc(hidden)]
    fn admits(state: &Self::State, item: usize) -> bool;
}

/// Why a pool cannot bind its static storage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerPoolBindError {
    /// The storage address does not fit the controller address width.
    AddressWidth,
    InvalidBase(BluetoothControllerSramAddressError),
    ExtentOutsidePhysicalSram,
    /// A graph part would compress to the empty link.
    ZeroCompressedLink,
    /// The pool holds more instances than an item identity can name.
    TooManyInstances,
    /// The pool holds more instances than its allocation numbers admit.
    AllocationNumbers,
}

/// Pinned storage of every instance of one pool.
#[pin_project]
#[repr(C)]
pub struct SchedulerRolePoolStorage<S, const N: usize> {
    instances: [S; N],
    #[pin]
    _pin: PhantomPinned,
}

impl<S: SchedulerRoleStorage, const N: usize> SchedulerRolePoolStorage<S, N> {
    pub const fn new() -> Self {
        Self {
            instances: [const { S::NEW }; N],
            _pin: PhantomPinned,
        }
    }
}

impl<S: SchedulerRoleStorage, const N: usize> Default for SchedulerRolePoolStorage<S, N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Synthetic controller-SRAM base used only by native ownership models.
#[cfg(not(target_arch = "riscv32"))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SchedulerPoolModelAddress(BluetoothControllerSramAddress);

#[cfg(not(target_arch = "riscv32"))]
impl SchedulerPoolModelAddress {
    pub const fn new(address: u32) -> Result<Self, BluetoothControllerSramAddressError> {
        match BluetoothControllerSramAddress::new(address) {
            Ok(address) => Ok(Self(address)),
            Err(error) => Err(error),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InstanceState {
    Free,
    Acquired { listed: u8, withheld: bool },
}

/// Why an instance or item transition was refused. Nothing changed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerPoolError {
    /// The handle or item names another pool.
    ForeignKind,
    /// The item index is outside the instance.
    NoSuchItem,
    /// The item is already listed.
    Listed,
    /// The item is not listed.
    NotListed,
    /// An item of the instance is listed or withheld.
    InstanceBusy,
    /// The role has not prepared an event for the item.
    NotPrepared,
}

/// Instance refused on release; the handle is returned.
#[derive(Debug)]
pub struct SchedulerRoleReleaseFailure {
    pub instance: SchedulerRoleInstance,
    pub error: SchedulerPoolError,
}

/// Fixed pool of role instances of one kind.
pub struct SchedulerRolePool<S: SchedulerRoleStorage, const N: usize> {
    storage: Pin<&'static mut SchedulerRolePoolStorage<S, N>>,
    bindings: [S::Binding; N],
    states: [InstanceState; N],
    role_states: [S::State; N],
}

impl<S: SchedulerRoleStorage, const N: usize> SchedulerRolePool<S, N> {
    /// Bind static storage at its linked address.
    #[cfg(target_arch = "riscv32")]
    pub fn bind(
        storage: &'static mut SchedulerRolePoolStorage<S, N>,
        numbers: SchedulerAllocationNumbers,
    ) -> Result<Self, SchedulerPoolBindError> {
        let base = u32::try_from(core::ptr::addr_of!(*storage).addr())
            .map_err(|_| SchedulerPoolBindError::AddressWidth)?;
        Self::bind_at(storage, base, numbers)
    }

    /// Bind static storage at a synthetic controller address.
    #[cfg(not(target_arch = "riscv32"))]
    pub fn bind_model(
        storage: &'static mut SchedulerRolePoolStorage<S, N>,
        base: SchedulerPoolModelAddress,
        numbers: SchedulerAllocationNumbers,
    ) -> Result<Self, SchedulerPoolBindError> {
        Self::bind_at(storage, base.0.address(), numbers)
    }

    fn bind_at(
        storage: &'static mut SchedulerRolePoolStorage<S, N>,
        base: u32,
        numbers: SchedulerAllocationNumbers,
    ) -> Result<Self, SchedulerPoolBindError> {
        if N > usize::from(u8::MAX) || S::ITEMS > 8 {
            return Err(SchedulerPoolBindError::TooManyInstances);
        }
        if N * S::NUMBERS > usize::from(numbers.count) {
            return Err(SchedulerPoolBindError::AllocationNumbers);
        }
        BluetoothControllerSramAddress::new(base).map_err(SchedulerPoolBindError::InvalidBase)?;
        let bytes = u32::try_from(core::mem::size_of::<SchedulerRolePoolStorage<S, N>>())
            .map_err(|_| SchedulerPoolBindError::ExtentOutsidePhysicalSram)?;
        if base < BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_LOW
            || bytes > BLUETOOTH_CONTROLLER_PHYSICAL_SRAM_HIGH.saturating_sub(base)
        {
            return Err(SchedulerPoolBindError::ExtentOutsidePhysicalSram);
        }
        let stride = core::mem::size_of::<S>() as u32;
        let mut bindings = [None; N];
        for (instance, binding) in bindings.iter_mut().enumerate() {
            let first_number = numbers.first + (instance * S::NUMBERS) as u16;
            *binding = Some(S::bind(base + stride * instance as u32, first_number)?);
        }
        let mut pool = Self {
            storage: Pin::static_mut(storage),
            bindings: bindings.map(|binding| binding.expect("every instance is bound")),
            states: [InstanceState::Free; N],
            role_states: [S::INITIAL_STATE; N],
        };
        for instance in 0..N {
            pool.reinitialize(instance);
        }
        Ok(pool)
    }

    /// Free every instance and return its graph to the allocation-time image
    /// after the Controller that followed it was reset, including instances
    /// whose items were withheld.
    pub fn reset_after_controller_reset(&mut self, _reset: &BluetoothControllerReset) {
        for instance in 0..N {
            self.reinitialize(instance);
            self.states[instance] = InstanceState::Free;
        }
    }

    /// Acquire a free instance.
    pub fn acquire(&mut self) -> Option<SchedulerRoleInstance> {
        let index = self
            .states
            .iter()
            .position(|state| *state == InstanceState::Free)?;
        self.states[index] = InstanceState::Acquired {
            listed: 0,
            withheld: false,
        };
        Some(SchedulerRoleInstance {
            kind: S::KIND,
            index: index as u8,
        })
    }

    /// Return an instance whose items are all back from the executor. Its
    /// graph returns to the allocation-time image.
    pub fn release(
        &mut self,
        instance: SchedulerRoleInstance,
    ) -> Result<(), SchedulerRoleReleaseFailure> {
        if let Err(error) = self.quiescent(&instance) {
            return Err(SchedulerRoleReleaseFailure { instance, error });
        }
        self.reinitialize(instance.index());
        self.states[instance.index()] = InstanceState::Free;
        Ok(())
    }
    /// Mark an item listed before the executor inserts it, after clearing
    /// its software completion-queue link.
    pub fn submit(
        &mut self,
        instance: &SchedulerRoleInstance,
        item: usize,
    ) -> Result<SchedulerItemId, SchedulerPoolError> {
        self.check_kind(instance.kind)?;
        if item >= S::ITEMS {
            return Err(SchedulerPoolError::NoSuchItem);
        }
        let index = instance.index();
        let InstanceState::Acquired { listed, withheld } = self.states[index] else {
            unreachable!("an instance handle names an acquired instance");
        };
        if withheld {
            return Err(SchedulerPoolError::InstanceBusy);
        }
        if listed & (1 << item) != 0 {
            return Err(SchedulerPoolError::Listed);
        }
        if !S::admits(&self.role_states[index], item) {
            return Err(SchedulerPoolError::NotPrepared);
        }
        let header = SchedulerItemHeader::new(self.graph(index).item_words(item));
        header.set_completion_link(0);
        self.states[index] = InstanceState::Acquired {
            listed: listed | 1 << item,
            withheld,
        };
        Ok(instance.item(item))
    }

    /// Return an item that the executor released.
    pub fn retire(&mut self, id: SchedulerItemId) -> Result<(), SchedulerPoolError> {
        let listed = self.listed_mut(id)?;
        *listed &= !(1 << id.item);
        Ok(())
    }

    /// Keep an item that hardware may still reach. Its instance is never
    /// released again.
    pub fn withhold(&mut self, id: SchedulerItemId) -> Result<(), SchedulerPoolError> {
        self.listed_mut(id)?;
        let InstanceState::Acquired { withheld, .. } = &mut self.states[id.instance()] else {
            unreachable!("a listed item belongs to an acquired instance");
        };
        *withheld = true;
        Ok(())
    }

    /// Whether the item is listed.
    pub fn is_listed(&self, id: SchedulerItemId) -> bool {
        id.kind == S::KIND
            && id.instance() < N
            && matches!(
                self.states[id.instance()],
                InstanceState::Acquired { listed, .. } if listed & (1 << id.item) != 0
            )
    }

    /// CPU access to a quiescent instance.
    pub(crate) fn cpu(
        &mut self,
        instance: &SchedulerRoleInstance,
    ) -> Result<SchedulerRoleCpu<'_, S>, SchedulerPoolError> {
        self.quiescent(instance)?;
        let index = instance.index();
        Ok(SchedulerRoleCpu {
            graph: &mut self.storage.as_mut().project().instances[index],
            binding: &self.bindings[index],
            state: &mut self.role_states[index],
        })
    }

    /// Read access to an instance for observations that hardware tolerates
    /// while items are listed.
    pub(crate) fn shared(
        &self,
        instance: &SchedulerRoleInstance,
    ) -> Result<(&S, &S::Binding, &S::State), SchedulerPoolError> {
        self.check_kind(instance.kind)?;
        let index = instance.index();
        Ok((
            self.graph(index),
            &self.bindings[index],
            &self.role_states[index],
        ))
    }

    fn quiescent(&self, instance: &SchedulerRoleInstance) -> Result<(), SchedulerPoolError> {
        self.check_kind(instance.kind)?;
        match self.states[instance.index()] {
            InstanceState::Acquired {
                listed: 0,
                withheld: false,
            } => Ok(()),
            InstanceState::Acquired { .. } => Err(SchedulerPoolError::InstanceBusy),
            InstanceState::Free => unreachable!("an instance handle names an acquired instance"),
        }
    }

    fn listed_mut(&mut self, id: SchedulerItemId) -> Result<&mut u8, SchedulerPoolError> {
        self.check_kind(id.kind)?;
        if id.instance() >= N || id.item() >= S::ITEMS {
            return Err(SchedulerPoolError::NoSuchItem);
        }
        match &mut self.states[id.instance()] {
            InstanceState::Acquired { listed, .. } if *listed & (1 << id.item) != 0 => Ok(listed),
            _ => Err(SchedulerPoolError::NotListed),
        }
    }

    fn check_kind(&self, kind: SchedulerRoleKind) -> Result<(), SchedulerPoolError> {
        if kind == S::KIND {
            Ok(())
        } else {
            Err(SchedulerPoolError::ForeignKind)
        }
    }
    fn reinitialize(&mut self, index: usize) {
        let binding = self.bindings[index];
        self.role_states[index] =
            self.storage.as_mut().project().instances[index].reinitialize(&binding);
    }

    fn graph(&self, index: usize) -> &S {
        &self.storage.as_ref().get_ref().instances[index]
    }
}

/// CPU view of one quiescent instance, used by the role codecs.
pub(crate) struct SchedulerRoleCpu<'pool, S: SchedulerRoleStorage> {
    pub(crate) graph: &'pool mut S,
    pub(crate) binding: &'pool S::Binding,
    pub(crate) state: &'pool mut S::State,
}

/// Item memory of one pool, as seen by the executor.
pub trait SchedulerItemSource: sealed::Sealed {
    #[doc(hidden)]
    fn kind(&self) -> SchedulerRoleKind;
    #[doc(hidden)]
    fn item_link(&self, id: SchedulerItemId) -> ControllerSramLinkAddress;
    #[doc(hidden)]
    fn item_words(&self, id: SchedulerItemId) -> &[VolatileCell<u32>];
    #[doc(hidden)]
    fn listed_item(&self, link: ControllerSramLinkAddress) -> Option<SchedulerItemId>;
}

impl<S: SchedulerRoleStorage, const N: usize> sealed::Sealed for SchedulerRolePool<S, N> {}

impl<S: SchedulerRoleStorage, const N: usize> SchedulerItemSource for SchedulerRolePool<S, N> {
    fn kind(&self) -> SchedulerRoleKind {
        S::KIND
    }

    fn item_link(&self, id: SchedulerItemId) -> ControllerSramLinkAddress {
        S::item_link(&self.bindings[id.instance()], id.item())
    }

    fn item_words(&self, id: SchedulerItemId) -> &[VolatileCell<u32>] {
        debug_assert!(
            self.is_listed(id),
            "the executor addresses only listed items"
        );
        self.graph(id.instance()).item_words(id.item())
    }

    fn listed_item(&self, link: ControllerSramLinkAddress) -> Option<SchedulerItemId> {
        (0..N)
            .flat_map(|instance| {
                (0..S::ITEMS).map(move |item| SchedulerItemId {
                    kind: S::KIND,
                    instance: instance as u8,
                    item: item as u8,
                })
            })
            .find(|id| self.is_listed(*id) && self.item_link(*id) == link)
    }
}

/// The item memory of every pool that shares list zero.
#[derive(Clone, Copy)]
pub struct SchedulerItemSpace<'pools> {
    sources: [Option<&'pools dyn SchedulerItemSource>; SchedulerRoleKind::COUNT],
}

impl<'pools> SchedulerItemSpace<'pools> {
    pub const fn new() -> Self {
        Self {
            sources: [None; SchedulerRoleKind::COUNT],
        }
    }

    /// Add the pool of one kind.
    pub fn with(mut self, pool: &'pools dyn SchedulerItemSource) -> Self {
        self.sources[pool.kind().index()] = Some(pool);
        self
    }

    fn source(&self, id: SchedulerItemId) -> &'pools dyn SchedulerItemSource {
        self.sources[id.kind.index()].expect("the space holds the pool of every listed item")
    }

    fn header(&self, id: SchedulerItemId) -> SchedulerItemHeader<'pools, [VolatileCell<u32>]> {
        SchedulerItemHeader::new(self.source(id).item_words(id))
    }

    /// Controller link of the item.
    pub fn link(&self, id: SchedulerItemId) -> ControllerSramLinkAddress {
        self.source(id).item_link(id)
    }

    /// The submitted item whose controller link is `link`.
    ///
    /// Pool storage is bound for `'static`, so a link found here stays valid
    /// memory for as long as hardware may follow it.
    pub fn listed_item(&self, link: ControllerSramLinkAddress) -> Option<SchedulerItemId> {
        self.sources
            .iter()
            .flatten()
            .find_map(|source| source.listed_item(link))
    }

    /// Prepare a listed item for insertion; see the scheduler item header.
    pub fn prepare_for_list(
        &self,
        id: SchedulerItemId,
        previous: Option<ControllerSramLinkAddress>,
        next: Option<ControllerSramLinkAddress>,
    ) {
        self.header(id).prepare_for_list(previous, next);
    }

    pub fn set_next(&self, id: SchedulerItemId, next: Option<ControllerSramLinkAddress>) {
        self.header(id).link_hardware_next(next);
    }

    pub fn set_previous(&self, id: SchedulerItemId, previous: Option<ControllerSramLinkAddress>) {
        self.header(id).link_previous(previous);
    }

    /// Recorded status, or `None` while hardware has not executed the item.
    pub fn completion_status(&self, id: SchedulerItemId) -> Option<SchedulerItemCompletionStatus> {
        self.header(id).completion_status()
    }

    /// Record a status as hardware does when it executes a listed item.
    #[cfg(test)]
    #[doc(hidden)]
    pub fn record_status_for_validation(&self, id: SchedulerItemId, status: u32) {
        self.header(id).set_status(status);
    }

    /// Mark a listed item deleted before it leaves the list.
    pub fn mark_deleted(&self, id: SchedulerItemId) {
        self.header(id).mark_deleted();
    }
}

impl Default for SchedulerItemSpace<'_> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
