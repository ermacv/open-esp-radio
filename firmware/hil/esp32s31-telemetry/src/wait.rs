//! Diagnostic future residence and suspension, including cancellation.
//!
//! Suspension includes executor scheduling latency. It is not CPU time and
//! does not identify the resource for which the observed future is waiting.

use core::{
    future::Future,
    sync::atomic::{AtomicU32, Ordering},
};

#[derive(Clone, Copy, Debug, Default)]
pub struct Snapshot {
    pub polls: u32,
    pub pending: u32,
    pub completed: u32,
    pub cancelled: u32,
    pub poll_micros: u32,
    pub suspended_micros: u32,
}

pub struct Counters {
    polls: AtomicU32,
    pending: AtomicU32,
    completed: AtomicU32,
    cancelled: AtomicU32,
    poll_micros: AtomicU32,
    suspended_micros: AtomicU32,
}

impl Counters {
    pub const fn new() -> Self {
        Self {
            polls: AtomicU32::new(0),
            pending: AtomicU32::new(0),
            completed: AtomicU32::new(0),
            cancelled: AtomicU32::new(0),
            poll_micros: AtomicU32::new(0),
            suspended_micros: AtomicU32::new(0),
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            polls: self.polls.load(Ordering::Relaxed),
            pending: self.pending.load(Ordering::Relaxed),
            completed: self.completed.load(Ordering::Relaxed),
            cancelled: self.cancelled.load(Ordering::Relaxed),
            poll_micros: self.poll_micros.load(Ordering::Relaxed),
            suspended_micros: self.suspended_micros.load(Ordering::Relaxed),
        }
    }

    /// Preserve the future's result and waker. Accumulate locally until it
    /// completes or is dropped, so every empty poll does not update atomics.
    pub async fn observe<F: Future>(&self, future: F, clock: impl Fn() -> u64) -> F::Output {
        let mut observation = Observation {
            counters: self,
            clock,
            sample: Snapshot::default(),
            suspended_since: None,
        };
        let mut future = core::pin::pin!(future);
        core::future::poll_fn(|cx| {
            let started = (observation.clock)();
            observation.end_suspension(started);
            let result = future.as_mut().poll(cx);
            let ended = (observation.clock)();
            observation.sample.polls = observation.sample.polls.wrapping_add(1);
            observation.sample.poll_micros = observation
                .sample
                .poll_micros
                .wrapping_add(ended.saturating_sub(started) as u32);
            if result.is_pending() {
                observation.sample.pending = observation.sample.pending.wrapping_add(1);
                observation.suspended_since = Some(ended);
            } else {
                observation.sample.completed = 1;
            }
            result
        })
        .await
    }
}

impl Default for Counters {
    fn default() -> Self {
        Self::new()
    }
}

struct Observation<'a, C: Fn() -> u64> {
    counters: &'a Counters,
    clock: C,
    sample: Snapshot,
    suspended_since: Option<u64>,
}

impl<C: Fn() -> u64> Observation<'_, C> {
    fn end_suspension(&mut self, now: u64) {
        if let Some(start) = self.suspended_since.take() {
            self.sample.suspended_micros = self
                .sample
                .suspended_micros
                .wrapping_add(now.saturating_sub(start) as u32);
        }
    }
}

impl<C: Fn() -> u64> Drop for Observation<'_, C> {
    fn drop(&mut self) {
        self.end_suspension((self.clock)());
        self.sample.cancelled = u32::from(self.sample.completed == 0);
        for (counter, value) in [
            (&self.counters.polls, self.sample.polls),
            (&self.counters.pending, self.sample.pending),
            (&self.counters.completed, self.sample.completed),
            (&self.counters.cancelled, self.sample.cancelled),
            (&self.counters.poll_micros, self.sample.poll_micros),
            (
                &self.counters.suspended_micros,
                self.sample.suspended_micros,
            ),
        ] {
            counter.fetch_add(value, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests;
