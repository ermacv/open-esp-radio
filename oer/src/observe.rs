//! What the library observes of its own work for an image that measures it
//! (`observe` feature): the executors' CPU, the polls of named parts, the
//! station's and the network bridge's counts, the shared RF actor's PHY
//! tracking ticks and, on the boot hart, its cycle, instruction and cache
//! counters and a PC sampler.
//!
//! The executors, the station and the RF actor report through
//! `oer_trace::observe!`, which only an `oer_trace` build compiles: an image
//! with this feature declares `instrumentation = ["trace"]`.
//!
//! Each counter has one writer, the context that observes it; a counter a
//! reader on another task or hart takes whole is published under a sequence
//! number, odd while its writer writes, in atomic words only. A read that
//! sees the number change was concurrent with a write and is refused
//! ([`Concurrent`]); the async readers sample again after yielding and
//! never wait for the writer. Counts and microseconds wrap at 2^32 unless a
//! type says otherwise: a reader takes the differences of two readings.
//!
//! The types here are the library's own: an image translates them into the
//! vocabulary of whatever reports them.

pub mod cpu;
pub mod link;
pub mod parts;
pub mod tracking;

#[cfg(all(target_arch = "riscv32", feature = "esp32s31"))]
pub use crate::esp32s31::observed::*;

/// A reading ran while the counter's writer wrote: it holds no state the
/// writer published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Concurrent;

/// What the network bridge did with the station's frames: the frames it
/// took, those the stack refused for want of a pool packet, of queue room
/// or for their length or a down link, and those the station lent in place
/// and the bridge copied after all.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Bridge {
    pub received: u32,
    pub refused_pool: u32,
    pub refused_queue: u32,
    pub refused_other: u32,
    pub copied: u32,
}

/// The network bridge's counts; the IP service is their one writer, and a
/// reader takes each word on its own.
pub struct BridgeCounter {
    /// Received, refused for the pool, the queue and otherwise, copied.
    counts: [core::sync::atomic::AtomicU32; 5],
}

impl BridgeCounter {
    pub const fn new() -> Self {
        Self {
            counts: [const { core::sync::atomic::AtomicU32::new(0) }; 5],
        }
    }

    /// The bridge took a frame from the station.
    pub fn received(&self) {
        self.bump(0);
    }

    /// The stack refused a frame for `reason`.
    pub fn refused(&self, reason: oer_embassy_net::RxEnqueueError) {
        use oer_embassy_net::RxEnqueueError;
        self.bump(match reason {
            RxEnqueueError::PoolExhausted => 1,
            RxEnqueueError::QueueFull => 2,
            RxEnqueueError::InvalidLength(_) | RxEnqueueError::LinkDown => 3,
        });
    }

    /// The bridge copied a frame the station lent in place.
    pub fn copied(&self) {
        self.bump(4);
    }

    fn bump(&self, index: usize) {
        if let Some(count) = self.counts.get(index) {
            bump(count, 1);
        }
    }

    /// The counts now.
    pub fn read(&self) -> Bridge {
        use core::sync::atomic::Ordering;
        let [received, refused_pool, refused_queue, refused_other, copied] = self
            .counts
            .each_ref()
            .map(|count| count.load(Ordering::Relaxed));
        Bridge {
            received,
            refused_pool,
            refused_queue,
            refused_other,
            copied,
        }
    }
}

impl Default for BridgeCounter {
    fn default() -> Self {
        Self::new()
    }
}

/// Add `by` to `count`, wrapping; only its one writer calls it.
fn bump(count: &core::sync::atomic::AtomicU32, by: u32) {
    use core::sync::atomic::Ordering;
    count.store(
        count.load(Ordering::Relaxed).wrapping_add(by),
        Ordering::Relaxed,
    );
}

#[cfg(test)]
mod tests;
