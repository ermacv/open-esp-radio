//! Retain an owner while its affine name travels through an executor.

use core::{
    cell::UnsafeCell,
    mem::{ManuallyDrop, MaybeUninit},
    ptr,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::UniqueQueueIndex;

/// Construction authority for one array of retained owners.
///
/// # Safety
/// The authority is affine and binds exactly one storage instance to its
/// private name domain. Names must not be usable with another instance,
/// including an instance constructed from another authority of the same
/// Rust type. A sealed interface must keep both the names and the raw
/// storage private and route replies back to their originating instance.
/// Each name also meets [`UniqueQueueIndex`]'s bounds, uniqueness and reuse
/// ordering. A new authority must not recreate names still in flight.
///
/// The domain may carry a short installation brand while `Name` contains
/// only an index. That requires the sealed binding's affine executor
/// endpoints to establish the association; erasing a brand alone is not
/// a proof. Neither a name nor its returned wrapper may reference the
/// retained values or their containing allocation.
///
/// `Stopped` is issued only by the binding, and only after hardware use of
/// every value this store can retain has ended: [`IndexedCustody::finish`]
/// releases every retained value on a borrow of it.
#[allow(unsafe_code, reason = "the binding proves the private name domain")]
pub unsafe trait CustodyDomain<const N: usize> {
    type Name: UniqueQueueIndex<N>;
    /// Proof, issued only by the binding, that hardware use of every value
    /// this store retains has ended. [`IndexedCustody::finish`] borrows it.
    type Stopped;
}

/// What a retained value does when its store is quarantined.
///
/// Quarantine leaks every owner, never dropping it, because hardware may
/// still reach it. It must still erase secret material: erasing a CPU copy
/// never conflicts with a DMA read of a frame.
///
/// # Safety
/// [`IndexedCustody`]'s destructor relies on this method neither releasing,
/// moving nor returning any owner the value holds, and on it not waiting.
#[allow(
    unsafe_code,
    reason = "the quarantine destructor relies on the implementation"
)]
pub unsafe trait Quarantine {
    fn quarantine(&mut self);
}

/// An initialized cell's unique name, returned only after insertion.
///
/// This value contains no pointer to the retained owner and no lifetime
/// borrowed from it. Moving it to an executor does not move that owner.
/// Dropping or forgetting the name does not remove the retained value.
#[must_use]
pub struct RetainedName<K, const N: usize> {
    name: K,
}

#[allow(unsafe_code, reason = "retention preserves the same affine index")]
// SAFETY: the private constructor consumes the unique input name. It is
// returned only after the cell is initialized, and extraction consumes this
// wrapper. No clone, index mutation or independent constructor is exposed.
unsafe impl<K: UniqueQueueIndex<N>, const N: usize> UniqueQueueIndex<N> for RetainedName<K, N> {
    fn queue_index(&self) -> usize {
        self.name.queue_index()
    }
}

/// One retained value per affine place, without an occupancy check or wait.
///
/// [`Self::insert`] initializes the place and produces its only removal
/// name. [`Self::take`] consumes that exact name after it returns through
/// the binding's reply path. The domain, not a runtime lookup or flag,
/// proves that the chosen cell belongs to this storage and is in the
/// required state. The initialization bits account only for the values
/// [`Self::finish`] yields and quarantine visits.
///
/// The ordinary end is explicit: after the binding proves that hardware has
/// stopped, [`Self::finish`] consumes the store and yields every value still
/// retained, to be returned to its pools. Destroying a non-empty store is the
/// quarantine path only: each remaining value is quarantined ([`Quarantine`])
/// and leaked, never dropped, so an owner hardware may still read (an MPDU
/// whose DMA has not ended) keeps the memory and resources it owns.
/// The store does not stop hardware or give an executor access to borrowed
/// context.
pub struct IndexedCustody<T: Quarantine, D: CustodyDomain<N>, const N: usize> {
    values: [UnsafeCell<MaybeUninit<T>>; N],
    initialized: [AtomicBool; N],
    _domain: D,
}

#[allow(unsafe_code, reason = "affine names serialize each retained cell")]
// SAFETY: insertion consumes the unique vacant name, and only the resulting
// RetainedName permits access or removal. A mutable borrow of that name
// excludes other access to its cell. The domain binds names to this instance
// and orders reuse across cores. T: Send permits that ownership transfer;
// D: Sync permits sharing the retained construction authority. Exclusive
// destruction cannot race another method's storage borrow.
unsafe impl<T: Quarantine + Send, D: CustodyDomain<N> + Sync, const N: usize> Sync
    for IndexedCustody<T, D, N>
{
}

impl<T: Quarantine, D: CustodyDomain<N>, const N: usize> IndexedCustody<T, D, N> {
    pub const fn new(domain: D) -> Self {
        Self {
            values: [const { UnsafeCell::new(MaybeUninit::uninit()) }; N],
            initialized: [const { AtomicBool::new(false) }; N],
            _domain: domain,
        }
    }

    /// Retain the entire owner before publishing its name to an executor.
    pub fn insert(&self, name: D::Name, value: T) -> RetainedName<D::Name, N> {
        let index = name.queue_index();
        #[allow(unsafe_code, reason = "the domain owns this vacant bounded cell")]
        // SAFETY: the consumed name proves index < N and exclusive vacant
        // ownership. No RetainedName for the previous value remains. There
        // is no user code between initialization and its accounting bit.
        unsafe {
            (*self.values.get_unchecked(index).get()).write(value);
            self.initialized
                .get_unchecked(index)
                .store(true, Ordering::Relaxed);
        }
        RetainedName { name }
    }

    /// Execute a finite operation on the retained record without moving it.
    /// The closure cannot retain its borrow beyond the mutable name loan.
    pub fn with_mut<R>(
        &self,
        name: &mut RetainedName<D::Name, N>,
        apply: impl FnOnce(&mut T) -> R,
    ) -> R {
        let index = name.queue_index();
        #[allow(unsafe_code, reason = "the affine initialized name owns this cell")]
        // SAFETY: insertion is the only constructor of RetainedName. Domain
        // association proves this exact initialized cell; &mut name prevents
        // another access or removal until the closure's borrow has ended.
        apply(unsafe { (*self.values.get_unchecked(index).get()).assume_init_mut() })
    }

    /// Recover the original name and owner after the reply returns the name.
    pub fn take(&self, name: RetainedName<D::Name, N>) -> (D::Name, T) {
        let index = name.queue_index();
        #[allow(unsafe_code, reason = "consume the sole initialized-cell name")]
        // SAFETY: the domain binds this name to this initialized cell. Moving
        // the owner out and clearing its accounting bit finishes before
        // the original permission can return and authorize another write.
        let value = unsafe {
            let value = (*self.values.get_unchecked(index).get()).assume_init_read();
            self.initialized
                .get_unchecked(index)
                .store(false, Ordering::Relaxed);
            value
        };
        (name.name, value)
    }

    /// The ordinary end: after hardware stopped, consume the store and yield
    /// every value still retained, in place order, to return to its pools.
    pub fn finish(self, _stopped: &D::Stopped) -> Drained<T, N> {
        let this = ManuallyDrop::new(self);
        #[allow(
            unsafe_code,
            reason = "move the cells out of a store whose destructor never runs"
        )]
        // SAFETY: `this` is never dropped and each field is read exactly once.
        // The stop proof ends every hardware use, and consuming the store ends
        // every name's access: no RetainedName can reach the moved cells.
        let (values, initialized, domain) = unsafe {
            (
                ptr::read(&this.values),
                ptr::read(&this.initialized),
                ptr::read(&this._domain),
            )
        };
        drop(domain);
        Drained {
            values: values.map(UnsafeCell::into_inner),
            initialized: initialized.map(AtomicBool::into_inner),
            next: 0,
        }
    }
}

/// Every value a stopped store still retained, yielded by value. Dropping it
/// drops the values not yet taken: hardware use has ended.
pub struct Drained<T, const N: usize> {
    values: [MaybeUninit<T>; N],
    initialized: [bool; N],
    next: usize,
}

impl<T, const N: usize> Iterator for Drained<T, N> {
    type Item = T;

    fn next(&mut self) -> Option<T> {
        let start = self.next;
        for (value, initialized) in self
            .values
            .iter_mut()
            .zip(&mut self.initialized)
            .skip(start)
        {
            self.next += 1;
            if core::mem::take(initialized) {
                #[allow(unsafe_code, reason = "each initialized cell is moved out once")]
                // SAFETY: the bit was set by insert and is cleared here before
                // the move, so this cell is read exactly once.
                return Some(unsafe { value.assume_init_read() });
            }
        }
        None
    }
}

impl<T, const N: usize> Drop for Drained<T, N> {
    fn drop(&mut self) {
        for value in self.by_ref() {
            drop(value);
        }
    }
}

impl<T: Quarantine, D: CustodyDomain<N>, const N: usize> Drop for IndexedCustody<T, D, N> {
    /// The quarantine path: erase each remaining value's secrets in place and
    /// keep it, never dropping an owner hardware may still reach.
    fn drop(&mut self) {
        for (cell, initialized) in self.values.iter_mut().zip(&mut self.initialized) {
            if *initialized.get_mut() {
                #[allow(
                    unsafe_code,
                    reason = "exclusive destruction quarantines retained values"
                )]
                // SAFETY: the accounting bit is set immediately after insert
                // and cleared immediately after take, with no intervening
                // user code. Exclusive destruction excludes cell accesses.
                unsafe { cell.get_mut().assume_init_mut() }.quarantine();
            }
        }
    }
}

#[cfg(test)]
mod tests;
