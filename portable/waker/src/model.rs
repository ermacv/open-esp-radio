//! Model checks of the cell over every schedule the checker explores.

use core::future::poll_fn;
use core::sync::atomic::Ordering::Relaxed;
use core::task::Poll;

use loom::future::block_on;
use loom::sync::Arc;
use loom::sync::atomic::AtomicUsize;
use loom::thread;

use crate::AtomicWaker;

struct Channel {
    published: AtomicUsize,
    waker: AtomicWaker,
}

/// Every producer publishes, then wakes; the consumer registers, then checks.
/// A lost notification leaves `block_on` asleep, which the checker reports.
fn no_notification_is_lost(producers: usize) {
    loom::model(move || {
        let channel = Arc::new(Channel {
            published: AtomicUsize::new(0),
            waker: AtomicWaker::new(),
        });
        for _ in 0..producers {
            let channel = Arc::clone(&channel);
            thread::spawn(move || {
                channel.published.fetch_add(1, Relaxed);
                channel.waker.wake();
            });
        }
        block_on(poll_fn(|cx| {
            channel.waker.register(cx.waker());
            if channel.published.load(Relaxed) == producers {
                Poll::Ready(())
            } else {
                // A wake in progress makes the registration wake its own task.
                // This task must let that producer run: a consumer that
                // cannot, above a preempted producer, needs `PriorityWaker`.
                thread::yield_now();
                Poll::Pending
            }
        }));
    });
}

#[test]
fn one_producer() {
    no_notification_is_lost(1);
}

#[test]
fn two_producers() {
    no_notification_is_lost(2);
}
