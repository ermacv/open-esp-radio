//! Audited bounded SPSC ownership transfer.
//!
//! The queue moves affine values between one producer and one consumer. It is
//! intentionally executor-agnostic: higher layers may wrap its non-blocking
//! endpoints in their own wake and telemetry policy without reopening unsafe
//! slot access.

mod indexed;
pub use indexed::{
    AffineIndexedSpscQueue, AffineIndexedSpscReceiver, AffineIndexedSpscSender, UniqueQueueIndex,
};

use core::{
    cell::{Cell, UnsafeCell},
    marker::PhantomData,
    mem::MaybeUninit,
    sync::atomic::{AtomicUsize, Ordering},
};

#[repr(align(64))]
struct CacheLine<T>(T);

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
pub struct AffineSpscQueue<T, const DEPTH: usize> {
    values: [UnsafeCell<MaybeUninit<T>>; DEPTH],
    producer: CacheLine<AtomicUsize>,
    consumer: CacheLine<AtomicUsize>,
}

#[allow(
    unsafe_code,
    reason = "bounded SPSC cursor ownership serializes every value slot"
)]
// SAFETY: each slot has exactly one producer before its Release publication
// and exactly one consumer after the matching Acquire load. The consumer's
// Release cursor prevents reuse until the affine value has moved out.
unsafe impl<T: Send, const DEPTH: usize> Sync for AffineSpscQueue<T, DEPTH> {}

impl<T, const DEPTH: usize> AffineSpscQueue<T, DEPTH> {
    pub const fn new() -> Self {
        const {
            assert!(DEPTH != 0, "affine SPSC queue must not be empty");
            assert!(
                DEPTH <= usize::MAX / 2,
                "affine SPSC cursor domain must fit usize"
            );
        }
        Self {
            values: [const { UnsafeCell::new(MaybeUninit::uninit()) }; DEPTH],
            producer: CacheLine(AtomicUsize::new(0)),
            consumer: CacheLine(AtomicUsize::new(0)),
        }
    }

    /// Exclusively borrow the storage to grant its sole endpoint pair.
    /// Both endpoints retain this borrow: another split cannot overlap them.
    /// Values left by an earlier pair stay owned by the queue and are received
    /// by the next consumer in their original order.
    pub fn split(
        &mut self,
    ) -> (
        AffineSpscSender<'_, T, DEPTH>,
        AffineSpscReceiver<'_, T, DEPTH>,
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
        let consumer = self.consumer.0.load(Ordering::Acquire);
        let producer = self.producer.0.load(Ordering::Acquire);
        Self::cursor_distance(consumer, producer).min(DEPTH)
    }

    const fn cursor_distance(consumer: usize, producer: usize) -> usize {
        if producer >= consumer {
            producer - consumer
        } else {
            DEPTH * 2 - (consumer - producer)
        }
    }

    const fn advance(cursor: usize) -> usize {
        if cursor + 1 == DEPTH * 2 {
            0
        } else {
            cursor + 1
        }
    }

    fn vacant_slot(&self) -> Option<AffineSpscSendPermit<'_, T, DEPTH>> {
        let producer = self.producer.0.load(Ordering::Relaxed);
        let consumer = self.consumer.0.load(Ordering::Acquire);
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
        let consumer = self.consumer.0.load(Ordering::Relaxed);
        let producer = self.producer.0.load(Ordering::Acquire);
        if consumer == producer {
            return Err(AffineSpscTryReceiveError::Empty);
        }
        #[allow(
            unsafe_code,
            reason = "the single consumer owns this published SPSC slot"
        )]
        // SAFETY: the producer's Release cursor initialized this slot, and
        // only the consumer moves its affine value out before returning it.
        let value = unsafe { (*self.values[consumer % DEPTH].get()).assume_init_read() };
        self.consumer
            .0
            .store(Self::advance(consumer), Ordering::Release);
        Ok(value)
    }
}

impl<T, const DEPTH: usize> Default for AffineSpscQueue<T, DEPTH> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const DEPTH: usize> Drop for AffineSpscQueue<T, DEPTH> {
    fn drop(&mut self) {
        let mut consumer = *self.consumer.0.get_mut();
        let producer = *self.producer.0.get_mut();
        while consumer != producer {
            #[allow(
                unsafe_code,
                reason = "queue drop releases every still-published affine value"
            )]
            // SAFETY: exclusive queue ownership proves no endpoint exists;
            // every cursor-visible slot still contains one initialized value.
            unsafe {
                self.values[consumer % DEPTH].get_mut().assume_init_drop();
            }
            consumer = Self::advance(consumer);
        }
        *self.consumer.0.get_mut() = producer;
    }
}

/// The only producer endpoint for one [`AffineSpscQueue`] ownership epoch.
pub struct AffineSpscSender<'queue, T, const DEPTH: usize> {
    queue: &'queue AffineSpscQueue<T, DEPTH>,
    // Moving the endpoint is valid, but safe code must not share one endpoint
    // between concurrent producers and violate the SPSC cursor proof.
    _not_sync: PhantomData<Cell<()>>,
}

impl<'queue, T, const DEPTH: usize> AffineSpscSender<'queue, T, DEPTH> {
    #[inline]
    pub fn try_send(&self, value: T) -> Result<(), AffineSpscTrySendError<T>> {
        self.queue.try_send(value)
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
    pub fn try_reserve(&mut self) -> Result<AffineSpscSendPermit<'_, T, DEPTH>, &mut Self> {
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
pub struct AffineSpscSendPermit<'sender, T, const DEPTH: usize> {
    queue: &'sender AffineSpscQueue<T, DEPTH>,
    producer: usize,
    _not_sync: PhantomData<Cell<()>>,
}

impl<T, const DEPTH: usize> AffineSpscSendPermit<'_, T, DEPTH> {
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
            (*self.queue.values[self.producer % DEPTH].get()).write(value);
        }
        self.queue.producer.0.store(
            AffineSpscQueue::<T, DEPTH>::advance(self.producer),
            Ordering::Release,
        );
    }
}

/// The only consumer endpoint for one [`AffineSpscQueue`] ownership epoch.
pub struct AffineSpscReceiver<'queue, T, const DEPTH: usize> {
    queue: &'queue AffineSpscQueue<T, DEPTH>,
    // Moving the endpoint is valid, but safe code must not share one endpoint
    // between concurrent consumers and violate the SPSC cursor proof.
    _not_sync: PhantomData<Cell<()>>,
}

impl<T, const DEPTH: usize> AffineSpscReceiver<'_, T, DEPTH> {
    #[inline]
    pub fn try_receive(&self) -> Result<T, AffineSpscTryReceiveError> {
        self.queue.try_receive()
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
