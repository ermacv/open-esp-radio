//! A waking SPSC queue whose consumer closes its reception
//! ([`ClosingSpscReceiver::close`]): once the close completes, no
//! publication is in progress and none follows, so what the queue holds
//! then is everything it ever will hold. A later send, and a permit issued
//! before the close, return their value whole
//! ([`ClosingSpscSendError::Closed`], [`ClosingSpscClosed`]). A queue that
//! never closes is a [`WakingSpscQueue`]: its sends have no closed refusal.

use super::{
    QueueNotify, WakingSpscQueue, WakingSpscReceiver, WakingSpscSendPermit, WakingSpscSender,
};
use crate::AffineSpscTrySendError;
use core::future::poll_fn;
use core::sync::atomic::{AtomicU8, Ordering};
use core::task::{Context, Poll};
use oer_portable_waker::AtomicWaker;

/// A publication is in progress: the closing consumer waits for its end.
const SENDING: u8 = 1;
/// The consumer closed its reception: no publication starts any more.
const CLOSED: u8 = 2;

/// Why a send did not publish: its value comes back whole.
#[derive(Debug, Eq, PartialEq)]
pub enum ClosingSpscSendError<T> {
    /// No cell is free now: a receive frees one.
    Full(T),
    /// The consumer closed its reception: nothing receives it, ever.
    Closed(T),
}

impl<T> ClosingSpscSendError<T> {
    /// The value the send did not publish.
    pub fn into_inner(self) -> T {
        match self {
            Self::Full(value) | Self::Closed(value) => value,
        }
    }
}

/// A permit's value, returned whole: the consumer closed its reception
/// before the permit published.
#[derive(Debug, Eq, PartialEq)]
pub struct ClosingSpscClosed<T>(pub T);

/// Queue storage, its notifies and its reception state.
pub struct ClosingSpscQueue<T, const DEPTH: usize, C = AtomicWaker, P = AtomicWaker> {
    queue: WakingSpscQueue<T, DEPTH, C, P>,
    /// [`SENDING`] while the producer publishes, [`CLOSED`] once the
    /// consumer closed its reception.
    state: AtomicU8,
}

type Endpoints<'a, T, const DEPTH: usize, C, P> = (
    ClosingSpscSender<'a, T, DEPTH, C, P>,
    ClosingSpscReceiver<'a, T, DEPTH, C, P>,
);

impl<T, const DEPTH: usize> ClosingSpscQueue<T, DEPTH> {
    pub const fn new() -> Self {
        Self {
            queue: WakingSpscQueue::new(),
            state: AtomicU8::new(0),
        }
    }
}

impl<T, const DEPTH: usize> Default for ClosingSpscQueue<T, DEPTH> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T, const DEPTH: usize, C: QueueNotify, P: QueueNotify> ClosingSpscQueue<T, DEPTH, C, P> {
    /// Exclusively borrow the storage to grant its sole endpoint pair, its
    /// reception open. A later pair receives any values left in the queue
    /// by its predecessor.
    pub fn split(&mut self) -> Endpoints<'_, T, DEPTH, C, P> {
        *self.state.get_mut() = 0;
        let (sender, receiver) = self.queue.split();
        let state = &self.state;
        (
            ClosingSpscSender { sender, state },
            ClosingSpscReceiver { receiver, state },
        )
    }
}

/// One publication under the closing protocol: the in-progress mark first,
/// then a closed reception refuses before any cell is written; otherwise
/// `publish` runs. The mark ends after it, and a consumer that closed
/// meanwhile is woken.
fn publishing<U, R>(
    state: &AtomicU8,
    consumer: &impl QueueNotify,
    value: U,
    publish: impl FnOnce(U) -> Result<R, ClosingSpscSendError<U>>,
) -> Result<R, ClosingSpscSendError<U>> {
    let prior = state.fetch_or(SENDING, Ordering::SeqCst);
    let result = if prior & CLOSED == 0 {
        publish(value)
    } else {
        Err(ClosingSpscSendError::Closed(value))
    };
    if state.fetch_and(!SENDING, Ordering::SeqCst) & CLOSED != 0 {
        consumer.notify();
    }
    result
}

/// The producer endpoint of a [`ClosingSpscQueue`].
pub struct ClosingSpscSender<'a, T, const DEPTH: usize, C = AtomicWaker, P = AtomicWaker> {
    sender: WakingSpscSender<'a, T, DEPTH, C, P>,
    state: &'a AtomicU8,
}

impl<T, const DEPTH: usize, C: QueueNotify, P: QueueNotify> ClosingSpscSender<'_, T, DEPTH, C, P> {
    /// Cells free now ([`WakingSpscSender::free_capacity`]).
    pub fn free_capacity(&self) -> usize {
        self.sender.free_capacity()
    }

    /// Whether the consumer closed its reception: every send refuses.
    pub fn is_closed(&self) -> bool {
        self.state.load(Ordering::SeqCst) & CLOSED != 0
    }

    /// [`WakingSpscSender::try_send`].
    ///
    /// # Errors
    ///
    /// A full queue, or one whose reception closed: the value comes back
    /// with all its ownership.
    pub fn try_send(&self, value: T) -> Result<(), ClosingSpscSendError<T>> {
        let (sender, consumer) = (&self.sender, self.sender.consumer);
        publishing(self.state, consumer, value, |value| {
            sender.try_send(value).map_err(full)
        })
    }

    /// [`WakingSpscSender::try_send_held`].
    ///
    /// # Errors
    ///
    /// As [`Self::try_send`].
    pub fn try_send_held(&self, value: T) -> Result<(), ClosingSpscSendError<T>> {
        let (sender, consumer) = (&self.sender, self.sender.consumer);
        publishing(self.state, consumer, value, |value| {
            sender.try_send_held(value).map_err(full)
        })
    }

    /// [`WakingSpscSender::flush`].
    pub fn flush(&self) {
        self.sender.flush();
    }

    /// [`WakingSpscSender::try_reserve`]. The permit refuses its value
    /// whole once the consumer closed its reception meanwhile.
    pub fn try_reserve(&mut self) -> Option<ClosingSpscSendPermit<'_, T, DEPTH, C>> {
        let state = self.state;
        let permit = self.sender.try_reserve()?;
        Some(ClosingSpscSendPermit { permit, state })
    }

    /// [`WakingSpscSender::poll_room`].
    pub fn poll_room(&mut self, cx: &mut Context<'_>) -> Poll<()> {
        self.sender.poll_room(cx)
    }

    /// [`WakingSpscSender::ready`]. The permit refuses its value whole once
    /// the consumer closed its reception meanwhile.
    pub async fn ready(&mut self) -> ClosingSpscSendPermit<'_, T, DEPTH, C> {
        let state = self.state;
        let permit = self.sender.ready().await;
        ClosingSpscSendPermit { permit, state }
    }
}

fn full<T>(AffineSpscTrySendError(value): AffineSpscTrySendError<T>) -> ClosingSpscSendError<T> {
    ClosingSpscSendError::Full(value)
}

/// Exclusive capacity of a [`ClosingSpscSender`].
#[must_use = "send a value or drop the permit to release the sender borrow"]
pub struct ClosingSpscSendPermit<'sender, T, const DEPTH: usize, C = AtomicWaker> {
    permit: WakingSpscSendPermit<'sender, T, DEPTH, C>,
    state: &'sender AtomicU8,
}

impl<T, const DEPTH: usize, C: QueueNotify> ClosingSpscSendPermit<'_, T, DEPTH, C> {
    /// Publish once and wake the consumer.
    ///
    /// # Errors
    ///
    /// The consumer closed its reception since the permit was issued: the
    /// value comes back whole, and nothing is published.
    pub fn send(self, value: T) -> Result<(), ClosingSpscClosed<T>> {
        self.send_with(value, |value| value)
    }

    /// Publish `wrap(value)` once and wake the consumer: `wrap` runs only
    /// when the reception is open, so a refusal returns `value` itself.
    ///
    /// # Errors
    ///
    /// As [`Self::send`]: `value` comes back whole, unwrapped.
    pub fn send_with<U>(
        self,
        value: U,
        wrap: impl FnOnce(U) -> T,
    ) -> Result<(), ClosingSpscClosed<U>> {
        let Self { permit, state } = self;
        let consumer = permit.consumer;
        publishing(state, consumer, value, |value| {
            permit.send(wrap(value));
            Ok(())
        })
        .map_err(|refused| ClosingSpscClosed(refused.into_inner()))
    }
}

/// The consumer endpoint of a [`ClosingSpscQueue`].
pub struct ClosingSpscReceiver<'a, T, const DEPTH: usize, C = AtomicWaker, P = AtomicWaker> {
    receiver: WakingSpscReceiver<'a, T, DEPTH, C, P>,
    state: &'a AtomicU8,
}

impl<T, const DEPTH: usize, C: QueueNotify, P: QueueNotify>
    ClosingSpscReceiver<'_, T, DEPTH, C, P>
{
    /// Current queued owners.
    pub fn len(&self) -> usize {
        self.receiver.len()
    }

    pub fn is_empty(&self) -> bool {
        self.receiver.is_empty()
    }

    /// [`WakingSpscReceiver::try_receive`]; the values queued before a
    /// close stay receivable, in order.
    #[must_use]
    pub fn try_receive(&self) -> Option<T> {
        self.receiver.try_receive()
    }

    /// [`WakingSpscReceiver::ready`].
    pub async fn ready(&mut self) {
        self.receiver.ready().await;
    }

    /// Close the reception: from the mark on, no publication starts, and
    /// this ends once a publication already in progress ended (its end
    /// wakes the wait). Then what the queue holds is all it ever will hold,
    /// and every later send or permit refuses its value whole.
    pub async fn close(&mut self) {
        self.state.fetch_or(CLOSED, Ordering::SeqCst);
        let (state, consumer) = (self.state, self.receiver.consumer);
        poll_fn(|cx| {
            // Registered before the check: a publication ending after it
            // notifies this registration.
            consumer.register(cx);
            if state.load(Ordering::SeqCst) & SENDING == 0 {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
        .await;
    }

    /// Whether the reception was closed.
    pub fn is_closed(&self) -> bool {
        self.state.load(Ordering::SeqCst) & CLOSED != 0
    }
}

#[cfg(test)]
mod tests;
