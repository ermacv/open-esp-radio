//! A single-producer, single-consumer queue whose values stay in their
//! slots: the producer fills a free slot in place and the consumer reads
//! the oldest filled one in place, so a large value is written once and
//! never moved through the queue. Built from [`ClaimCell`] slots and two
//! affine queues of slot indices, it needs no unsafe code of its own.
//!
//! An end wakes the other only while that one waits: the consumer until
//! the queue holds as many filled slots as it asked for (one by default),
//! the producer on a full queue. A fill or a read while the other end is
//! busy raises no notify, so ends on different harts exchange one signal
//! per burst, not one per value. A producer that knows where its burst ends
//! holds that signal for a consumer waiting for a single value
//! ([`SlotSender::try_send_held_with`], [`SlotSender::flush`]), which then
//! finds the burst, not its first value.

use crate::{AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, ClaimCell, QueueNotify};
use atomic_waker::AtomicWaker;
use core::future::poll_fn;
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering, fence};
use core::task::Poll;

#[cfg(test)]
mod tests;

/// Slots, the indices of filled and free ones, and the two notifies: `C`
/// tells the consumer that a slot was filled, `P` the producer that one was
/// freed. The composition splits it once and moves the two endpoints to
/// their tasks.
pub struct WakingSlotQueue<T, const DEPTH: usize, C = AtomicWaker, P = AtomicWaker> {
    slots: [ClaimCell<T>; DEPTH],
    filled: AffineSpscQueue<u8, DEPTH>,
    free: AffineSpscQueue<u8, DEPTH>,
    consumer: C,
    producer: P,
    /// The filled slots the consumer waits for after its registration;
    /// zero while it does not wait.
    consumer_wants: AtomicUsize,
    /// The producer found no free slot after its registration.
    producer_waits: AtomicBool,
    installed: bool,
}

/// Why a send filled no slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlotSendError<E> {
    /// Every slot is filled or being read.
    Full,
    /// The fill refused; the slot stays free.
    Refused(E),
    /// A slot the indices named was not free: a broken queue.
    Lost,
}

impl<T, const DEPTH: usize> WakingSlotQueue<T, DEPTH> {
    pub const fn new() -> Self {
        Self::across_harts(AtomicWaker::new(), AtomicWaker::new())
    }
}

impl<T, const DEPTH: usize, C: QueueNotify, P: QueueNotify> WakingSlotQueue<T, DEPTH, C, P> {
    const FITS: () = {
        if DEPTH == 0 || DEPTH > u8::MAX as usize + 1 {
            panic!("a slot queue names its slots in u8 and has at least one");
        }
    };

    /// A queue whose ends wake through `consumer` when a slot is filled and
    /// through `producer` when one is freed: for ends on different harts,
    /// each the waiting end's executor signal.
    pub const fn across_harts(consumer: C, producer: P) -> Self {
        let () = Self::FITS;
        Self {
            slots: [const { ClaimCell::new() }; DEPTH],
            filled: const { AffineSpscQueue::new() },
            free: const { AffineSpscQueue::new() },
            consumer,
            producer,
            consumer_wants: AtomicUsize::new(0),
            producer_waits: AtomicBool::new(false),
            installed: false,
        }
    }

    /// Exclusively borrow the storage to grant its sole endpoint pair. The
    /// first split installs every slot with `init`'s value; a later one
    /// reuses the slots and receives the values its predecessor left
    /// filled, in their order. Every other slot is free again, including
    /// one a dropped producer held between a refused fill and its next.
    pub fn split(
        &mut self,
        mut init: impl FnMut() -> T,
    ) -> (
        SlotSender<'_, T, DEPTH, C, P>,
        SlotReceiver<'_, T, DEPTH, C, P>,
    ) {
        let (filled_send, filled_receive) = self.filled.split();
        let (free_send, free_receive) = self.free.split();
        if !self.installed {
            for slot in &self.slots {
                // A fresh queue: every slot is empty.
                let _installed = slot.install(init());
            }
            self.installed = true;
        }
        // Rebuild both index queues: the filled ones in their order, then
        // every other index free. Both have room for all `DEPTH`.
        let mut order = [0_u8; DEPTH];
        let mut filled = [false; DEPTH];
        let mut count = 0_usize;
        while let Ok(index) = filled_receive.try_receive() {
            if let (Some(seen), Some(place)) =
                (filled.get_mut(usize::from(index)), order.get_mut(count))
                && !*seen
            {
                *seen = true;
                *place = index;
                count = count.saturating_add(1);
            }
        }
        while free_receive.try_receive().is_ok() {}
        for &index in order.iter().take(count) {
            let _queued = filled_send.try_send(index);
        }
        for (index, seen) in (0_u8..).zip(filled) {
            if !seen {
                let _queued = free_send.try_send(index);
            }
        }
        let (consumer, producer) = (&self.consumer, &self.producer);
        let waits = Waits {
            consumer: &self.consumer_wants,
            producer: &self.producer_waits,
        };
        // A new pair starts with neither end waiting: each registers again.
        waits.consumer.store(0, Ordering::SeqCst);
        waits.producer.store(false, Ordering::SeqCst);
        (
            SlotSender {
                slots: &self.slots,
                filled: filled_send,
                free: free_receive,
                spare: None,
                consumer,
                producer,
                waits,
            },
            SlotReceiver {
                slots: &self.slots,
                filled: filled_receive,
                free: free_send,
                consumer,
                producer,
                waits,
            },
        )
    }
}

/// Whether each end waits. An end that registers sets its flag and then
/// checks the queue; the other end publishes and then takes the flag. A
/// `SeqCst` fence on both sides orders each end's store before its load,
/// so either the waiting end sees the value or the publishing end sees
/// the flag and notifies: no wake is lost.
#[derive(Clone, Copy)]
struct Waits<'a> {
    consumer: &'a AtomicUsize,
    producer: &'a AtomicBool,
}

/// Set `waits`, then whether `ready` still holds; a ready end waits no more.
fn arm(waits: &AtomicBool, ready: impl FnOnce() -> bool) -> bool {
    waits.store(true, Ordering::SeqCst);
    fence(Ordering::SeqCst);
    let ready = ready();
    if ready {
        waits.store(false, Ordering::SeqCst);
    }
    ready
}

/// Set the consumer's `wants`, then whether `filled` already holds that
/// many; a ready consumer waits no more.
fn arm_wants(wants: &AtomicUsize, want: usize, filled: impl FnOnce() -> usize) -> bool {
    wants.store(want, Ordering::SeqCst);
    fence(Ordering::SeqCst);
    let ready = filled() >= want;
    if ready {
        wants.store(0, Ordering::SeqCst);
    }
    ready
}

/// After a fill: notify the consumer only when it waits and `filled` slots
/// reach what it wants. A consumer that registers again meanwhile runs and
/// sees the slots itself.
fn wake_wanting(wants: &AtomicUsize, filled: usize, least: usize, notify: impl FnOnce()) {
    fence(Ordering::SeqCst);
    let want = wants.load(Ordering::SeqCst);
    if want >= least
        && filled >= want
        && wants
            .compare_exchange(want, 0, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    {
        notify();
    }
}

/// After a publication: notify the other end only when it waits.
fn wake_waiting(waits: &AtomicBool, notify: impl FnOnce()) {
    fence(Ordering::SeqCst);
    if waits.swap(false, Ordering::SeqCst) {
        notify();
    }
}

impl<T, const DEPTH: usize> Default for WakingSlotQueue<T, DEPTH> {
    fn default() -> Self {
        Self::new()
    }
}

/// The producer: fills free slots in place.
pub struct SlotSender<'a, T, const DEPTH: usize, C = AtomicWaker, P = AtomicWaker> {
    slots: &'a [ClaimCell<T>; DEPTH],
    filled: AffineSpscSender<'a, u8, DEPTH>,
    free: AffineSpscReceiver<'a, u8, DEPTH>,
    /// A free slot a refused fill left: the next fill takes it first.
    spare: Option<u8>,
    consumer: &'a C,
    producer: &'a P,
    waits: Waits<'a>,
}

impl<T, const DEPTH: usize, C: QueueNotify, P: QueueNotify> SlotSender<'_, T, DEPTH, C, P> {
    /// Slots free now; a concurrent receive may free another.
    pub fn free_capacity(&self) -> usize {
        self.free
            .len()
            .saturating_add(usize::from(self.spare.is_some()))
    }

    /// Fill the oldest free slot with `fill` and publish it, waking the
    /// consumer if it waits. A refused fill leaves the slot free and
    /// publishes nothing.
    ///
    /// # Errors
    ///
    /// [`SlotSendError`]: no free slot, the fill's refusal, or a broken
    /// queue.
    pub fn try_send_with<R, E>(
        &mut self,
        fill: impl FnOnce(&mut T) -> Result<R, E>,
    ) -> Result<R, SlotSendError<E>> {
        let value = self.fill(fill)?;
        let consumer = self.consumer;
        wake_wanting(self.waits.consumer, self.filled.len(), 1, || {
            consumer.notify();
        });
        Ok(value)
    }

    /// [`Self::try_send_with`] within a burst: a consumer waiting for a
    /// single filled slot sleeps on until [`Self::flush`], so it finds the
    /// burst, not its first value; one waiting for more wakes as usual.
    ///
    /// # Errors
    ///
    /// As [`Self::try_send_with`].
    pub fn try_send_held_with<R, E>(
        &mut self,
        fill: impl FnOnce(&mut T) -> Result<R, E>,
    ) -> Result<R, SlotSendError<E>> {
        let value = self.fill(fill)?;
        let consumer = self.consumer;
        wake_wanting(self.waits.consumer, self.filled.len(), 2, || {
            consumer.notify();
        });
        Ok(value)
    }

    /// End a burst: wake the consumer if what is filled is what it waits
    /// for.
    pub fn flush(&mut self) {
        let consumer = self.consumer;
        wake_wanting(self.waits.consumer, self.filled.len(), 1, || {
            consumer.notify();
        });
    }

    /// Fill the oldest free slot and publish it.
    fn fill<R, E>(
        &mut self,
        fill: impl FnOnce(&mut T) -> Result<R, E>,
    ) -> Result<R, SlotSendError<E>> {
        let Some(index) = self.spare.take().or_else(|| self.free.try_receive().ok()) else {
            return Err(SlotSendError::Full);
        };
        let slot = self
            .slots
            .get(usize::from(index))
            .ok_or(SlotSendError::Lost)?;
        let filled = slot.lend(fill).map_err(|_| SlotSendError::Lost)?;
        let value = match filled {
            Ok(value) => value,
            Err(refused) => {
                // The slot stays free, for the next fill.
                self.spare = Some(index);
                return Err(SlotSendError::Refused(refused));
            }
        };
        // The index was free, so the filled queue has room for it.
        self.filled
            .try_send(index)
            .map_err(|_| SlotSendError::Lost)?;
        Ok(value)
    }

    /// Wait until a slot is free. Cancellation consumes nothing.
    pub async fn ready(&mut self) {
        poll_fn(|cx| {
            self.producer.register(cx);
            let (spare, free) = (self.spare.is_some(), &self.free);
            if arm(self.waits.producer, || spare || !free.is_empty()) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }
}

/// The consumer: reads the oldest filled slot in place.
pub struct SlotReceiver<'a, T, const DEPTH: usize, C = AtomicWaker, P = AtomicWaker> {
    slots: &'a [ClaimCell<T>; DEPTH],
    filled: AffineSpscReceiver<'a, u8, DEPTH>,
    free: AffineSpscSender<'a, u8, DEPTH>,
    consumer: &'a C,
    producer: &'a P,
    waits: Waits<'a>,
}

/// Why a receive read no slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlotReceiveError {
    /// No slot is filled.
    Empty,
    /// A slot the indices named was not filled: a broken queue.
    Lost,
}

impl<T, const DEPTH: usize, C: QueueNotify, P: QueueNotify> SlotReceiver<'_, T, DEPTH, C, P> {
    /// Filled slots now.
    pub fn len(&self) -> usize {
        self.filled.len()
    }

    pub fn is_empty(&self) -> bool {
        self.filled.is_empty()
    }

    /// Run `read` on the oldest filled slot, then free it and wake the
    /// producer if it waits. The value stays in its slot for the next fill.
    ///
    /// # Errors
    ///
    /// [`SlotReceiveError`]: nothing filled, or a broken queue.
    pub fn try_receive_with<R>(
        &mut self,
        read: impl FnOnce(&mut T) -> R,
    ) -> Result<R, SlotReceiveError> {
        let index = self
            .filled
            .try_receive()
            .map_err(|_| SlotReceiveError::Empty)?;
        let slot = self
            .slots
            .get(usize::from(index))
            .ok_or(SlotReceiveError::Lost)?;
        let value = slot.lend(read).map_err(|_| SlotReceiveError::Lost)?;
        // The index was filled, so the free queue has room for it.
        self.free
            .try_send(index)
            .map_err(|_| SlotReceiveError::Lost)?;
        let producer = self.producer;
        wake_waiting(self.waits.producer, || producer.notify());
        Ok(value)
    }

    /// Wait for a filled slot without reading it. Cancellation leaves it.
    pub async fn ready(&mut self) {
        self.ready_at_least(1).await;
    }

    /// Wait until `want` slots are filled (at least one, at most `DEPTH`)
    /// without reading them: the producer notifies once its fills reach
    /// that many, not at each. Cancellation leaves them.
    pub async fn ready_at_least(&mut self, want: usize) {
        let want = want.clamp(1, DEPTH);
        poll_fn(|cx| {
            self.consumer.register(cx);
            let filled = &self.filled;
            if arm_wants(self.waits.consumer, want, || filled.len()) {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }
}
