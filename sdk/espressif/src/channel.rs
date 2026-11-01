//! A typed channel between the application's future and the second boot's:
//! `A` values go from the application to the second boot's future, `B`
//! values back, each way a queue of `N`.
//!
//! The image declares the channel once as a static. The boot takes the
//! application's end, which splits both queues and keeps the other end;
//! the second boot takes that end. Each end is taken once. An end stays
//! with the future that polls it (`!Send`): its waits wake that one task.

use core::marker::PhantomData;

use atomic_waker::AtomicWaker;
use oer_memory::{ClaimCell, QueueNotify, WakingSpscQueue, WakingSpscReceiver, WakingSpscSender};
use static_cell::ConstStaticCell;

/// The two queues of a channel, and their notifies: `C` the application
/// end's, `S` the second end's.
type Queues<A, B, const N: usize, C, S> =
    (WakingSpscQueue<A, N, S, C>, WakingSpscQueue<B, N, C, S>);

/// The halves the second end takes, kept in the channel until it does.
type SecondHalves<A, B, const N: usize, C, S> = (
    WakingSpscReceiver<'static, A, N, S, C>,
    WakingSpscSender<'static, B, N, C, S>,
);

/// A channel of `A` to the second boot and `B` back, `N` deep each way.
pub struct Channel<
    A: 'static,
    B: 'static,
    const N: usize,
    C: 'static = AtomicWaker,
    S: 'static = AtomicWaker,
> {
    queues: ConstStaticCell<Queues<A, B, N, C, S>>,
    second: ClaimCell<SecondHalves<A, B, N, C, S>>,
}

/// Why a channel refused an end.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelError {
    /// The end was taken already.
    Taken,
    /// The second end was asked for before the application's end split the
    /// channel.
    NotSplit,
}

impl<A, B, const N: usize> Channel<A, B, N> {
    /// A channel whose ends run on one hart.
    pub const fn new() -> Self {
        Self {
            queues: ConstStaticCell::new((WakingSpscQueue::new(), WakingSpscQueue::new())),
            second: ClaimCell::new(),
        }
    }
}

impl<A, B, const N: usize, C: QueueNotify + Copy, S: QueueNotify + Copy> Channel<A, B, N, C, S> {
    /// A channel whose ends run on two harts: `application` tells the
    /// application's end, `second` the second boot's, each the waiting
    /// task's executor signal.
    pub const fn across_harts(application: C, second: S) -> Self {
        Self {
            queues: ConstStaticCell::new((
                WakingSpscQueue::across_harts(second, application),
                WakingSpscQueue::across_harts(application, second),
            )),
            second: ClaimCell::new(),
        }
    }
}

impl<A, B, const N: usize> Default for Channel<A, B, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<A: Send, B: Send, const N: usize, C: QueueNotify, S: QueueNotify> Channel<A, B, N, C, S>
where
    SecondHalves<A, B, N, C, S>: Send,
{
    /// The application's end: split both queues and keep the second end.
    pub(crate) fn take_application(&'static self) -> Result<End<A, B, N, C, S>, ChannelError> {
        let (to_second, from_second) = self.queues.try_take().ok_or(ChannelError::Taken)?;
        let (send, receive_there) = to_second.split();
        let (send_there, receive) = from_second.split();
        if self.second.install((receive_there, send_there)).is_err() {
            return Err(ChannelError::Taken);
        }
        Ok(End {
            send,
            receive,
            _local: PhantomData,
        })
    }

    /// The second boot's end.
    pub(crate) fn take_second(&'static self) -> Result<End<B, A, N, S, C>, ChannelError> {
        let (receive, send) = self.second.take().map_err(|refusal| match refusal {
            oer_memory::Claim::Empty => ChannelError::NotSplit,
            oer_memory::Claim::Lent | oer_memory::Claim::Kept => ChannelError::Taken,
        })?;
        Ok(End {
            send,
            receive,
            _local: PhantomData,
        })
    }
}

/// One end of a channel: it sends `O` and receives `I`. `W` is the notify
/// its own waits register with, `P` its peer's.
pub struct End<O: 'static, I: 'static, const N: usize, W: 'static, P: 'static> {
    send: WakingSpscSender<'static, O, N, P, W>,
    receive: WakingSpscReceiver<'static, I, N, W, P>,
    _local: PhantomData<*mut ()>,
}

impl<O, I, const N: usize, W: QueueNotify, P: QueueNotify> End<O, I, N, W, P> {
    /// Send `value` once the queue has room.
    pub async fn send(&mut self, value: O) {
        self.send.ready().await.send(value);
    }

    /// Send `value` now.
    ///
    /// # Errors
    /// The queue is full: `value` comes back.
    pub fn try_send(&self, value: O) -> Result<(), O> {
        self.send.try_send(value).map_err(|full| full.0)
    }

    /// The next value the other end sent, once there is one.
    pub async fn receive(&mut self) -> I {
        loop {
            if let Some(value) = self.receive.try_receive() {
                return value;
            }
            self.receive.ready().await;
        }
    }

    /// The next value the other end sent, if there is one now.
    pub fn try_receive(&self) -> Option<I> {
        self.receive.try_receive()
    }
}

#[cfg(test)]
mod tests;
