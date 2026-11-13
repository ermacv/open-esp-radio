//! Wake policy around the audited affine SPSC queue: one producer and one
//! consumer endpoint, each with its own notify, by default an ordinary
//! atomic waker. A freed cell tells the producer only while it waits; a
//! producer that knows where its burst ends notifies the consumer once for
//! it ([`WakingSpscSender::try_send_held`], [`WakingSpscSender::flush`]).

use crate::{
    AffineSpscQueue, AffineSpscReceiver, AffineSpscSendPermit, AffineSpscSender,
    AffineSpscTrySendError,
};
use core::cell::Cell;
use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::sync::atomic::{AtomicBool, Ordering, fence};
use core::task::{Context, Poll};
use oer_portable_waker::AtomicWaker;

/// How one end of a queue waits for its peer and how the peer tells it.
///
/// An end on the same hart as its peer uses an [`AtomicWaker`], the
/// default. An end whose peer runs on another hart must not be woken by a
/// waker (no waker crosses harts): the composition gives the queue a notify
/// that raises the waiting end's executor signal instead. A queue names its
/// notifies' types, so every call is direct.
pub trait QueueNotify: Sync {
    /// Make the polling task the one [`Self::notify`] wakes. The caller then
    /// checks the queue, so a notify before this registration is found.
    fn register(&self, cx: &mut Context<'_>);
    /// Tell the registered end that the queue changed, after publishing.
    fn notify(&self);
}

/// A notify borrowed from where several queues share it.
impl<N: QueueNotify + ?Sized> QueueNotify for &N {
    fn register(&self, cx: &mut Context<'_>) {
        N::register(self, cx);
    }
    fn notify(&self) {
        N::notify(self);
    }
}

impl QueueNotify for AtomicWaker {
    fn register(&self, cx: &mut Context<'_>) {
        AtomicWaker::register(self, cx.waker());
    }
    fn notify(&self) {
        self.wake();
    }
}

impl QueueNotify for oer_portable_sync::PriorityWaker {
    fn register(&self, cx: &mut Context<'_>) {
        oer_portable_sync::PriorityWaker::register(self, cx.waker());
    }
    fn notify(&self) {
        self.wake();
    }
}

/// Queue storage and its two notifies: `C` tells the consumer that a value
/// was published, `P` the producer that a cell was freed. The composition
/// splits it once and moves the two non-cloneable endpoints to their tasks.
pub struct WakingSpscQueue<T, const DEPTH: usize, C = AtomicWaker, P = AtomicWaker> {
    queue: AffineSpscQueue<T, DEPTH>,
    consumer: C,
    producer: P,
    /// The producer found no free cell after its registration.
    producer_waits: AtomicBool,
}

type Endpoints<'a, T, const DEPTH: usize, C, P> = (
    WakingSpscSender<'a, T, DEPTH, C, P>,
    WakingSpscReceiver<'a, T, DEPTH, C, P>,
);

impl<T, const DEPTH: usize> WakingSpscQueue<T, DEPTH> {
    pub const fn new() -> Self {
        Self::across_harts(AtomicWaker::new(), AtomicWaker::new())
    }
}

impl<T, const DEPTH: usize, C: QueueNotify, P: QueueNotify> WakingSpscQueue<T, DEPTH, C, P> {
    /// A queue whose ends wake through `consumer` when a value is
    /// published and through `producer` when a cell is freed: for ends on
    /// different harts, each the waiting end's executor signal.
    pub const fn across_harts(consumer: C, producer: P) -> Self {
        Self {
            queue: const { AffineSpscQueue::new() },
            consumer,
            producer,
            producer_waits: AtomicBool::new(false),
        }
    }

    /// Exclusively borrow the storage to grant its sole endpoint pair.
    /// A later pair receives any values left in the queue by its predecessor.
    pub fn split(&mut self) -> Endpoints<'_, T, DEPTH, C, P> {
        let (sender, receiver) = self.queue.split();
        let (consumer, producer) = (&self.consumer, &self.producer);
        let producer_waits = &self.producer_waits;
        (
            WakingSpscSender {
                sender,
                consumer,
                producer,
                producer_waits,
                held: Cell::new(false),
            },
            WakingSpscReceiver {
                receiver,
                consumer,
                producer,
                producer_waits,
            },
        )
    }
}

impl<T, const DEPTH: usize> Default for WakingSpscQueue<T, DEPTH> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct WakingSpscSender<'a, T, const DEPTH: usize, C = AtomicWaker, P = AtomicWaker> {
    sender: AffineSpscSender<'a, T, DEPTH>,
    consumer: &'a C,
    producer: &'a P,
    producer_waits: &'a AtomicBool,
    /// A value waits for [`Self::flush`] to notify the consumer.
    held: Cell<bool>,
}

impl<T, const DEPTH: usize, C: QueueNotify, P: QueueNotify> WakingSpscSender<'_, T, DEPTH, C, P> {
    /// Cells free now, observed without reserving one; a concurrent receive
    /// may free another before the next send.
    pub fn free_capacity(&self) -> usize {
        self.sender.free_capacity()
    }

    /// A full queue returns the original value and all its ownership.
    pub fn try_send(&self, value: T) -> Result<(), AffineSpscTrySendError<T>> {
        self.sender.try_send(value)?;
        self.consumer.notify();
        Ok(())
    }

    /// [`Self::try_send`] within a burst: the consumer is notified once, by
    /// the burst's [`Self::flush`], so ends on different harts exchange one
    /// signal per burst, not one per value.
    pub fn try_send_held(&self, value: T) -> Result<(), AffineSpscTrySendError<T>> {
        self.sender.try_send(value)?;
        self.held.set(true);
        Ok(())
    }

    /// Build a value at a vacant cell, deferring its notification to
    /// [`Self::flush`]. A full queue runs no builder and returns `false`;
    /// a builder returning `None` also publishes nothing.
    pub fn try_send_held_with(&mut self, make: impl FnOnce() -> Option<T>) -> bool {
        let Ok(permit) = self.sender.try_reserve() else {
            return false;
        };
        let sent = permit.send_with(make);
        if sent {
            self.held.set(true);
        }
        sent
    }

    /// End a burst: notify the consumer if a held value waits for it.
    pub fn flush(&self) {
        if self.held.take() {
            self.consumer.notify();
        }
    }

    /// Reserve one vacant cell now, without waiting or registering a wake;
    /// `None` while the queue is full. The caller keeps its value until it
    /// sends through the permit.
    pub fn try_reserve(&mut self) -> Option<WakingSpscSendPermit<'_, T, DEPTH, C>> {
        let permit = self.sender.try_reserve().ok()?;
        Some(WakingSpscSendPermit {
            permit,
            consumer: self.consumer,
        })
    }

    /// Poll for a free cell without reserving one: ready while a cell is
    /// free. Otherwise this endpoint's one producer registration is armed as
    /// [`Self::ready`] arms it (register, mark the wait, fence, then check
    /// the queue again), so a cell freed after that check wakes the task,
    /// and nothing else does. It takes no cell and publishes nothing; the
    /// caller keeps its value. The exclusive borrow keeps one waiter per
    /// endpoint: no [`Self::ready`] wait is outstanding meanwhile.
    pub fn poll_room(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        // As in `SendReady`: a reclamation before the flag is found by the
        // check below; one after it sees the flag and wakes this
        // registration.
        self.producer.register(cx);
        self.producer_waits.store(true, Ordering::SeqCst);
        fence(Ordering::SeqCst);
        if self.sender.free_capacity() == 0 {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }

    /// Wait for capacity while retaining the sole producer wake registration.
    /// The returned permit keeps the exclusive sender borrow and publishes
    /// without another capacity check. The caller owns its value until send;
    /// cancelling the wait or dropping a permit consumes no credit.
    ///
    /// ```compile_fail,E0499
    /// use oer_portable_memory::WakingSpscQueue;
    /// let mut queue = WakingSpscQueue::<u8, 1>::new();
    /// let (mut sender, receiver) = queue.split();
    /// let first = sender.ready();
    /// let second = sender.ready();
    /// drop((first, second));
    /// ```
    ///
    /// ```
    /// use oer_portable_memory::WakingSpscQueue;
    /// async fn sequential() {
    ///     let mut queue = WakingSpscQueue::<u8, 1>::new();
    ///     let (mut sender, receiver) = queue.split();
    ///     sender.ready().await.send(1);
    ///     assert_eq!(receiver.try_receive(), Some(1));
    ///     drop(sender.ready().await);
    /// }
    /// ```
    pub async fn ready(&mut self) -> WakingSpscSendPermit<'_, T, DEPTH, C> {
        SendReady {
            sender: Some(&mut self.sender),
            consumer: self.consumer,
            producer: self.producer,
            producer_waits: self.producer_waits,
        }
        .await
    }
}

/// Exclusive capacity obtained by [`WakingSpscSender::ready`].
///
/// ```compile_fail,E0502
/// use oer_portable_memory::WakingSpscQueue;
/// async fn cannot_publish_twice() {
///     let mut queue = WakingSpscQueue::<u8, 1>::new();
///     let (mut sender, receiver) = queue.split();
///     let permit = sender.ready().await;
///     sender.try_send(1).unwrap();
///     permit.send(2);
/// }
/// ```
#[must_use = "send a value or drop the permit to release the sender borrow"]
pub struct WakingSpscSendPermit<'sender, T, const DEPTH: usize, C = AtomicWaker> {
    permit: AffineSpscSendPermit<'sender, T, DEPTH>,
    consumer: &'sender C,
}

impl<T, const DEPTH: usize, C: QueueNotify> WakingSpscSendPermit<'_, T, DEPTH, C> {
    /// Publish once and wake the consumer. The reserved cell cannot be full.
    pub fn send(self, value: T) {
        self.permit.send(value);
        self.consumer.notify();
    }
}

struct SendReady<'sender, 'queue, T, const DEPTH: usize, C, P> {
    // None only after returning the permit; repolling a completed wait parks.
    sender: Option<&'sender mut AffineSpscSender<'queue, T, DEPTH>>,
    consumer: &'sender C,
    producer: &'sender P,
    producer_waits: &'sender AtomicBool,
}

impl<'sender, T, const DEPTH: usize, C: QueueNotify, P: QueueNotify> Future
    for SendReady<'sender, '_, T, DEPTH, C, P>
{
    type Output = WakingSpscSendPermit<'sender, T, DEPTH, C>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let Some(sender) = this.sender.take() else {
            return Poll::Pending;
        };
        // Reclamation before the flag is seen by the Acquire load in
        // try_reserve; reclamation after it sees the flag and wakes this
        // sole registration. A ready poll keeps the flag: its caller may
        // wait for more than this cell, and the next reclamation clears it.
        this.producer.register(cx);
        this.producer_waits.store(true, Ordering::SeqCst);
        fence(Ordering::SeqCst);
        match sender.try_reserve() {
            Ok(permit) => Poll::Ready(WakingSpscSendPermit {
                permit,
                consumer: this.consumer,
            }),
            Err(sender) => {
                this.sender = Some(sender);
                Poll::Pending
            }
        }
    }
}

pub struct WakingSpscReceiver<'a, T, const DEPTH: usize, C = AtomicWaker, P = AtomicWaker> {
    receiver: AffineSpscReceiver<'a, T, DEPTH>,
    consumer: &'a C,
    producer: &'a P,
    producer_waits: &'a AtomicBool,
}

struct PrefixFreed<'a, P: QueueNotify> {
    producer: &'a P,
    producer_waits: &'a AtomicBool,
    freed: bool,
}

impl<P: QueueNotify> Drop for PrefixFreed<'_, P> {
    fn drop(&mut self) {
        if self.freed {
            fence(Ordering::SeqCst);
            if self.producer_waits.swap(false, Ordering::SeqCst) {
                self.producer.notify();
            }
        }
    }
}

impl<T, const DEPTH: usize, C: QueueNotify, P: QueueNotify> WakingSpscReceiver<'_, T, DEPTH, C, P> {
    /// Current queued owners, observed without consuming one or changing wakes.
    pub fn len(&self) -> usize {
        self.receiver.len()
    }

    pub fn is_empty(&self) -> bool {
        self.receiver.is_empty()
    }

    #[must_use]
    pub fn try_receive(&self) -> Option<T> {
        let value = self.receiver.try_receive().ok()?;
        // Only a producer waiting for a cell hears of it: across harts a
        // notify is an interrupt.
        fence(Ordering::SeqCst);
        if self.producer_waits.swap(false, Ordering::SeqCst) {
            self.producer.notify();
        }
        Some(value)
    }

    /// Consume the prefix published now, in place, freeing its cells and
    /// telling a waiting producer once. See [`AffineSpscReceiver::consume_prefix`]
    /// for ownership, early exit and unwind behavior.
    pub fn consume_prefix(&mut self, limit: usize, mut each: impl FnMut(&mut T) -> bool) -> usize {
        let mut freed = PrefixFreed {
            producer: self.producer,
            producer_waits: self.producer_waits,
            freed: false,
        };
        self.receiver.consume_prefix(limit, |value| {
            let more = each(value);
            freed.freed = true;
            more
        })
    }

    /// Register the sole consumer before checking its queue. As with
    /// [`Self::ready`], only one task uses this endpoint's registration.
    pub fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<()> {
        self.consumer.register(cx);
        if self.receiver.is_empty() {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }

    /// Wait for a value without consuming it. The exclusive borrow lasts for
    /// the entire future, so the queue's sole consumer waker cannot be replaced
    /// by another outstanding wait. Cancellation leaves the value in the queue.
    ///
    /// ```compile_fail,E0499
    /// use oer_portable_memory::WakingSpscQueue;
    /// let mut queue = WakingSpscQueue::<u8, 1>::new();
    /// let (sender, mut receiver) = queue.split();
    /// let first = receiver.ready();
    /// let second = receiver.ready();
    /// drop((first, second));
    /// ```
    ///
    /// ```
    /// use oer_portable_memory::WakingSpscQueue;
    /// async fn sequential() {
    ///     let mut queue = WakingSpscQueue::<u8, 1>::new();
    ///     let (sender, mut receiver) = queue.split();
    ///     sender.try_send(1).unwrap();
    ///     receiver.ready().await;
    ///     assert_eq!(receiver.try_receive(), Some(1));
    /// }
    /// ```
    pub async fn ready(&mut self) {
        // Register before testing the queue: a publication before registration
        // is found by the load; a later one wakes this registration.
        poll_fn(|cx| self.poll_ready(cx)).await;
    }
}

mod closing;
pub use closing::{
    ClosingSpscClosed, ClosingSpscQueue, ClosingSpscReceiver, ClosingSpscReceptionClosed,
    ClosingSpscSendError, ClosingSpscSendPermit, ClosingSpscSender,
};

#[cfg(test)]
mod tests;
