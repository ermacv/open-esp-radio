//! Stack/driver boundary counters. No wakeups or admission policy are changed.

use core::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy)]
pub enum Event {
    NetworkPoll,
    PollWithoutTransfer,
    TxReady,
    TxUnavailable,
    TxAccepted,
    TxRejected,
    RxEmpty,
    RxDelivered,
}

const COUNT: usize = Event::RxDelivered as usize + 1;
pub struct Counters([AtomicU32; COUNT]);

#[derive(Clone, Copy)]
pub struct Snapshot([u32; COUNT]);

impl Default for Counters {
    fn default() -> Self {
        Self::new()
    }
}

impl Counters {
    pub const fn new() -> Self {
        Self([const { AtomicU32::new(0) }; COUNT])
    }
    pub fn record(&self, event: Event) {
        self.0[event as usize].fetch_add(1, Ordering::Relaxed);
    }
    pub fn transfers(&self) -> u32 {
        self.0[Event::TxAccepted as usize]
            .load(Ordering::Relaxed)
            .wrapping_add(self.0[Event::RxDelivered as usize].load(Ordering::Relaxed))
    }
    pub fn snapshot(&self) -> Snapshot {
        Snapshot(core::array::from_fn(|i| self.0[i].load(Ordering::Relaxed)))
    }
}

impl Snapshot {
    pub fn get(self, event: Event) -> u32 {
        self.0[event as usize]
    }
    pub fn delta(self, earlier: Self) -> Self {
        Self(core::array::from_fn(|i| {
            self.0[i].wrapping_sub(earlier.0[i])
        }))
    }
}

pub struct Device<D> {
    pub inner: D,
    pub counters: &'static Counters,
}

impl<D> Device<D> {
    pub fn new(inner: D, counters: &'static Counters) -> Self {
        Self { inner, counters }
    }
}

/// A poll without a packet transfer may still perform internal protocol work.
/// This counter measures only the boundary, not whether the entire poll is useful.
pub async fn observe<F: core::future::Future>(future: F, counters: &Counters) -> F::Output {
    let mut future = core::pin::pin!(future);
    core::future::poll_fn(|cx| {
        let before = counters.transfers();
        let result = future.as_mut().poll(cx);
        counters.record(Event::NetworkPoll);
        if counters.transfers() == before {
            counters.record(Event::PollWithoutTransfer);
        }
        result
    })
    .await
}

// Each network stack's driver contract, counted at the same boundary.
#[cfg(feature = "owned-network")]
mod owned;
