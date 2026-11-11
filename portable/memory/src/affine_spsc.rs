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
    /// The values its endpoint moved past this cursor and has not yet
    /// passed it over: staged by the producer, taken by the consumer.
    /// Only its endpoint writes it; an exclusive owner settles it.
    #[doc(hidden)]
    fn pending(&self) -> &AtomicUsize;
    #[doc(hidden)]
    fn pending_mut(&mut self) -> &mut AtomicUsize;
}

/// One cache line per cursor, the default queue layout.
#[repr(align(64))]
pub struct CachedSpscCursor(AtomicUsize, AtomicUsize);
/// Unpadded atomic cursor for a same-core priority handoff.
pub struct CompactSpscCursor(AtomicUsize, AtomicUsize);

macro_rules! cursor {
    ($layout:ty) => {
        impl cursor_sealed::Sealed for $layout {}
        impl SpscCursor for $layout {
            const NEW: Self = Self(AtomicUsize::new(0), AtomicUsize::new(0));
            #[inline(always)]
            fn counter(&self) -> &AtomicUsize {
                &self.0
            }
            fn counter_mut(&mut self) -> &mut AtomicUsize {
                &mut self.0
            }
            #[inline(always)]
            fn pending(&self) -> &AtomicUsize {
                &self.1
            }
            fn pending_mut(&mut self) -> &mut AtomicUsize {
                &mut self.1
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
/// use oer_portable_memory::AffineSpscQueue;
/// let mut queue = AffineSpscQueue::<u8, 2>::new();
/// let (first, consumer) = queue.split();
/// let second = queue.split();
/// first.try_send(1).unwrap();
/// ```
///
/// Capacity checks run at compile time even in an ordinary function call.
///
/// ```compile_fail,E0080
/// use oer_portable_memory::AffineSpscQueue;
/// let queue = AffineSpscQueue::<u8, 0>::new();
/// ```
///
/// ```compile_fail,E0080
/// use oer_portable_memory::AffineSpscQueue;
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
                producer: CachedSpscCursor(AtomicUsize::new(0), AtomicUsize::new(0)),
                consumer: CachedSpscCursor(AtomicUsize::new(0), AtomicUsize::new(0)),
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
        self.settle();
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

    /// End the batches of an earlier endpoint pair, which ended without
    /// ending them: what its producer staged is published, in order, and
    /// the cells its consumer took values out of are freed. No value is
    /// seen again, and none is lost.
    fn settle(&mut self) {
        let taken = core::mem::take(self.consumer.pending_mut().get_mut());
        let consumer = *self.consumer.counter_mut().get_mut();
        *self.consumer.counter_mut().get_mut() = Self::advance_by(consumer, taken);
        let staged = core::mem::take(self.producer.pending_mut().get_mut());
        let producer = *self.producer.counter_mut().get_mut();
        *self.producer.counter_mut().get_mut() = Self::advance_by(producer, staged);
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

    /// The cursor `count` cells past `cursor`, `count` at most `DEPTH`. A
    /// cursor stays below `DEPTH * 2`, which fits `usize`
    /// ([`Self::check_capacity`]): neither step can overflow.
    const fn advance_by(cursor: usize, count: usize) -> usize {
        let room = DEPTH * 2 - cursor;
        if count >= room {
            count - room
        } else {
            cursor + count
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
        // A batch no endpoint ended: staged values are published here and
        // dropped below, taken ones moved out already and freed.
        self.settle();
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
    /// Publish `value`, after every value staged before it.
    #[inline]
    pub fn try_send(&self, value: T) -> Result<(), AffineSpscTrySendError<T>> {
        self.publish();
        self.queue.try_send(value)
    }

    /// Write `value` in the next cell past the published ones without
    /// publishing it: the consumer sees it, with every value staged before,
    /// at [`Self::publish`]. A batch of values publishes its cursor once.
    ///
    /// ```
    /// use oer_portable_memory::AffineSpscQueue;
    /// let mut queue = AffineSpscQueue::<u8, 2>::new();
    /// let (mut sender, receiver) = queue.split();
    /// sender.stage(1).unwrap();
    /// sender.stage(2).unwrap();
    /// assert_eq!(sender.stage(3).unwrap_err().0, 3, "the queue is full");
    /// assert!(receiver.is_empty(), "nothing is published yet");
    /// sender.publish();
    /// assert_eq!(receiver.try_receive(), Ok(1));
    /// assert_eq!(receiver.try_receive(), Ok(2));
    /// ```
    ///
    /// Only a `Copy` value is staged. An endpoint that ends before its
    /// publication leaves its values staged in the queue: the next
    /// [`AffineSpscQueue::split`], or the queue's drop, publishes them.
    ///
    /// # Errors
    /// [`AffineSpscTrySendError`]: every cell is published or staged; the
    /// value comes back.
    pub fn stage(&mut self, value: T) -> Result<(), AffineSpscTrySendError<T>>
    where
        T: Copy,
    {
        let staged = self.queue.producer.pending().load(Ordering::Relaxed);
        let producer = self.queue.producer.counter().load(Ordering::Relaxed);
        let consumer = self.queue.consumer.counter().load(Ordering::Acquire);
        if AffineSpscQueue::<T, DEPTH, C>::cursor_distance(consumer, producer)
            .saturating_add(staged)
            >= DEPTH
        {
            return Err(AffineSpscTrySendError(value));
        }
        let cursor = AffineSpscQueue::<T, DEPTH, C>::advance_by(producer, staged);
        #[allow(
            unsafe_code,
            reason = "the sole producer writes a cell past its published cursor"
        )]
        // SAFETY: the Acquire consumer cursor shows this cell read and freed:
        // the published and staged values together leave it vacant. The
        // consumer reads only up to the published producer cursor, which
        // stays below this cell until `publish`; this endpoint is the sole
        // producer, borrowed mutably, so no other write reaches the cell.
        unsafe {
            (*self.queue.slot(cursor).get()).write(value);
        }
        self.queue
            .producer
            .pending()
            .store(staged.saturating_add(1), Ordering::Relaxed);
        Ok(())
    }

    /// Publish every staged value at once, in its order.
    pub fn publish(&self) {
        let staged = self.queue.producer.pending().load(Ordering::Relaxed);
        if staged == 0 {
            return;
        }
        self.queue.producer.pending().store(0, Ordering::Relaxed);
        let producer = self.queue.producer.counter().load(Ordering::Relaxed);
        self.queue.producer.counter().store(
            AffineSpscQueue::<T, DEPTH, C>::advance_by(producer, staged),
            Ordering::Release,
        );
    }

    /// Values staged and not yet published.
    pub fn staged(&self) -> usize {
        self.queue.producer.pending().load(Ordering::Relaxed)
    }

    /// Reserve one vacant cell, exclusively borrowing the producer until
    /// the permit is sent or dropped. A full queue returns that same borrow,
    /// allowing a readiness future to keep its sole producer registration.
    /// Neither refusal nor dropping a permit advances the producer cursor.
    ///
    /// ```compile_fail,E0502
    /// use oer_portable_memory::AffineSpscQueue;
    /// let mut queue = AffineSpscQueue::<u8, 1>::new();
    /// let (mut sender, receiver) = queue.split();
    /// let permit = sender.try_reserve().ok().unwrap();
    /// sender.try_send(1).unwrap();
    /// permit.send(2);
    /// ```
    ///
    /// ```
    /// use oer_portable_memory::AffineSpscQueue;
    /// let mut queue = AffineSpscQueue::<u8, 1>::new();
    /// let (mut sender, receiver) = queue.split();
    /// sender.try_reserve().ok().unwrap().send(1);
    /// assert_eq!(receiver.try_receive(), Ok(1));
    /// ```
    pub fn try_reserve(&mut self) -> Result<AffineSpscSendPermit<'_, T, DEPTH, C>, &mut Self> {
        // A permit's cell follows every staged value.
        self.publish();
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

    /// Cells neither published nor staged.
    pub fn free_capacity(&self) -> usize {
        DEPTH
            .saturating_sub(self.len())
            .saturating_sub(self.staged())
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
    /// Move the next value out, after releasing every cell taken before.
    #[inline]
    pub fn try_receive(&self) -> Result<T, AffineSpscTryReceiveError> {
        self.release();
        self.queue.try_receive()
    }

    /// Move the next published value out without freeing its cell: the
    /// producer may write the cell again only after [`Self::release`]. A
    /// batch of values frees its cells once. Only a `Copy` value is taken.
    /// An endpoint that ends before its release leaves its cells taken in
    /// the queue: the next [`AffineSpscQueue::split`], or the queue's
    /// drop, frees them, and no value is received again.
    ///
    /// ```
    /// use oer_portable_memory::AffineSpscQueue;
    /// let mut queue = AffineSpscQueue::<u8, 2>::new();
    /// let (sender, mut receiver) = queue.split();
    /// sender.try_send(1).unwrap();
    /// sender.try_send(2).unwrap();
    /// assert_eq!(receiver.take(), Some(1));
    /// assert_eq!(receiver.take(), Some(2));
    /// assert_eq!(receiver.take(), None);
    /// assert!(sender.try_send(3).is_err(), "no cell is free yet");
    /// receiver.release();
    /// assert!(sender.try_send(3).is_ok());
    /// ```
    ///
    /// A value with an owner is received whole, never taken:
    ///
    /// ```compile_fail,E0277
    /// use oer_portable_memory::AffineSpscQueue;
    /// let mut queue = AffineSpscQueue::<Box<u8>, 2>::new();
    /// let (_sender, mut receiver) = queue.split();
    /// let _ = receiver.take();
    /// ```
    pub fn take(&mut self) -> Option<T>
    where
        T: Copy,
    {
        let taken = self.queue.consumer.pending().load(Ordering::Relaxed);
        let consumer = self.queue.consumer.counter().load(Ordering::Relaxed);
        let producer = self.queue.producer.counter().load(Ordering::Acquire);
        if AffineSpscQueue::<T, DEPTH, C>::cursor_distance(consumer, producer) <= taken {
            return None;
        }
        let cursor = AffineSpscQueue::<T, DEPTH, C>::advance_by(consumer, taken);
        #[allow(
            unsafe_code,
            reason = "the sole consumer moves a published value out of its cell"
        )]
        // SAFETY: the producer's Release cursor, read with Acquire, covers
        // this cell, so it holds one initialized value; the values taken
        // before it lie below, each moved out once. The producer writes a
        // cell only once the consumer cursor passes it, which waits for
        // `release`; this endpoint is the sole consumer, borrowed mutably.
        let value = unsafe { (*self.queue.slot(cursor).get()).assume_init_read() };
        self.queue
            .consumer
            .pending()
            .store(taken.saturating_add(1), Ordering::Relaxed);
        Some(value)
    }

    /// Free every cell taken, at once, for the producer.
    pub fn release(&self) {
        let taken = self.queue.consumer.pending().load(Ordering::Relaxed);
        if taken == 0 {
            return;
        }
        self.queue.consumer.pending().store(0, Ordering::Relaxed);
        let consumer = self.queue.consumer.counter().load(Ordering::Relaxed);
        self.queue.consumer.counter().store(
            AffineSpscQueue::<T, DEPTH, C>::advance_by(consumer, taken),
            Ordering::Release,
        );
    }

    /// Published values not yet taken.
    pub fn len(&self) -> usize {
        self.queue
            .len()
            .saturating_sub(self.queue.consumer.pending().load(Ordering::Relaxed))
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests;
