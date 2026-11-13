//! What the library observes of its own work for an image that measures it
//! (`observe` feature): the executors' CPU, the polls of named parts, the
//! station's and the network bridge's counts, the network core's turns,
//! the ADDBA Requests the station took, a connect's timeline, the shared RF actor's PHY
//! tracking ticks and, on the boot hart, its cycle, instruction and cache
//! counters and a PC sampler.
//!
//! The executors, the station and the RF actor report through
//! `oer_portable_trace::observe!`, which only an `oer_trace` build compiles: an image
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

pub mod addba;
pub mod connect;
pub mod cpu;
pub mod link;
pub mod network;
pub mod parts;
pub mod profile;
pub mod tracking;

/// A reading ran while the counter's writer wrote: it holds no state the
/// writer published.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Concurrent;

/// Add `by` to `count`, wrapping; only its one writer calls it.
fn bump(count: &core::sync::atomic::AtomicU32, by: u32) {
    use core::sync::atomic::Ordering;
    count.store(
        count.load(Ordering::Relaxed).wrapping_add(by),
        Ordering::Relaxed,
    );
}
