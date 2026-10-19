//! Serialize the sole async USB writer against immediate emergency output.

use core::{
    future::poll_fn,
    sync::atomic::{AtomicBool, Ordering},
    task::Poll,
};
use embassy_sync::waitqueue::AtomicWaker;

pub struct Writer {
    active: AtomicBool,
    async_waiting: AtomicBool,
    released: AtomicWaker,
}

impl Default for Writer {
    fn default() -> Self {
        Self::new()
    }
}

impl Writer {
    pub const fn new() -> Self {
        Self {
            active: AtomicBool::new(false),
            async_waiting: AtomicBool::new(false),
            released: AtomicWaker::new(),
        }
    }

    pub fn try_acquire(&self) -> Option<Guard<'_>> {
        if self.async_waiting.load(Ordering::Acquire) {
            return None;
        }
        self.acquire()
    }

    fn acquire(&self) -> Option<Guard<'_>> {
        self.active
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .ok()
            .map(|_| Guard(self))
    }

    pub async fn acquire_async(&self) -> Guard<'_> {
        self.async_waiting.store(true, Ordering::Release);
        let _waiting = Waiting(&self.async_waiting);
        poll_fn(|cx| {
            self.released.register(cx.waker());
            if let Some(guard) = self.acquire() {
                Poll::Ready(guard)
            } else {
                Poll::Pending
            }
        })
        .await
    }
}

pub struct Guard<'a>(&'a Writer);
impl Drop for Guard<'_> {
    fn drop(&mut self) {
        self.0.active.store(false, Ordering::Release);
        // AtomicWaker retains its registration. Do not wake an already admitted
        // async writer on each subsequent text/USB write.
        if self.0.async_waiting.load(Ordering::Acquire) {
            self.0.released.wake();
        }
    }
}

// Cancellation must not leave immediate output permanently excluded. There is
// exactly one async consumer: logger_task owns the USB transmit endpoint.
struct Waiting<'a>(&'a AtomicBool);
impl Drop for Waiting<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}
