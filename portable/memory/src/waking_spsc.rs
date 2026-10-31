//! Wake policy around the audited affine SPSC queue: one producer and one
//! consumer endpoint, each with its own ordinary atomic waker.

use crate::{
    AffineSpscQueue, AffineSpscReceiver, AffineSpscSendPermit, AffineSpscSender,
    AffineSpscTrySendError,
};
use atomic_waker::AtomicWaker;
use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::task::{Context, Poll};

/// Queue storage and atomic wake state. The composition splits it once and
/// moves the two non-cloneable endpoints to their tasks.
pub struct WakingSpscQueue<T, const DEPTH: usize> {
    queue: AffineSpscQueue<T, DEPTH>,
    consumer: AtomicWaker,
    producer: AtomicWaker,
}

type Endpoints<'a, T, const DEPTH: usize> = (
    WakingSpscSender<'a, T, DEPTH>,
    WakingSpscReceiver<'a, T, DEPTH>,
);

impl<T, const DEPTH: usize> WakingSpscQueue<T, DEPTH> {
    pub const fn new() -> Self {
        Self {
            queue: const { AffineSpscQueue::new() },
            consumer: AtomicWaker::new(),
            producer: AtomicWaker::new(),
        }
    }

    /// Exclusively borrow the storage to grant its sole endpoint pair.
    /// A later pair receives any values left in the queue by its predecessor.
    pub fn split(&mut self) -> Endpoints<'_, T, DEPTH> {
        let (sender, receiver) = self.queue.split();
        (
            WakingSpscSender {
                sender,
                consumer: &self.consumer,
                producer: &self.producer,
            },
            WakingSpscReceiver {
                receiver,
                consumer: &self.consumer,
                producer: &self.producer,
            },
        )
    }
}

impl<T, const DEPTH: usize> Default for WakingSpscQueue<T, DEPTH> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct WakingSpscSender<'a, T, const DEPTH: usize> {
    sender: AffineSpscSender<'a, T, DEPTH>,
    consumer: &'a AtomicWaker,
    producer: &'a AtomicWaker,
}

impl<T, const DEPTH: usize> WakingSpscSender<'_, T, DEPTH> {
    /// Cells free now, observed without reserving one; a concurrent receive
    /// may free another before the next send.
    pub fn free_capacity(&self) -> usize {
        self.sender.free_capacity()
    }

    /// A full queue returns the original value and all its ownership.
    pub fn try_send(&self, value: T) -> Result<(), AffineSpscTrySendError<T>> {
        self.sender.try_send(value)?;
        self.consumer.wake();
        Ok(())
    }

    /// EXPERIMENT: see [`AffineSpscSender::try_send_with`].
    pub fn try_send_with(
        &self,
        init: impl FnOnce(&mut core::mem::MaybeUninit<T>) -> bool,
    ) -> Option<bool> {
        let sent = self.sender.try_send_with(init)?;
        if sent {
            self.consumer.wake();
        }
        Some(sent)
    }

    /// Wait for capacity while retaining the sole producer wake registration.
    /// The returned permit keeps the exclusive sender borrow and publishes
    /// without another capacity check. The caller owns its value until send;
    /// cancelling the wait or dropping a permit consumes no credit.
    ///
    /// ```compile_fail,E0499
    /// use oer_memory::WakingSpscQueue;
    /// let mut queue = WakingSpscQueue::<u8, 1>::new();
    /// let (mut sender, receiver) = queue.split();
    /// let first = sender.ready();
    /// let second = sender.ready();
    /// drop((first, second));
    /// ```
    ///
    /// ```
    /// use oer_memory::WakingSpscQueue;
    /// async fn sequential() {
    ///     let mut queue = WakingSpscQueue::<u8, 1>::new();
    ///     let (mut sender, receiver) = queue.split();
    ///     sender.ready().await.send(1);
    ///     assert_eq!(receiver.try_receive(), Some(1));
    ///     drop(sender.ready().await);
    /// }
    /// ```
    pub async fn ready(&mut self) -> WakingSpscSendPermit<'_, T, DEPTH> {
        SendReady {
            sender: Some(&mut self.sender),
            consumer: self.consumer,
            producer: self.producer,
        }
        .await
    }
}

/// Exclusive capacity obtained by [`WakingSpscSender::ready`].
///
/// ```compile_fail,E0502
/// use oer_memory::WakingSpscQueue;
/// async fn cannot_publish_twice() {
///     let mut queue = WakingSpscQueue::<u8, 1>::new();
///     let (mut sender, receiver) = queue.split();
///     let permit = sender.ready().await;
///     sender.try_send(1).unwrap();
///     permit.send(2);
/// }
/// ```
#[must_use = "send a value or drop the permit to release the sender borrow"]
pub struct WakingSpscSendPermit<'sender, T, const DEPTH: usize> {
    permit: AffineSpscSendPermit<'sender, T, DEPTH>,
    consumer: &'sender AtomicWaker,
}

impl<T, const DEPTH: usize> WakingSpscSendPermit<'_, T, DEPTH> {
    /// Publish once and wake the consumer. The reserved cell cannot be full.
    pub fn send(self, value: T) {
        self.permit.send(value);
        self.consumer.wake();
    }
}

struct SendReady<'sender, 'queue, T, const DEPTH: usize> {
    // None only after returning the permit; repolling a completed wait parks.
    sender: Option<&'sender mut AffineSpscSender<'queue, T, DEPTH>>,
    consumer: &'sender AtomicWaker,
    producer: &'sender AtomicWaker,
}

impl<'sender, T, const DEPTH: usize> Future for SendReady<'sender, '_, T, DEPTH> {
    type Output = WakingSpscSendPermit<'sender, T, DEPTH>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let Some(sender) = this.sender.take() else {
            return Poll::Pending;
        };
        // Reclamation before registration is seen by the Acquire load in
        // try_reserve; reclamation after it wakes this sole registration.
        this.producer.register(cx.waker());
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

pub struct WakingSpscReceiver<'a, T, const DEPTH: usize> {
    receiver: AffineSpscReceiver<'a, T, DEPTH>,
    consumer: &'a AtomicWaker,
    producer: &'a AtomicWaker,
}

impl<T, const DEPTH: usize> WakingSpscReceiver<'_, T, DEPTH> {
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
        self.producer.wake();
        Some(value)
    }

    /// EXPERIMENT: see [`AffineSpscReceiver::try_receive_with`].
    pub fn try_receive_with<R>(&self, use_value: impl FnOnce(&mut T) -> R) -> Option<R> {
        let result = self.receiver.try_receive_with(use_value)?;
        self.producer.wake();
        Some(result)
    }

    /// Wait for a value without consuming it. The exclusive borrow lasts for
    /// the entire future, so the queue's sole consumer waker cannot be replaced
    /// by another outstanding wait. Cancellation leaves the value in the queue.
    ///
    /// ```compile_fail,E0499
    /// use oer_memory::WakingSpscQueue;
    /// let mut queue = WakingSpscQueue::<u8, 1>::new();
    /// let (sender, mut receiver) = queue.split();
    /// let first = receiver.ready();
    /// let second = receiver.ready();
    /// drop((first, second));
    /// ```
    ///
    /// ```
    /// use oer_memory::WakingSpscQueue;
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
        poll_fn(|cx| {
            self.consumer.register(cx.waker());
            if self.receiver.is_empty() {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
    }
}

#[cfg(test)]
mod tests;
