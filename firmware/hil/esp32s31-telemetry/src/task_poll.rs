//! Optional executor-poll residence measurements for HIL tasks.

use core::sync::atomic::{AtomicU32, Ordering};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TaskPollSnapshot {
    pub polls: u32,
    pub poll_micros: u32,
    pub lifetime_max_micros: u32,
    pub over_100_micros: u32,
    pub over_500_micros: u32,
    pub over_1_000_micros: u32,
    pub over_5_000_micros: u32,
}

impl TaskPollSnapshot {
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            polls: self.polls.wrapping_sub(earlier.polls),
            poll_micros: self.poll_micros.wrapping_sub(earlier.poll_micros),
            lifetime_max_micros: self.lifetime_max_micros,
            over_100_micros: self.over_100_micros.wrapping_sub(earlier.over_100_micros),
            over_500_micros: self.over_500_micros.wrapping_sub(earlier.over_500_micros),
            over_1_000_micros: self
                .over_1_000_micros
                .wrapping_sub(earlier.over_1_000_micros),
            over_5_000_micros: self
                .over_5_000_micros
                .wrapping_sub(earlier.over_5_000_micros),
        }
    }
}

pub struct TaskPollCounters {
    polls: AtomicU32,
    poll_micros: AtomicU32,
    lifetime_max_micros: AtomicU32,
    over_100_micros: AtomicU32,
    over_500_micros: AtomicU32,
    over_1_000_micros: AtomicU32,
    over_5_000_micros: AtomicU32,
}

impl TaskPollCounters {
    pub const fn new() -> Self {
        Self {
            polls: AtomicU32::new(0),
            poll_micros: AtomicU32::new(0),
            lifetime_max_micros: AtomicU32::new(0),
            over_100_micros: AtomicU32::new(0),
            over_500_micros: AtomicU32::new(0),
            over_1_000_micros: AtomicU32::new(0),
            over_5_000_micros: AtomicU32::new(0),
        }
    }

    #[inline]
    pub fn record(&self, elapsed_micros: u64) {
        let elapsed_micros = u32::try_from(elapsed_micros).unwrap_or(u32::MAX);
        self.polls.fetch_add(1, Ordering::Relaxed);
        self.poll_micros
            .fetch_add(elapsed_micros, Ordering::Relaxed);
        self.lifetime_max_micros
            .fetch_max(elapsed_micros, Ordering::Relaxed);
        for (threshold, counter) in [
            (100, &self.over_100_micros),
            (500, &self.over_500_micros),
            (1_000, &self.over_1_000_micros),
            (5_000, &self.over_5_000_micros),
        ] {
            if elapsed_micros > threshold {
                counter.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    /// Merge a pre-aggregated task-poll batch without charging every hot poll
    /// for seven cross-module atomic updates.
    pub fn record_batch(&self, batch: TaskPollSnapshot) {
        self.polls.fetch_add(batch.polls, Ordering::Relaxed);
        self.poll_micros
            .fetch_add(batch.poll_micros, Ordering::Relaxed);
        self.lifetime_max_micros
            .fetch_max(batch.lifetime_max_micros, Ordering::Relaxed);
        self.over_100_micros
            .fetch_add(batch.over_100_micros, Ordering::Relaxed);
        self.over_500_micros
            .fetch_add(batch.over_500_micros, Ordering::Relaxed);
        self.over_1_000_micros
            .fetch_add(batch.over_1_000_micros, Ordering::Relaxed);
        self.over_5_000_micros
            .fetch_add(batch.over_5_000_micros, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> TaskPollSnapshot {
        TaskPollSnapshot {
            polls: self.polls.load(Ordering::Relaxed),
            poll_micros: self.poll_micros.load(Ordering::Relaxed),
            lifetime_max_micros: self.lifetime_max_micros.load(Ordering::Relaxed),
            over_100_micros: self.over_100_micros.load(Ordering::Relaxed),
            over_500_micros: self.over_500_micros.load(Ordering::Relaxed),
            over_1_000_micros: self.over_1_000_micros.load(Ordering::Relaxed),
            over_5_000_micros: self.over_5_000_micros.load(Ordering::Relaxed),
        }
    }
}

impl Default for TaskPollCounters {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TaskPollSetSnapshot {
    pub network: TaskPollSnapshot,
    pub radio: TaskPollSnapshot,
    pub udp_rx: TaskPollSnapshot,
    pub udp_tx: TaskPollSnapshot,
    pub tcp: TaskPollSnapshot,
}

impl TaskPollSetSnapshot {
    /// Close the interval before asynchronous reporting can add more polls.
    pub fn wrapping_delta_since(self, earlier: Self) -> Self {
        Self {
            network: self.network.wrapping_delta_since(earlier.network),
            radio: self.radio.wrapping_delta_since(earlier.radio),
            udp_rx: self.udp_rx.wrapping_delta_since(earlier.udp_rx),
            udp_tx: self.udp_tx.wrapping_delta_since(earlier.udp_tx),
            tcp: self.tcp.wrapping_delta_since(earlier.tcp),
        }
    }
}

pub struct TaskPollSet {
    network: TaskPollCounters,
    radio: TaskPollCounters,
    udp_rx: TaskPollCounters,
    udp_tx: TaskPollCounters,
    tcp: TaskPollCounters,
}

impl TaskPollSet {
    pub const fn new() -> Self {
        Self {
            network: TaskPollCounters::new(),
            radio: TaskPollCounters::new(),
            udp_rx: TaskPollCounters::new(),
            udp_tx: TaskPollCounters::new(),
            tcp: TaskPollCounters::new(),
        }
    }

    pub const fn network(&self) -> &TaskPollCounters {
        &self.network
    }

    pub const fn radio(&self) -> &TaskPollCounters {
        &self.radio
    }

    pub const fn udp_rx(&self) -> &TaskPollCounters {
        &self.udp_rx
    }

    pub const fn udp_tx(&self) -> &TaskPollCounters {
        &self.udp_tx
    }

    pub const fn tcp(&self) -> &TaskPollCounters {
        &self.tcp
    }

    pub fn snapshot(&self) -> TaskPollSetSnapshot {
        TaskPollSetSnapshot {
            network: self.network.snapshot(),
            radio: self.radio.snapshot(),
            udp_rx: self.udp_rx.snapshot(),
            udp_tx: self.udp_tx.snapshot(),
            tcp: self.tcp.snapshot(),
        }
    }
}

impl Default for TaskPollSet {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests;
