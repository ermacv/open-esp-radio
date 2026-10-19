//! Runtime wake policy around the audited affine SPSC queue.

use atomic_waker::AtomicWaker;
use core::future::{Future, poll_fn};
use core::pin::Pin;
use core::task::{Context, Poll};
use oer_memory::{
    AffineSpscQueue, AffineSpscReceiver, AffineSpscSendPermit, AffineSpscSender,
    AffineSpscTrySendError,
};

/// Queue storage and atomic wake state. The composition splits it once and
/// moves the two non-cloneable endpoints to the application and the service.
/// The service uses it for both network MSDUs and application commands.
pub struct MsduQueue<T, const DEPTH: usize> {
    queue: AffineSpscQueue<T, DEPTH>,
    consumer: AtomicWaker,
    producer: AtomicWaker,
}

type Endpoints<'a, T, const DEPTH: usize> = (MsduSender<'a, T, DEPTH>, MsduReceiver<'a, T, DEPTH>);

impl<T, const DEPTH: usize> MsduQueue<T, DEPTH> {
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
            MsduSender {
                sender,
                consumer: &self.consumer,
                producer: &self.producer,
            },
            MsduReceiver {
                receiver,
                consumer: &self.consumer,
                producer: &self.producer,
            },
        )
    }
}

impl<T, const DEPTH: usize> Default for MsduQueue<T, DEPTH> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct MsduSender<'a, T, const DEPTH: usize> {
    sender: AffineSpscSender<'a, T, DEPTH>,
    consumer: &'a AtomicWaker,
    producer: &'a AtomicWaker,
}

impl<T, const DEPTH: usize> MsduSender<'_, T, DEPTH> {
    /// A full queue returns the original value and all its ownership.
    pub fn try_send(&self, value: T) -> Result<(), AffineSpscTrySendError<T>> {
        self.sender.try_send(value)?;
        self.consumer.wake();
        Ok(())
    }

    /// Wait for capacity while retaining the sole producer wake registration.
    /// The returned permit keeps the exclusive sender borrow and publishes
    /// without another capacity check. The caller owns its value until send;
    /// cancelling the wait or dropping a permit consumes no credit.
    ///
    /// ```compile_fail,E0499
    /// use oer_ieee80211_runtime::station_service::MsduQueue;
    /// let mut queue = MsduQueue::<u8, 1>::new();
    /// let (mut sender, receiver) = queue.split();
    /// let first = sender.ready();
    /// let second = sender.ready();
    /// drop((first, second));
    /// ```
    ///
    /// ```
    /// use oer_ieee80211_runtime::station_service::MsduQueue;
    /// async fn sequential() {
    ///     let mut queue = MsduQueue::<u8, 1>::new();
    ///     let (mut sender, receiver) = queue.split();
    ///     sender.ready().await.send(1);
    ///     assert_eq!(receiver.try_receive(), Some(1));
    ///     drop(sender.ready().await);
    /// }
    /// ```
    pub async fn ready(&mut self) -> SendPermit<'_, T, DEPTH> {
        SendReady {
            sender: Some(&mut self.sender),
            consumer: self.consumer,
            producer: self.producer,
        }
        .await
    }
}

/// Exclusive capacity obtained by [`MsduSender::ready`].
///
/// ```compile_fail,E0502
/// use oer_ieee80211_runtime::station_service::MsduQueue;
/// async fn cannot_publish_twice() {
///     let mut queue = MsduQueue::<u8, 1>::new();
///     let (mut sender, receiver) = queue.split();
///     let permit = sender.ready().await;
///     sender.try_send(1).unwrap();
///     permit.send(2);
/// }
/// ```
#[must_use = "send a value or drop the permit to release the sender borrow"]
pub struct SendPermit<'sender, T, const DEPTH: usize> {
    permit: AffineSpscSendPermit<'sender, T, DEPTH>,
    consumer: &'sender AtomicWaker,
}

impl<T, const DEPTH: usize> SendPermit<'_, T, DEPTH> {
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
    type Output = SendPermit<'sender, T, DEPTH>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.get_mut();
        let Some(sender) = this.sender.take() else {
            return Poll::Pending;
        };
        // Reclamation before registration is seen by the Acquire load in
        // try_reserve; reclamation after it wakes this sole registration.
        this.producer.register(cx.waker());
        match sender.try_reserve() {
            Ok(permit) => Poll::Ready(SendPermit {
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

pub struct MsduReceiver<'a, T, const DEPTH: usize> {
    receiver: AffineSpscReceiver<'a, T, DEPTH>,
    consumer: &'a AtomicWaker,
    producer: &'a AtomicWaker,
}

impl<T, const DEPTH: usize> MsduReceiver<'_, T, DEPTH> {
    #[must_use]
    pub fn try_receive(&self) -> Option<T> {
        let value = self.receiver.try_receive().ok()?;
        self.producer.wake();
        Some(value)
    }

    /// Wait for a value without consuming it. The exclusive borrow lasts for
    /// the entire future, so the queue's sole consumer waker cannot be replaced
    /// by another outstanding wait. Cancellation leaves the value in the queue.
    ///
    /// ```compile_fail,E0499
    /// use oer_ieee80211_runtime::station_service::MsduQueue;
    /// let mut queue = MsduQueue::<u8, 1>::new();
    /// let (sender, mut receiver) = queue.split();
    /// let first = receiver.ready();
    /// let second = receiver.ready();
    /// drop((first, second));
    /// ```
    ///
    /// ```
    /// use oer_ieee80211_runtime::station_service::MsduQueue;
    /// async fn sequential() {
    ///     let mut queue = MsduQueue::<u8, 1>::new();
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
mod tests {
    use super::*;
    use core::future::Future;
    use core::sync::atomic::{AtomicUsize, Ordering};
    use core::task::Context;
    use std::sync::Arc;
    use std::task::{Wake, Waker};

    struct Wakes(AtomicUsize);

    impl Wake for Wakes {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, Ordering::Relaxed);
        }
    }

    #[test]
    fn full_queue_returns_the_same_non_clone_owner_and_keeps_fifo() {
        struct Owner(u8);
        let mut queue = MsduQueue::<Owner, 2>::new();
        let (sender, receiver) = queue.split();
        assert!(sender.try_send(Owner(1)).is_ok());
        assert!(sender.try_send(Owner(2)).is_ok());
        assert_eq!(sender.try_send(Owner(3)).err().unwrap().0.0, 3);
        assert_eq!(receiver.try_receive().unwrap().0, 1);
        assert!(sender.try_send(Owner(4)).is_ok());
        assert_eq!(receiver.try_receive().unwrap().0, 2);
        assert_eq!(receiver.try_receive().unwrap().0, 4);
        assert!(receiver.try_receive().is_none());
    }

    #[test]
    fn publication_wakes_the_consumer_and_reclaim_wakes_the_blocked_producer() {
        struct Owner(u8);
        let mut queue = MsduQueue::<Owner, 1>::new();
        let (mut sender, mut receiver) = queue.split();
        let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
        let waker = Waker::from(Arc::clone(&wakes));
        let mut cx = Context::from_waker(&waker);
        {
            let mut ready = core::pin::pin!(receiver.ready());
            assert!(ready.as_mut().poll(&mut cx).is_pending());
            assert!(sender.try_send(Owner(1)).is_ok());
            assert_eq!(wakes.0.load(Ordering::Relaxed), 1);
            assert!(ready.as_mut().poll(&mut cx).is_ready());
        }
        let owner = Owner(2);
        {
            let mut ready = core::pin::pin!(sender.ready());
            assert!(ready.as_mut().poll(&mut cx).is_pending());
            assert_eq!(receiver.try_receive().unwrap().0, 1);
            assert_eq!(wakes.0.load(Ordering::Relaxed), 2);
            let Poll::Ready(permit) = ready.as_mut().poll(&mut cx) else {
                panic!("the receiver returned the credit");
            };
            permit.send(owner);
        }
        assert_eq!(receiver.try_receive().unwrap().0, 2);
        assert!(receiver.try_receive().is_none());
    }

    #[test]
    fn cancellation_keeps_the_owner_and_both_endpoints_can_wait_again() {
        struct Owner(u8);
        let mut queue = MsduQueue::<Owner, 1>::new();
        let (mut sender, mut receiver) = queue.split();
        let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
        let waker = Waker::from(wakes);
        let mut cx = Context::from_waker(&waker);
        assert!(sender.try_send(Owner(1)).is_ok());
        let owner = Owner(2);
        {
            let mut wait = core::pin::pin!(sender.ready());
            assert!(wait.as_mut().poll(&mut cx).is_pending());
        }
        assert_eq!(owner.0, 2);
        assert_eq!(receiver.try_receive().unwrap().0, 1);
        {
            let mut wait = core::pin::pin!(sender.ready());
            // Reclamation before registration is seen without another wake.
            assert!(wait.as_mut().poll(&mut cx).is_ready());
        }
        {
            let mut wait = core::pin::pin!(receiver.ready());
            assert!(wait.as_mut().poll(&mut cx).is_pending());
        }
        assert!(sender.try_send(owner).is_ok());
        {
            let mut wait = core::pin::pin!(receiver.ready());
            // Publication before registration is seen without another wake.
            assert!(wait.as_mut().poll(&mut cx).is_ready());
        }
        assert_eq!(receiver.try_receive().unwrap().0, 2);
    }

    #[test]
    fn resplitting_keeps_the_queued_owner_until_the_next_receiver_takes_it() {
        struct Owner(Arc<AtomicUsize>);
        impl Drop for Owner {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::Relaxed);
            }
        }
        let drops = Arc::new(AtomicUsize::new(0));
        {
            let mut queue = MsduQueue::<Owner, 1>::new();
            {
                let (sender, _receiver) = queue.split();
                assert!(sender.try_send(Owner(Arc::clone(&drops))).is_ok());
            }
            let (_next_sender, next_receiver) = queue.split();
            assert_eq!(drops.load(Ordering::Relaxed), 0);
            let owner = next_receiver.try_receive().unwrap();
            assert!(next_receiver.try_receive().is_none());
            assert_eq!(drops.load(Ordering::Relaxed), 0);
            drop(owner);
            assert_eq!(drops.load(Ordering::Relaxed), 1);
        }
        assert_eq!(drops.load(Ordering::Relaxed), 1);
    }

    struct ThreadWake(std::thread::Thread);

    impl Wake for ThreadWake {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
        fn wake_by_ref(self: &Arc<Self>) {
            self.0.unpark();
        }
    }

    fn wait<F: Future>(future: F) -> F::Output {
        let waker = Waker::from(Arc::new(ThreadWake(std::thread::current())));
        let mut cx = Context::from_waker(&waker);
        let mut future = core::pin::pin!(future);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            if let Poll::Ready(value) = future.as_mut().poll(&mut cx) {
                return value;
            }
            assert!(std::time::Instant::now() < deadline, "queue wake timed out");
            std::thread::park_timeout(
                deadline.saturating_duration_since(std::time::Instant::now()),
            );
            assert!(std::time::Instant::now() < deadline, "queue wake timed out");
        }
    }

    #[test]
    fn a_dropped_send_permit_publishes_nothing_and_can_be_reserved_again() {
        let mut queue = MsduQueue::<u8, 1>::new();
        let (mut sender, receiver) = queue.split();
        drop(wait(sender.ready()));
        assert!(receiver.try_receive().is_none());
        wait(sender.ready()).send(9);
        assert_eq!(receiver.try_receive(), Some(9));
    }

    #[test]
    fn a_producer_on_another_thread_sends_each_owner_once_through_its_permit() {
        struct Owner(usize, Arc<AtomicUsize>);
        impl Drop for Owner {
            fn drop(&mut self) {
                self.1.fetch_add(1, Ordering::Relaxed);
            }
        }
        const COUNT: usize = 512;
        let drops = Arc::new(AtomicUsize::new(0));
        let mut queue = MsduQueue::<Owner, 1>::new();
        let (mut sender, mut receiver) = queue.split();
        std::thread::scope(|scope| {
            let producer_drops = Arc::clone(&drops);
            let producer = scope.spawn(move || {
                for id in 0..COUNT {
                    wait(sender.ready()).send(Owner(id, Arc::clone(&producer_drops)));
                }
            });
            for expected in 0..COUNT {
                wait(receiver.ready());
                let owner = receiver.try_receive().unwrap();
                assert_eq!(owner.0, expected);
                drop(owner);
            }
            producer.join().unwrap();
        });
        assert!(receiver.try_receive().is_none());
        assert_eq!(drops.load(Ordering::Relaxed), COUNT);
    }
}
