//! Audited bounded SPSC ownership transfer.
//!
//! The queue moves affine values between one producer and one consumer. It is
//! intentionally executor-agnostic: higher layers may wrap its non-blocking
//! endpoints in their own wake and telemetry policy without reopening unsafe
//! slot access.

mod indexed;
mod local;
pub use indexed::{
    AffineIndexedSpscQueue, AffineIndexedSpscReceiver, AffineIndexedSpscSender, UniqueQueueIndex,
};
pub use local::{LocalSpscReceiver, LocalSpscSender};

use core::{
    cell::{Cell, UnsafeCell},
    marker::PhantomData,
    mem::MaybeUninit,
    sync::atomic::{AtomicUsize, Ordering},
};

mod cursor_sealed {
    pub trait Sealed {}
}

/// Sealed cursor storage layouts for a bounded SPSC queue.
/// Existing cross-core queues use `CachedSpscCursor`; same-core priority
/// handoffs may use `CompactSpscCursor` without cache-line padding.
pub trait SpscCursor: cursor_sealed::Sealed + Send + Sync {
    #[doc(hidden)]
    const NEW: Self;
    #[doc(hidden)]
    fn counter(&self) -> &AtomicUsize;
    #[doc(hidden)]
    fn counter_mut(&mut self) -> &mut AtomicUsize;
}

/// One cache line per cursor, the default queue layout.
#[repr(align(64))]
pub struct CachedSpscCursor(AtomicUsize);
/// Unpadded atomic cursor for a same-core priority handoff.
pub struct CompactSpscCursor(AtomicUsize);

macro_rules! cursor {
    ($layout:ty) => {
        impl cursor_sealed::Sealed for $layout {}
        impl SpscCursor for $layout {
            const NEW: Self = Self(AtomicUsize::new(0));
            #[inline(always)]
            fn counter(&self) -> &AtomicUsize {
                &self.0
            }
            fn counter_mut(&mut self) -> &mut AtomicUsize {
                &mut self.0
            }
        }
    };
}
cursor!(CachedSpscCursor);
cursor!(CompactSpscCursor);

/// Full value returned when a bounded SPSC publication finds no free slot.
#[derive(Debug)]
pub struct AffineSpscTrySendError<T>(pub T);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AffineSpscTryReceiveError {
    Empty,
}

/// Storage for one bounded single-producer/single-consumer stream.
///
/// Endpoint uniqueness is proved by the exclusive storage borrow, including
/// across lifecycle transitions. Move the receiver itself to the next phase:
/// a producer cannot manufacture another receiver.
///
/// ```compile_fail,E0499
/// use oer_memory::AffineSpscQueue;
/// let mut queue = AffineSpscQueue::<u8, 2>::new();
/// let (first, consumer) = queue.split();
/// let second = queue.split();
/// first.try_send(1).unwrap();
/// ```
///
/// Capacity checks run at compile time even in an ordinary function call.
///
/// ```compile_fail,E0080
/// use oer_memory::AffineSpscQueue;
/// let queue = AffineSpscQueue::<u8, 0>::new();
/// ```
///
/// ```compile_fail,E0080
/// use oer_memory::AffineSpscQueue;
/// let queue = AffineSpscQueue::<(), { usize::MAX / 2 + 1 }>::new();
/// ```
pub struct AffineSpscQueue<T, const DEPTH: usize, C: SpscCursor = CachedSpscCursor> {
    values: [UnsafeCell<MaybeUninit<T>>; DEPTH],
    producer: C,
    consumer: C,
}

#[allow(
    unsafe_code,
    reason = "bounded SPSC cursor ownership serializes every value slot"
)]
// SAFETY: each slot has exactly one producer context before its Release publication
// and exactly one consumer after the matching Acquire load. The consumer's
// Release cursor prevents reuse until the affine value has moved out.
// Short local endpoint loans suspend the original endpoint. A local producer
// loan is copyable only within its context and offers synchronous publication,
// with no reservation that could overlap another local copy's publication.
unsafe impl<T: Send, const DEPTH: usize, C: SpscCursor> Sync for AffineSpscQueue<T, DEPTH, C> {}

impl<T, const DEPTH: usize> AffineSpscQueue<T, DEPTH> {
    /// Construct an empty cached queue directly in its final storage.
    /// Only cursor words are initialized; no queued payload exists yet.
    #[inline]
    #[allow(
        unsafe_code,
        reason = "the audited placement macro initializes every queue field"
    )]
    pub fn init_in_place(storage: &mut MaybeUninit<Self>) -> &mut Self {
        Self::check_capacity();
        crate::init_in_place!(
            storage,
            Self {
                values: [const { UnsafeCell::new(MaybeUninit::uninit()) }; DEPTH],
                producer: CachedSpscCursor(AtomicUsize::new(0)),
                consumer: CachedSpscCursor(AtomicUsize::new(0)),
            }
        )
    }
}

impl<T, const DEPTH: usize, C: SpscCursor> AffineSpscQueue<T, DEPTH, C> {
    const fn check_capacity() {
        const {
            assert!(DEPTH != 0, "affine SPSC queue must not be empty");
            assert!(
                DEPTH <= usize::MAX / 2,
                "affine SPSC cursor domain must fit usize"
            );
        }
    }

    pub const fn new() -> Self {
        Self::check_capacity();
        Self {
            values: [const { UnsafeCell::new(MaybeUninit::uninit()) }; DEPTH],
            producer: C::NEW,
            consumer: C::NEW,
        }
    }

    /// Exclusively borrow the storage to grant its sole endpoint pair.
    /// Both endpoints retain this borrow: another split cannot overlap them.
    /// Values left by an earlier pair stay owned by the queue and are received
    /// by the next consumer in their original order.
    pub fn split(
        &mut self,
    ) -> (
        AffineSpscSender<'_, T, DEPTH, C>,
        AffineSpscReceiver<'_, T, DEPTH, C>,
    ) {
        (
            AffineSpscSender {
                queue: self,
                _not_sync: PhantomData,
            },
            AffineSpscReceiver {
                queue: self,
                _not_sync: PhantomData,
            },
        )
    }

    fn len(&self) -> usize {
        let consumer = self.consumer.counter().load(Ordering::Acquire);
        let producer = self.producer.counter().load(Ordering::Acquire);
        Self::cursor_distance(consumer, producer).min(DEPTH)
    }

    const fn cursor_distance(consumer: usize, producer: usize) -> usize {
        if producer >= consumer {
            producer - consumer
        } else {
            DEPTH * 2 - (consumer - producer)
        }
    }

    /// The cell a cursor position names.
    #[expect(
        clippy::indexing_slicing,
        reason = "internal invariant: a cursor reduced modulo DEPTH is below DEPTH"
    )]
    fn slot(&self, cursor: usize) -> &UnsafeCell<MaybeUninit<T>> {
        &self.values[cursor % DEPTH]
    }

    const fn advance(cursor: usize) -> usize {
        if cursor + 1 == DEPTH * 2 {
            0
        } else {
            cursor + 1
        }
    }

    fn vacant_slot(&self) -> Option<AffineSpscSendPermit<'_, T, DEPTH, C>> {
        let producer = self.producer.counter().load(Ordering::Relaxed);
        let consumer = self.consumer.counter().load(Ordering::Acquire);
        if Self::cursor_distance(consumer, producer) >= DEPTH {
            return None;
        }
        Some(AffineSpscSendPermit {
            queue: self,
            producer,
            _not_sync: PhantomData,
        })
    }

    #[inline]
    fn try_send(&self, value: T) -> Result<(), AffineSpscTrySendError<T>> {
        let Some(permit) = self.vacant_slot() else {
            return Err(AffineSpscTrySendError(value));
        };
        permit.send(value);
        Ok(())
    }

    #[inline]
    fn try_receive(&self) -> Result<T, AffineSpscTryReceiveError> {
        let consumer = self.consumer.counter().load(Ordering::Relaxed);
        let producer = self.producer.counter().load(Ordering::Acquire);
        if consumer == producer {
            return Err(AffineSpscTryReceiveError::Empty);
        }
        #[allow(
            unsafe_code,
            reason = "the single consumer owns this published SPSC slot"
        )]
        // SAFETY: the producer's Release cursor initialized this slot, and
        // only the consumer moves its affine value out before returning it.
        let value = unsafe { (*self.slot(consumer).get()).assume_init_read() };
        self.consumer
            .counter()
            .store(Self::advance(consumer), Ordering::Release);
        Ok(value)
    }
}

impl<T, const DEPTH: usize, C: SpscCursor> Default for AffineSpscQueue<T, DEPTH, C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const DEPTH: usize, C: SpscCursor> Drop for AffineSpscQueue<T, DEPTH, C> {
    fn drop(&mut self) {
        let mut consumer = *self.consumer.counter_mut().get_mut();
        let producer = *self.producer.counter_mut().get_mut();
        while consumer != producer {
            #[allow(
                unsafe_code,
                reason = "queue drop releases every still-published affine value"
            )]
            // SAFETY: exclusive queue ownership proves no endpoint exists;
            // every cursor-visible slot still contains one initialized value.
            unsafe {
                (*self.slot(consumer).get()).assume_init_drop();
            }
            consumer = Self::advance(consumer);
        }
        *self.consumer.counter_mut().get_mut() = producer;
    }
}

/// The only producer endpoint for one [`AffineSpscQueue`] ownership epoch.
pub struct AffineSpscSender<'queue, T, const DEPTH: usize, C: SpscCursor = CachedSpscCursor> {
    queue: &'queue AffineSpscQueue<T, DEPTH, C>,
    // Moving the endpoint is valid, but safe code must not share one endpoint
    // between concurrent producers and violate the SPSC cursor proof.
    _not_sync: PhantomData<Cell<()>>,
}

impl<'queue, T, const DEPTH: usize, C: SpscCursor> AffineSpscSender<'queue, T, DEPTH, C> {
    #[inline]
    pub fn try_send(&self, value: T) -> Result<(), AffineSpscTrySendError<T>> {
        self.queue.try_send(value)
    }

    /// EXPERIMENT: initialize the next vacant cell in place. `init` returns
    /// `true` only after it initialized the cell; `false` publishes nothing.
    /// `None` when the queue is full.
    #[inline]
    pub fn try_send_with(&self, init: impl FnOnce(&mut MaybeUninit<T>) -> bool) -> Option<bool> {
        let permit = self.queue.vacant_slot()?;
        #[allow(unsafe_code, reason = "EXPERIMENT")]
        // SAFETY: EXPERIMENT ONLY: the permit proves the cell vacant and
        // owned by the sole producer; `init` reports whether it initialized it.
        let cell = unsafe { &mut *permit.queue.slot(permit.producer).get() };
        if !init(cell) {
            return Some(false);
        }
        permit.queue.producer.counter().store(
            AffineSpscQueue::<T, DEPTH, C>::advance(permit.producer),
            Ordering::Release,
        );
        Some(true)
    }

    /// Reserve one vacant cell, exclusively borrowing the producer until
    /// the permit is sent or dropped. A full queue returns that same borrow,
    /// allowing a readiness future to keep its sole producer registration.
    /// Neither refusal nor dropping a permit advances the producer cursor.
    ///
    /// ```compile_fail,E0502
    /// use oer_memory::AffineSpscQueue;
    /// let mut queue = AffineSpscQueue::<u8, 1>::new();
    /// let (mut sender, receiver) = queue.split();
    /// let permit = sender.try_reserve().ok().unwrap();
    /// sender.try_send(1).unwrap();
    /// permit.send(2);
    /// ```
    ///
    /// ```
    /// use oer_memory::AffineSpscQueue;
    /// let mut queue = AffineSpscQueue::<u8, 1>::new();
    /// let (mut sender, receiver) = queue.split();
    /// sender.try_reserve().ok().unwrap().send(1);
    /// assert_eq!(receiver.try_receive(), Ok(1));
    /// ```
    pub fn try_reserve(&mut self) -> Result<AffineSpscSendPermit<'_, T, DEPTH, C>, &mut Self> {
        match self.queue.vacant_slot() {
            Some(permit) => Ok(permit),
            None => Err(self),
        }
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn free_capacity(&self) -> usize {
        DEPTH.saturating_sub(self.len())
    }
}

/// Exclusive right to publish one value into a proven vacant SPSC cell.
///
/// The producer endpoint remains borrowed; the consumer can only free more
/// cells. Dropping an unused permit leaves the cursor and capacity unchanged.
#[must_use = "send a value or drop the permit to release the producer borrow"]
pub struct AffineSpscSendPermit<'sender, T, const DEPTH: usize, C: SpscCursor = CachedSpscCursor> {
    queue: &'sender AffineSpscQueue<T, DEPTH, C>,
    producer: usize,
    _not_sync: PhantomData<Cell<()>>,
}

impl<T, const DEPTH: usize, C: SpscCursor> AffineSpscSendPermit<'_, T, DEPTH, C> {
    /// Publish into the reserved cell, transferring the complete owner.
    pub fn send(self, value: T) {
        #[allow(
            unsafe_code,
            reason = "the sole producer's permit retains its proven vacant cell"
        )]
        // SAFETY: vacant_slot's Acquire cursor or the indexed queue's affine
        // permission transfer proves the prior read complete and this cursor
        // vacant. The unique producer is borrowed until this send finishes;
        // neither constructor permits a competing publication at this cursor.
        // The consumer can only free more cells before this Release write.
        unsafe {
            (*self.queue.slot(self.producer).get()).write(value);
        }
        self.queue.producer.counter().store(
            AffineSpscQueue::<T, DEPTH, C>::advance(self.producer),
            Ordering::Release,
        );
    }
}

/// The only consumer endpoint for one [`AffineSpscQueue`] ownership epoch.
pub struct AffineSpscReceiver<'queue, T, const DEPTH: usize, C: SpscCursor = CachedSpscCursor> {
    queue: &'queue AffineSpscQueue<T, DEPTH, C>,
    // Moving the endpoint is valid, but safe code must not share one endpoint
    // between concurrent consumers and violate the SPSC cursor proof.
    _not_sync: PhantomData<Cell<()>>,
}

impl<T, const DEPTH: usize, C: SpscCursor> AffineSpscReceiver<'_, T, DEPTH, C> {
    #[inline]
    pub fn try_receive(&self) -> Result<T, AffineSpscTryReceiveError> {
        self.queue.try_receive()
    }

    /// EXPERIMENT: lend the oldest value in place; `Err` moves it out to the
    /// caller, `Ok` drops it in place.
    #[inline]
    pub fn try_receive_or_take<R>(
        &self,
        use_value: impl FnOnce(&mut T) -> Result<R, ()>,
    ) -> Option<Result<R, T>> {
        let queue = self.queue;
        let consumer = queue.consumer.counter().load(Ordering::Relaxed);
        let producer = queue.producer.counter().load(Ordering::Acquire);
        if consumer == producer {
            return None;
        }
        #[allow(unsafe_code, reason = "EXPERIMENT")]
        // SAFETY: EXPERIMENT ONLY: as try_receive_with; the refused value is
        // read out exactly once instead of dropped.
        let result = unsafe {
            let cell = &mut *queue.slot(consumer).get();
            match use_value(cell.assume_init_mut()) {
                Ok(result) => {
                    cell.assume_init_drop();
                    Ok(result)
                }
                Err(()) => Err(cell.assume_init_read()),
            }
        };
        queue.consumer.counter().store(
            AffineSpscQueue::<T, DEPTH, C>::advance(consumer),
            Ordering::Release,
        );
        Some(result)
    }

    /// EXPERIMENT: lend the oldest value in place, then drop it there.
    #[inline]
    pub fn try_receive_with<R>(&self, use_value: impl FnOnce(&mut T) -> R) -> Option<R> {
        let queue = self.queue;
        let consumer = queue.consumer.counter().load(Ordering::Relaxed);
        let producer = queue.producer.counter().load(Ordering::Acquire);
        if consumer == producer {
            return None;
        }
        #[allow(unsafe_code, reason = "EXPERIMENT")]
        // SAFETY: EXPERIMENT ONLY: the producer's Release cursor initialized
        // this cell; the sole consumer uses and drops it before advancing.
        let result = unsafe {
            let cell = &mut *queue.slot(consumer).get();
            let result = use_value(cell.assume_init_mut());
            cell.assume_init_drop();
            result
        };
        queue.consumer.counter().store(
            AffineSpscQueue::<T, DEPTH, C>::advance(consumer),
            Ordering::Release,
        );
        Some(result)
    }

    pub fn len(&self) -> usize {
        self.queue.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests;
