//! Batched endpoints of an [`AffineSpscQueue`]: each keeps its own cursor
//! and its batch as plain local indices, reads the other end's cursor once
//! for the prefix it may use, and stores its cursor once at the batch's
//! boundary. Only `Copy` values move this way, so an endpoint that ends in
//! a batch leaves no owner behind: what it staged is unpublished, and what
//! it took and did not release is received again by the queue's next
//! consumer. The ordinary endpoints pay nothing for this mode.

use core::cell::Cell;
use core::marker::PhantomData;
use core::sync::atomic::Ordering;

use super::{AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, CachedSpscCursor, SpscCursor};

/// The sole producer of one queue, in batches ([`AffineSpscSender::batched`]).
pub struct BatchedSpscSender<'queue, T, const DEPTH: usize, C: SpscCursor = CachedSpscCursor> {
    queue: &'queue AffineSpscQueue<T, DEPTH, C>,
    /// The published producer cursor: this endpoint alone moves it.
    producer: usize,
    /// Values written past it and not yet published.
    staged: usize,
    /// Cells past the staged ones the last look at the consumer's cursor
    /// found vacant.
    vacant: usize,
    // One endpoint per context, as the ordinary one.
    _not_sync: PhantomData<Cell<()>>,
}

/// The sole consumer of one queue, in batches
/// ([`AffineSpscReceiver::batched`]).
pub struct BatchedSpscReceiver<'queue, T, const DEPTH: usize, C: SpscCursor = CachedSpscCursor> {
    queue: &'queue AffineSpscQueue<T, DEPTH, C>,
    /// The released consumer cursor: this endpoint alone moves it.
    consumer: usize,
    /// Values read past it whose cells are not yet released.
    taken: usize,
    /// Values past the taken ones the last look at the producer's cursor
    /// found published.
    ready: usize,
    _not_sync: PhantomData<Cell<()>>,
}

impl<'queue, T: Copy, const DEPTH: usize, C: SpscCursor> AffineSpscSender<'queue, T, DEPTH, C> {
    /// This producer in batches: [`BatchedSpscSender::stage`] and one
    /// [`BatchedSpscSender::publish`] per batch.
    pub fn batched(self) -> BatchedSpscSender<'queue, T, DEPTH, C> {
        BatchedSpscSender {
            queue: self.queue,
            producer: self.queue.producer.counter().load(Ordering::Relaxed),
            staged: 0,
            vacant: 0,
            _not_sync: PhantomData,
        }
    }
}

impl<'queue, T: Copy, const DEPTH: usize, C: SpscCursor> AffineSpscReceiver<'queue, T, DEPTH, C> {
    /// This consumer in batches: [`BatchedSpscReceiver::take`] and one
    /// [`BatchedSpscReceiver::release`] per batch.
    pub fn batched(self) -> BatchedSpscReceiver<'queue, T, DEPTH, C> {
        BatchedSpscReceiver {
            queue: self.queue,
            consumer: self.queue.consumer.counter().load(Ordering::Relaxed),
            taken: 0,
            ready: 0,
            _not_sync: PhantomData,
        }
    }
}

impl<T: Copy, const DEPTH: usize, C: SpscCursor> BatchedSpscSender<'_, T, DEPTH, C> {
    /// Look at the consumer's cursor once: the cells past the published and
    /// staged ones that are vacant now.
    fn look(&mut self) {
        let consumer = self.queue.consumer.counter().load(Ordering::Acquire);
        let used = AffineSpscQueue::<T, DEPTH, C>::cursor_distance(consumer, self.producer)
            .saturating_add(self.staged);
        self.vacant = DEPTH.saturating_sub(used);
    }

    /// Write `value` in the next cell, unseen by the consumer until
    /// [`Self::publish`]. The consumer's cursor is read only once the cells
    /// the last look found are used up.
    ///
    /// ```
    /// use oer_portable_memory::AffineSpscQueue;
    /// let mut queue = AffineSpscQueue::<u8, 2>::new();
    /// let (sender, receiver) = queue.split();
    /// let mut sender = sender.batched();
    /// sender.stage(1).unwrap();
    /// sender.stage(2).unwrap();
    /// assert_eq!(sender.stage(3), Err(3), "the queue is full");
    /// assert!(receiver.is_empty(), "nothing is published yet");
    /// sender.publish();
    /// assert_eq!(receiver.try_receive(), Ok(1));
    /// ```
    ///
    /// # Errors
    /// `value` back: every cell is published or staged.
    pub fn stage(&mut self, value: T) -> Result<(), T> {
        if self.vacant == 0 {
            self.look();
            if self.vacant == 0 {
                return Err(value);
            }
        }
        let cursor = AffineSpscQueue::<T, DEPTH, C>::advance_by(self.producer, self.staged);
        #[allow(
            unsafe_code,
            reason = "the sole producer writes a vacant cell past its published cursor"
        )]
        // SAFETY: the Acquire look at the consumer's cursor found this cell
        // among the vacant ones past every published and staged value, so
        // the consumer has read it and reads it again only after the
        // producer's Release store passes it, at `publish`. This endpoint
        // is the queue's sole producer, borrowed mutably here.
        unsafe {
            (*self.queue.slot(cursor).get()).write(value);
        }
        self.staged = self.staged.saturating_add(1);
        self.vacant = self.vacant.saturating_sub(1);
        Ok(())
    }

    /// Publish every staged value at once, in order: one store.
    pub fn publish(&mut self) {
        if self.staged == 0 {
            return;
        }
        self.producer = AffineSpscQueue::<T, DEPTH, C>::advance_by(self.producer, self.staged);
        self.staged = 0;
        self.queue
            .producer
            .counter()
            .store(self.producer, Ordering::Release);
    }

    /// Values staged and not yet published.
    pub const fn staged(&self) -> usize {
        self.staged
    }

    /// Published values the consumer has not released, at one look.
    pub fn len(&self) -> usize {
        let consumer = self.queue.consumer.counter().load(Ordering::Acquire);
        AffineSpscQueue::<T, DEPTH, C>::cursor_distance(consumer, self.producer)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<T: Copy, const DEPTH: usize, C: SpscCursor> BatchedSpscReceiver<'_, T, DEPTH, C> {
    /// Look at the producer's cursor once: the published values past the
    /// taken ones.
    fn look(&mut self) {
        let producer = self.queue.producer.counter().load(Ordering::Acquire);
        self.ready = AffineSpscQueue::<T, DEPTH, C>::cursor_distance(self.consumer, producer)
            .min(DEPTH)
            .saturating_sub(self.taken);
    }

    /// Read the next published value, its cell still held until
    /// [`Self::release`]. The producer's cursor is read only once the
    /// values the last look found are used up.
    ///
    /// ```
    /// use oer_portable_memory::AffineSpscQueue;
    /// let mut queue = AffineSpscQueue::<u8, 2>::new();
    /// let (sender, receiver) = queue.split();
    /// let mut receiver = receiver.batched();
    /// sender.try_send(1).unwrap();
    /// sender.try_send(2).unwrap();
    /// assert_eq!(receiver.take(), Some(1));
    /// assert_eq!(receiver.take(), Some(2));
    /// assert_eq!(receiver.take(), None);
    /// assert!(sender.try_send(3).is_err(), "no cell is free yet");
    /// receiver.release();
    /// assert!(sender.try_send(3).is_ok());
    /// ```
    pub fn take(&mut self) -> Option<T> {
        if self.ready == 0 {
            self.look();
            if self.ready == 0 {
                return None;
            }
        }
        let cursor = AffineSpscQueue::<T, DEPTH, C>::advance_by(self.consumer, self.taken);
        #[allow(
            unsafe_code,
            reason = "the sole consumer reads a published cell past its released cursor"
        )]
        // SAFETY: the Acquire look at the producer's cursor found this cell
        // among the published ones past every taken value, so it holds one
        // initialized value. The producer writes it again only after the
        // consumer's Release store passes it, at `release`. `T: Copy`: the
        // read leaves the cell's value valid and owning nothing.
        let value = unsafe { (*self.queue.slot(cursor).get()).assume_init_read() };
        self.taken = self.taken.saturating_add(1);
        self.ready = self.ready.saturating_sub(1);
        Some(value)
    }

    /// Free every taken cell at once for the producer: one store.
    pub fn release(&mut self) {
        if self.taken == 0 {
            return;
        }
        self.consumer = AffineSpscQueue::<T, DEPTH, C>::advance_by(self.consumer, self.taken);
        self.taken = 0;
        self.queue
            .consumer
            .counter()
            .store(self.consumer, Ordering::Release);
    }

    /// Values taken whose cells are not yet released.
    pub const fn taken(&self) -> usize {
        self.taken
    }

    /// Published values not yet taken, at one look.
    pub fn len(&self) -> usize {
        let producer = self.queue.producer.counter().load(Ordering::Acquire);
        AffineSpscQueue::<T, DEPTH, C>::cursor_distance(self.consumer, producer)
            .min(DEPTH)
            .saturating_sub(self.taken)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
