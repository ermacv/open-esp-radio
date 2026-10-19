//! Chip-neutral shared modem clock reference-count planner.
//!
//! Ported from ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/esp_hw_support/modem/modem_clock.c` (Copyright Espressif
//! Systems (Shanghai) Co Ltd, Apache-2.0) and modified by Vladislav
//! Ermachenkov; see `NOTICE`.
//!
//! Every radio module (Wi-Fi, Bluetooth, IEEE 802.15.4, PHY, coexistence,
//! ...) requests a fixed set of modem clock dependencies. Each dependency has
//! one software refcount. Both acquisition and release visit a module's
//! dependencies from the lowest vendor dependency bit to the highest. A
//! physical acquire edge is emitted only for a refcount transition from zero
//! to one; a physical release edge only for a transition from one to zero.
//! Two vendor exceptions are reproduced exactly:
//!
//! - a dependency that is not [refcounted](ModemClockDependency::refcounted)
//!   emits its edge at every module acquire and release, because the vendor
//!   keeps that refcount in another owner (the analog-I2C master clock);
//! - while Wi-Fi is initialized, the dependencies
//!   [retained](ModemClockDependency::retained_while_wifi_initialized) for it
//!   keep their hardware enabled: their one-to-zero transitions emit no
//!   physical edge, and a later zero-to-one transition re-enables them.
//!
//! Source: ESP-IDF `components/esp_hw_support/modem/modem_clock.c`
//! (`config_wrapper`, `modem_clock_device_control`,
//! `modem_clock_configure_wifi_status`), shared by every chip. Each chip
//! supplies its device order and module dependency sets, from its
//! `modem/port/<chip>/modem_clock_impl.c` and the device order of
//! `modem/include/modem/modem_clock_impl.h`, as a [`ModemClockDependency`]
//! table and performs each edge in [`execute_acquire`] and
//! [`execute_release`].
//!
//! This crate performs no MMIO and issues no PLL, clock, PHY, or
//! radio-readiness proof. A committed transaction is software accounting over
//! edges the chip's executor reported performed.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

use core::{
    fmt,
    marker::PhantomData,
    sync::atomic::{AtomicU32, Ordering},
};

/// Most dependencies one chip table may name.
pub const DEPENDENCY_CAPACITY: usize = 32;
/// Maximum count representable by the reviewed vendor `int16_t` contract.
const MAX_REFCOUNT: u16 = i16::MAX as u16;

/// Internal allocation-free capacity, not a vendor hardware or ABI limit.
const MAX_ACTIVE_LEASES: usize = 16;

/// One chip's modem clock dependencies, in the vendor device order.
pub trait ModemClockDependency: Copy + Eq + fmt::Debug + 'static {
    /// Every dependency, lowest vendor device first; at most
    /// [`DEPENDENCY_CAPACITY`].
    const LOW_BIT_FIRST: &'static [Self];

    /// Position of `self` in [`Self::LOW_BIT_FIRST`].
    fn index(self) -> usize;

    /// Whether this planner keeps the dependency's refcount. Otherwise the
    /// vendor keeps it in another owner and every request emits its edge.
    fn refcounted(self) -> bool;

    /// Whether the hardware stays enabled while Wi-Fi is initialized.
    fn retained_while_wifi_initialized(self) -> bool;
}

/// Whether a chip table satisfies the [`ModemClockDependency`] contract:
/// within capacity, and every `index` equal to its position.
pub fn table_is_consistent<D: ModemClockDependency>() -> bool {
    D::LOW_BIT_FIRST.len() <= DEPENDENCY_CAPACITY
        && D::LOW_BIT_FIRST
            .iter()
            .enumerate()
            .all(|(position, dependency)| dependency.index() == position)
}

/// One module's dependency membership. No raw mask crosses the crate
/// boundary.
pub struct DependencySet<D> {
    mask: u32,
    _dependency: PhantomData<fn() -> D>,
}

impl<D> Clone for DependencySet<D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D> Copy for DependencySet<D> {}

impl<D> PartialEq for DependencySet<D> {
    fn eq(&self, other: &Self) -> bool {
        self.mask == other.mask
    }
}

impl<D> Eq for DependencySet<D> {}

impl<D: ModemClockDependency> fmt::Debug for DependencySet<D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_set()
            .entries(D::LOW_BIT_FIRST.iter().filter(|d| self.contains(**d)))
            .finish()
    }
}

impl<D: ModemClockDependency> DependencySet<D> {
    const EMPTY: Self = Self {
        mask: 0,
        _dependency: PhantomData,
    };

    /// The set of `dependencies`.
    pub fn of(dependencies: &[D]) -> Self {
        let mut mask = 0;
        for dependency in dependencies {
            mask |= bit(*dependency);
        }
        Self {
            mask,
            _dependency: PhantomData,
        }
    }

    pub fn contains(self, dependency: D) -> bool {
        self.mask & bit(dependency) != 0
    }
}

fn bit<D: ModemClockDependency>(dependency: D) -> u32 {
    1 << dependency.index()
}

/// Unique accounting identity transferred by value into one planner epoch.
///
/// ```compile_fail,E0382
/// use oer_espressif_modem_clock::{ModemClockDependency, ModemClockPlanner, ModemClockPlannerIdentity};
/// fn duplicate<D: ModemClockDependency>(identity: ModemClockPlannerIdentity) {
///     let first = ModemClockPlanner::<D>::managed(identity);
///     let second = ModemClockPlanner::<D>::managed(identity);
/// }
/// ```
/// Keys are never reused, even if a planner or one of its leases is abandoned.
pub struct ModemClockPlannerIdentity {
    key: u32,
}

impl ModemClockPlannerIdentity {
    pub fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(1);
        #[expect(
            clippy::expect_used,
            reason = "exhaustion must stop rather than reuse an identity retained by a forgotten lease"
        )]
        let key = NEXT
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |key| {
                key.checked_add(1)
            })
            .expect("the modem clock identity space is exhausted");
        Self { key }
    }
}

impl Default for ModemClockPlannerIdentity {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Baseline {
    ExternallyRetained,
    Managed,
}

struct LeaseSlot<D> {
    generation: u64,
    active: bool,
    dependencies: DependencySet<D>,
}

impl<D> Clone for LeaseSlot<D> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<D> Copy for LeaseSlot<D> {}

impl<D: ModemClockDependency> LeaseSlot<D> {
    const EMPTY: Self = Self {
        generation: 0,
        active: false,
        dependencies: DependencySet::EMPTY,
    };
}

/// Unique software owner of one shared-clock accounting epoch.
///
/// This type is intentionally neither `Clone` nor `Copy`. Counts and slots have
/// no public accessors or mutation API.
#[must_use = "dropping the planner abandons its complete software ownership epoch"]
pub struct ModemClockPlanner<D> {
    identity: ModemClockPlannerIdentity,
    baseline: Baseline,
    wifi_initialized: bool,
    counts: [u16; DEPENDENCY_CAPACITY],
    slots: [LeaseSlot<D>; MAX_ACTIVE_LEASES],
}

impl<D: ModemClockDependency> ModemClockPlanner<D> {
    /// Adopt an externally retained baseline.
    ///
    /// Unknown externally retained counts cannot produce a managed plan.
    pub fn externally_retained(identity: ModemClockPlannerIdentity) -> Self {
        Self::with_baseline(identity, Baseline::ExternallyRetained)
    }

    /// Construct a managed epoch whose clients are the only owners of the
    /// modem gates.
    ///
    /// Counts start at zero, as the vendor's do: a zero-to-one edge enables a
    /// gate whatever its prior state, and a one-to-zero edge disables it.
    pub fn managed(identity: ModemClockPlannerIdentity) -> Self {
        Self::with_baseline(identity, Baseline::Managed)
    }

    fn with_baseline(identity: ModemClockPlannerIdentity, baseline: Baseline) -> Self {
        Self {
            identity,
            baseline,
            wifi_initialized: false,
            counts: [0; DEPENDENCY_CAPACITY],
            slots: [LeaseSlot::EMPTY; MAX_ACTIVE_LEASES],
        }
    }

    /// Record whether Wi-Fi is initialized; while it is, Wi-Fi clock
    /// dependencies keep their hardware enabled at a one-to-zero transition.
    pub fn set_wifi_initialized(&mut self, initialized: bool) {
        self.wifi_initialized = initialized;
    }

    /// Prepare acquisition of one module's exact vendor dependency set.
    ///
    /// Preparation is transactional: counts and lease slots remain unchanged
    /// until all emitted physical edges are performed and the transaction
    /// commits.
    #[allow(
        clippy::result_large_err,
        reason = "allocation-free failure retains the exact planner owner"
    )]
    pub fn prepare_acquire(
        self,
        dependencies: DependencySet<D>,
    ) -> Result<PreparedModemClockAcquire<D>, ModemClockAcquirePreparationFailure<D>> {
        if self.baseline != Baseline::Managed {
            return Err(ModemClockAcquirePreparationFailure {
                planner: self,
                error: ModemClockAcquirePreparationError::UnknownBaseline,
            });
        }

        for &dependency in D::LOW_BIT_FIRST {
            if dependencies.contains(dependency) {
                let Some(next_count) = self.counts[dependency.index()].checked_add(1) else {
                    return Err(ModemClockAcquirePreparationFailure {
                        planner: self,
                        error: ModemClockAcquirePreparationError::RefcountOverflow(dependency),
                    });
                };
                if next_count > MAX_REFCOUNT {
                    return Err(ModemClockAcquirePreparationFailure {
                        planner: self,
                        error: ModemClockAcquirePreparationError::RefcountOverflow(dependency),
                    });
                }
            }
        }

        let Some(slot) = self.slots.iter().position(|slot| !slot.active) else {
            return Err(ModemClockAcquirePreparationFailure {
                planner: self,
                error: ModemClockAcquirePreparationError::LeaseCapacityReached,
            });
        };
        let Some(generation) = self.slots[slot].generation.checked_add(1) else {
            return Err(ModemClockAcquirePreparationFailure {
                planner: self,
                error: ModemClockAcquirePreparationError::LeaseGenerationExhausted,
            });
        };

        Ok(PreparedModemClockAcquire {
            planner: self,
            dependencies,
            slot: slot as u8,
            generation,
            next_dependency: 0,
            completed_edges: 0,
        })
    }

    /// Validate an opaque lease and prepare its complete release transaction.
    ///
    /// Every validation failure returns both unchanged opaque owners. An unknown
    /// baseline is rejected before any lease interpretation or count change.
    #[allow(
        clippy::result_large_err,
        reason = "allocation-free failure retains the exact planner and lease owners"
    )]
    pub fn prepare_release(
        self,
        lease: ModemClockLease<D>,
    ) -> Result<PreparedModemClockRelease<D>, ModemClockReleasePreparationFailure<D>> {
        let error = if self.baseline != Baseline::Managed {
            Some(ModemClockReleasePreparationError::UnknownBaseline)
        } else if self.identity.key != lease.identity {
            Some(ModemClockReleasePreparationError::CrossManagerLease)
        } else if usize::from(lease.slot) >= MAX_ACTIVE_LEASES {
            Some(ModemClockReleasePreparationError::InvalidLeaseSlot)
        } else {
            let slot = self.slots[usize::from(lease.slot)];
            if slot.generation != lease.generation {
                Some(ModemClockReleasePreparationError::StaleLease)
            } else if !slot.active {
                Some(ModemClockReleasePreparationError::DuplicateRelease)
            } else if slot.dependencies != lease.dependencies {
                Some(ModemClockReleasePreparationError::LeaseRecordMismatch)
            } else {
                D::LOW_BIT_FIRST
                    .iter()
                    .find(|dependency| {
                        lease.dependencies.contains(**dependency)
                            && self.counts[dependency.index()] == 0
                    })
                    .map(|dependency| {
                        ModemClockReleasePreparationError::RefcountUnderflow(*dependency)
                    })
            }
        };

        if let Some(error) = error {
            return Err(ModemClockReleasePreparationFailure {
                planner: self,
                lease,
                error,
            });
        }

        Ok(PreparedModemClockRelease {
            planner: self,
            lease,
            next_dependency: 0,
            completed_edges: 0,
        })
    }
}

/// Error found before an acquire transaction exposes any physical edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModemClockAcquirePreparationError<D> {
    UnknownBaseline,
    RefcountOverflow(D),
    LeaseCapacityReached,
    LeaseGenerationExhausted,
}

/// Failed acquire preparation retaining the unchanged planner.
#[must_use = "the failed preparation still owns the complete planner"]
pub struct ModemClockAcquirePreparationFailure<D> {
    planner: ModemClockPlanner<D>,
    error: ModemClockAcquirePreparationError<D>,
}

impl<D: ModemClockDependency> fmt::Debug for ModemClockAcquirePreparationFailure<D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ModemClockAcquirePreparationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl<D: ModemClockDependency> ModemClockAcquirePreparationFailure<D> {
    pub const fn error(&self) -> ModemClockAcquirePreparationError<D> {
        self.error
    }

    /// Recover the unchanged planner; preparation has exposed no edge.
    pub fn into_planner(self) -> ModemClockPlanner<D> {
        self.planner
    }
}

/// Prepared but uncommitted acquire transaction.
///
/// No count or slot is modified while this owner exists.
#[must_use = "an acquire transaction must be executed or remain abandoned"]
pub struct PreparedModemClockAcquire<D> {
    planner: ModemClockPlanner<D>,
    dependencies: DependencySet<D>,
    slot: u8,
    generation: u64,
    next_dependency: u8,
    completed_edges: u8,
}

/// Next ownership state of an acquire transaction.
enum ModemClockAcquireStep<D> {
    Physical(PendingModemClockAcquireEdge<D>),
    CommitReady(ModemClockAcquireCommitReady<D>),
}

impl<D: ModemClockDependency> PreparedModemClockAcquire<D> {
    /// Move to the next boundary edge or the explicit commit state.
    fn advance(mut self) -> ModemClockAcquireStep<D> {
        while usize::from(self.next_dependency) < D::LOW_BIT_FIRST.len() {
            let dependency = D::LOW_BIT_FIRST[usize::from(self.next_dependency)];
            self.next_dependency += 1;
            if self.dependencies.contains(dependency)
                && (!dependency.refcounted() || self.planner.counts[dependency.index()] == 0)
            {
                return ModemClockAcquireStep::Physical(PendingModemClockAcquireEdge {
                    transaction: self,
                    dependency,
                });
            }
        }

        ModemClockAcquireStep::CommitReady(ModemClockAcquireCommitReady { transaction: self })
    }
}

/// Acquire transaction owner while one physical edge is outstanding.
///
/// Failure becomes an opaque poisoned owner, so partial physical execution
/// cannot be represented as a clean rollback.
struct PendingModemClockAcquireEdge<D> {
    transaction: PreparedModemClockAcquire<D>,
    dependency: D,
}

impl<D: ModemClockDependency> PendingModemClockAcquireEdge<D> {
    /// Record the edge performed and continue the same transaction.
    fn complete(mut self) -> PreparedModemClockAcquire<D> {
        self.transaction.completed_edges += 1;
        self.transaction
    }

    /// Retain the complete owner after an uncertain or failed physical edge.
    fn fail(self) -> PoisonedModemClockAcquire<D> {
        PoisonedModemClockAcquire { pending: self }
    }
}

/// Opaque owner after a physical acquire edge did not complete cleanly.
#[must_use = "a poisoned acquire still owns its planner and pending lease"]
pub struct PoisonedModemClockAcquire<D> {
    pending: PendingModemClockAcquireEdge<D>,
}

impl<D: ModemClockDependency> fmt::Debug for PoisonedModemClockAcquire<D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PoisonedModemClockAcquire")
            .field("dependency", &self.dependency())
            .field("completed_edges", &self.completed_edges())
            .finish_non_exhaustive()
    }
}

impl<D: ModemClockDependency> PoisonedModemClockAcquire<D> {
    /// The dependency whose acquire edge failed.
    pub const fn dependency(&self) -> D {
        self.pending.dependency
    }

    pub const fn completed_edges(&self) -> u8 {
        self.pending.transaction.completed_edges
    }
}

#[cfg(test)]
impl<D> PoisonedModemClockAcquire<D> {
    /// Re-expose the same pure-model edge to unit tests. This is not a
    /// hardware retry contract.
    fn reexpose_for_test(self) -> PendingModemClockAcquireEdge<D> {
        self.pending
    }
}

/// Acquire transaction whose required physical edge sequence is complete.
struct ModemClockAcquireCommitReady<D> {
    transaction: PreparedModemClockAcquire<D>,
}

impl<D: ModemClockDependency> ModemClockAcquireCommitReady<D> {
    /// Atomically publish logical counts and the new opaque lease.
    fn commit(mut self) -> (ModemClockPlanner<D>, ModemClockLease<D>) {
        for &dependency in D::LOW_BIT_FIRST {
            if self.transaction.dependencies.contains(dependency) {
                self.transaction.planner.counts[dependency.index()] += 1;
            }
        }

        let slot = &mut self.transaction.planner.slots[usize::from(self.transaction.slot)];
        slot.generation = self.transaction.generation;
        slot.active = true;
        slot.dependencies = self.transaction.dependencies;

        let lease = ModemClockLease {
            identity: self.transaction.planner.identity.key,
            slot: self.transaction.slot,
            generation: self.transaction.generation,
            dependencies: self.transaction.dependencies,
        };
        (self.transaction.planner, lease)
    }
}

/// Opaque ownership of one committed dependency acquisition.
///
/// This type is intentionally neither `Clone` nor `Copy` and exposes no slot,
/// generation, dependency set, or constructor.
#[must_use = "the shared-clock lease must remain owned until an explicit release"]
pub struct ModemClockLease<D> {
    identity: u32,
    slot: u8,
    generation: u64,
    dependencies: DependencySet<D>,
}

/// Error found before a release transaction exposes any physical edge.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModemClockReleasePreparationError<D> {
    UnknownBaseline,
    CrossManagerLease,
    InvalidLeaseSlot,
    StaleLease,
    DuplicateRelease,
    LeaseRecordMismatch,
    RefcountUnderflow(D),
}

/// Failed release preparation retaining the unchanged planner and exact lease.
#[must_use = "both opaque owners remain in this failed release"]
pub struct ModemClockReleasePreparationFailure<D> {
    planner: ModemClockPlanner<D>,
    lease: ModemClockLease<D>,
    error: ModemClockReleasePreparationError<D>,
}

impl<D: ModemClockDependency> fmt::Debug for ModemClockReleasePreparationFailure<D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ModemClockReleasePreparationFailure")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl<D: ModemClockDependency> ModemClockReleasePreparationFailure<D> {
    pub const fn error(&self) -> ModemClockReleasePreparationError<D> {
        self.error
    }

    /// Recover only the two unchanged opaque owners, never counts or IDs.
    pub fn into_owners(self) -> (ModemClockPlanner<D>, ModemClockLease<D>) {
        (self.planner, self.lease)
    }
}

/// Prepared but uncommitted release transaction.
///
/// The lease remains active and counts remain unchanged until the
/// transaction commits.
#[must_use = "a release transaction must be executed or remain abandoned"]
pub struct PreparedModemClockRelease<D> {
    planner: ModemClockPlanner<D>,
    lease: ModemClockLease<D>,
    next_dependency: u8,
    completed_edges: u8,
}

/// Next ownership state of a release transaction.
enum ModemClockReleaseStep<D> {
    Physical(PendingModemClockReleaseEdge<D>),
    CommitReady(ModemClockReleaseCommitReady<D>),
}

impl<D: ModemClockDependency> PreparedModemClockRelease<D> {
    /// Move to the next one-to-zero edge or the explicit commit state.
    fn advance(mut self) -> ModemClockReleaseStep<D> {
        while usize::from(self.next_dependency) < D::LOW_BIT_FIRST.len() {
            let dependency = D::LOW_BIT_FIRST[usize::from(self.next_dependency)];
            self.next_dependency += 1;
            let last = !dependency.refcounted() || self.planner.counts[dependency.index()] == 1;
            let retained =
                self.planner.wifi_initialized && dependency.retained_while_wifi_initialized();
            if self.lease.dependencies.contains(dependency) && last && !retained {
                return ModemClockReleaseStep::Physical(PendingModemClockReleaseEdge {
                    transaction: self,
                    dependency,
                });
            }
        }

        ModemClockReleaseStep::CommitReady(ModemClockReleaseCommitReady { transaction: self })
    }
}

/// Release transaction owner while one physical edge is outstanding.
struct PendingModemClockReleaseEdge<D> {
    transaction: PreparedModemClockRelease<D>,
    dependency: D,
}

impl<D: ModemClockDependency> PendingModemClockReleaseEdge<D> {
    /// Record the edge performed and continue the same transaction.
    fn complete(mut self) -> PreparedModemClockRelease<D> {
        self.transaction.completed_edges += 1;
        self.transaction
    }

    /// Retain the complete owner after an uncertain or failed physical edge.
    fn fail(self) -> PoisonedModemClockRelease<D> {
        PoisonedModemClockRelease { pending: self }
    }
}

/// Opaque owner after a physical release edge did not complete cleanly.
#[must_use = "a poisoned release still owns its planner and exact lease"]
pub struct PoisonedModemClockRelease<D> {
    pending: PendingModemClockReleaseEdge<D>,
}

impl<D: ModemClockDependency> fmt::Debug for PoisonedModemClockRelease<D> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PoisonedModemClockRelease")
            .field("dependency", &self.dependency())
            .field("completed_edges", &self.completed_edges())
            .finish_non_exhaustive()
    }
}

impl<D: ModemClockDependency> PoisonedModemClockRelease<D> {
    /// The dependency whose release edge failed.
    pub const fn dependency(&self) -> D {
        self.pending.dependency
    }

    pub const fn completed_edges(&self) -> u8 {
        self.pending.transaction.completed_edges
    }
}

#[cfg(test)]
impl<D> PoisonedModemClockRelease<D> {
    /// Re-expose the same pure-model edge to unit tests. This is not a
    /// hardware retry contract.
    fn reexpose_for_test(self) -> PendingModemClockReleaseEdge<D> {
        self.pending
    }
}

/// Release transaction whose required physical edge sequence is complete.
struct ModemClockReleaseCommitReady<D> {
    transaction: PreparedModemClockRelease<D>,
}

impl<D: ModemClockDependency> ModemClockReleaseCommitReady<D> {
    /// Atomically decrement counts and retire the exact lease slot.
    fn commit(mut self) -> ModemClockPlanner<D> {
        for &dependency in D::LOW_BIT_FIRST {
            if self.transaction.lease.dependencies.contains(dependency) {
                self.transaction.planner.counts[dependency.index()] -= 1;
            }
        }

        let slot = &mut self.transaction.planner.slots[usize::from(self.transaction.lease.slot)];
        slot.active = false;
        slot.dependencies = DependencySet::EMPTY;
        self.transaction.planner
    }
}

/// Perform every acquire edge of `prepared` in order and commit.
///
/// `perform` applies one dependency's physical enable action; the chip's
/// executor implements it. An edge counts as performed only when `perform`
/// returns `Ok`.
///
/// # Errors
///
/// A failed edge poisons the transaction at that edge: the planner cannot tell
/// a refused request from a partially applied one.
#[allow(
    clippy::result_large_err,
    reason = "the poisoned transaction retains the planner without allocation"
)]
pub fn execute_acquire<D: ModemClockDependency, E>(
    mut prepared: PreparedModemClockAcquire<D>,
    mut perform: impl FnMut(D) -> Result<(), E>,
) -> Result<(ModemClockPlanner<D>, ModemClockLease<D>), PoisonedModemClockAcquire<D>> {
    loop {
        match prepared.advance() {
            ModemClockAcquireStep::Physical(pending) => {
                if perform(pending.dependency).is_err() {
                    return Err(pending.fail());
                }
                prepared = pending.complete();
            }
            ModemClockAcquireStep::CommitReady(ready) => return Ok(ready.commit()),
        }
    }
}

/// Perform every release edge of `prepared` in order and commit.
///
/// `perform` applies one dependency's physical disable action.
///
/// # Errors
///
/// A failed edge poisons the transaction at that edge.
#[allow(
    clippy::result_large_err,
    reason = "the poisoned transaction retains the planner and lease without allocation"
)]
pub fn execute_release<D: ModemClockDependency, E>(
    mut prepared: PreparedModemClockRelease<D>,
    mut perform: impl FnMut(D) -> Result<(), E>,
) -> Result<ModemClockPlanner<D>, PoisonedModemClockRelease<D>> {
    loop {
        match prepared.advance() {
            ModemClockReleaseStep::Physical(pending) => {
                if perform(pending.dependency).is_err() {
                    return Err(pending.fail());
                }
                prepared = pending.complete();
            }
            ModemClockReleaseStep::CommitReady(ready) => return Ok(ready.commit()),
        }
    }
}

#[cfg(test)]
mod tests;
