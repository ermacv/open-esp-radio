extern crate std;

use super::*;
use core::future::Future;
use core::pin::pin;
use core::sync::atomic::AtomicUsize;
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

fn now<F: Future>(future: F) -> Option<F::Output> {
    let mut cx = Context::from_waker(Waker::noop());
    match pin!(future).poll(&mut cx) {
        Poll::Ready(value) => Some(value),
        Poll::Pending => None,
    }
}

#[derive(Debug, Eq, PartialEq)]
struct Owner(u8);

#[test]
fn a_closed_reception_refuses_every_later_send_whole_and_keeps_what_it_holds() {
    let mut queue = ClosingSpscQueue::<Owner, 3>::new();
    let (mut sender, mut receiver) = queue.split();
    assert!(sender.try_send(Owner(1)).is_ok());
    // A permit issued before the close publishes nothing after it.
    let permit = sender.try_reserve().expect("room");
    now(receiver.close()).expect("no publication in progress");
    assert!(receiver.is_closed());
    assert_eq!(permit.send(Owner(2)), Err(ClosingSpscClosed(Owner(2))));
    assert!(sender.is_closed());
    assert_eq!(
        sender.try_send(Owner(3)),
        Err(ClosingSpscSendError::Closed(Owner(3)))
    );
    assert_eq!(
        sender.try_send_held(Owner(4)),
        Err(ClosingSpscSendError::Closed(Owner(4)))
    );
    let late = now(sender.ready()).expect("a free cell").send(Owner(5));
    assert_eq!(late, Err(ClosingSpscClosed(Owner(5))));
    // What it held before the close stays, once, in order.
    assert_eq!(receiver.len(), 1);
    assert_eq!(receiver.try_receive(), Some(Owner(1)));
    assert!(receiver.try_receive().is_none());
}

#[test]
fn an_open_queue_refuses_a_full_send_as_full() {
    let mut queue = ClosingSpscQueue::<Owner, 1>::new();
    let (sender, receiver) = queue.split();
    assert!(sender.try_send(Owner(1)).is_ok());
    assert_eq!(
        sender.try_send(Owner(2)),
        Err(ClosingSpscSendError::Full(Owner(2)))
    );
    assert_eq!(receiver.try_receive(), Some(Owner(1)));
}

#[test]
fn a_close_waits_for_a_publication_in_progress_and_then_counts_it() {
    let mut queue = ClosingSpscQueue::<u8, 2>::new();
    let (sender, mut receiver) = queue.split();
    let wakes = Arc::new(Wakes(AtomicUsize::new(0)));
    let waker = Waker::from(Arc::clone(&wakes));
    let mut cx = Context::from_waker(&waker);
    let state = receiver.state;
    // A publication that began before the close, as on another hart.
    state.fetch_or(SENDING, Ordering::SeqCst);
    let mut close = pin!(receiver.close());
    assert!(
        close.as_mut().poll(&mut cx).is_pending(),
        "a send in progress"
    );
    // It ends: its value is in the queue before the close completes.
    assert!(sender.sender.try_send(7).is_ok());
    if state.fetch_and(!SENDING, Ordering::SeqCst) & CLOSED != 0 {
        sender.sender.consumer.notify();
    }
    assert!(wakes.0.load(Ordering::Relaxed) >= 1, "the close is woken");
    assert!(close.as_mut().poll(&mut cx).is_ready());
    assert_eq!(state.load(Ordering::SeqCst), CLOSED);
}

#[test]
fn a_new_pair_opens_the_reception_again() {
    let mut queue = ClosingSpscQueue::<u8, 1>::new();
    {
        let (sender, mut receiver) = queue.split();
        now(receiver.close()).expect("closed");
        assert!(sender.try_send(1).is_err());
    }
    let (sender, receiver) = queue.split();
    assert!(sender.try_send(1).is_ok());
    assert_eq!(receiver.try_receive(), Some(1));
}
