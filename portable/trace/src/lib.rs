#![no_std]
#![forbid(unsafe_code)]

//! Typed, reset-retained event trace and snapshot slots.
//!
//! A driver defines its trace points as [`Event`] types: each has a [`Kind`]
//! (a [`Domain`] plus the domain's own event id), a [`Channel`] that the host
//! enables or disables per session, and an encoding into two words. The
//! host decodes a drained [`Record`] with the same type, so no event is
//! identified by a bare number.
//!
//! The image places one [`Retained`] in memory that survives the resets it
//! cares about and builds one [`Trace`] over it; under `cfg(oer_trace)`
//! (the trace class, which `cargo fw` sets for an image that declares it)
//! it names, with [`clock!`], the function that reads its monotonic time in
//! microseconds:
//!
//! ```ignore
//! #[unsafe(link_section = ".rtc_fast.persistent")]
//! static RETAINED: oer_trace::Retained<512, 2, 1024> = oer_trace::Retained::new();
//! static TRACE: oer_trace::Trace = oer_trace::Trace::new(&RETAINED);
//! oer_trace::clock!(oer_embassy_time::now_micros);
//!
//! let boot = oer_trace::install(&TRACE); // holds what the previous boot left
//! // ... drain `boot.previous` if it matters ...
//! oer_trace::start(default_mask);
//! ```
//!
//! Retained memory only ever sees plain loads and stores: the write index,
//! the freeze latch and the snapshot cursor are read-modify-write atomics in
//! the [`Trace`]. An image that freezes from a panic with caches unavailable
//! places that `Trace` in uncached RAM; the global pointer and channel mask
//! are in `.critical.bss.*`, SRAM the runtime zeroes. After a reset the order of the
//! retained entries comes back from their 16-bit sequence tags, and an entry
//! a reset interrupted fails its commit word and is dropped.
//!
//! Without `cfg(oer_trace)`, [`emit`], [`freeze`] and [`capture`]
//! compile to nothing. With it, a disabled channel costs one relaxed load
//! and a branch at the call site; everything on the emit path is inlined
//! into the caller, so it runs from wherever the caller runs.

mod decode;
mod event;
mod store;

pub use decode::{Described, Describer, EventSet};
pub use event::{Channel, Domain, Event, Kind};
pub use store::{
    Boot, Geometry, Previous, Record, Retained, SnapshotRecord, SnapshotWriter, Trace, Trigger,
    oldest_first,
};

use core::sync::atomic::{AtomicU32, Ordering};

use oer_memory::StaticRef;

oer_memory::zeroed_static! {
    /// Enabled channels, one bit each; zero unless a trace is running. In
    /// SRAM, which interrupt handlers and the panic path reach without a
    /// cache.
    static MASK: [AtomicU32; 2] = zeroed in ".critical.bss.trace_mask";
    /// The installed trace, in SRAM like the mask.
    static INSTALLED: StaticRef<Trace> = zeroed in ".critical.bss.trace_pointer";
}

/// Make `trace` the image's trace. Recording stays off, and whatever the
/// previous boot left stays readable, until [`start`].
pub fn install(trace: &'static Trace) -> Boot {
    disable();
    let boot = trace.hold();
    INSTALLED.set(trace);
    boot
}

/// The installed trace, for draining.
#[inline(always)]
pub fn installed() -> Option<&'static Trace> {
    INSTALLED.get()
}

/// Discard what the previous boot left and record the channels in `mask`.
/// Without an installed trace this does nothing.
pub fn start(mask: u64) {
    if let Some(trace) = installed() {
        trace.restart();
        set_mask(mask);
    }
}

/// Record exactly the channels in `mask`, while the trace runs.
pub fn set_mask(mask: u64) {
    let Some(trace) = installed() else {
        return;
    };
    if !trace.is_running() {
        return;
    }
    MASK[0].store(mask as u32, Ordering::Relaxed);
    MASK[1].store((mask >> 32) as u32, Ordering::Relaxed);
}

/// The channels recorded now; zero while held, stopped or frozen.
pub fn mask() -> u64 {
    u64::from(MASK[0].load(Ordering::Relaxed)) | (u64::from(MASK[1].load(Ordering::Relaxed)) << 32)
}

#[inline(always)]
fn disable() {
    MASK[0].store(0, Ordering::Relaxed);
    MASK[1].store(0, Ordering::Relaxed);
}

/// Whether `channel` is recorded now.
#[inline(always)]
pub fn enabled(channel: Channel) -> bool {
    MASK[channel.word()].load(Ordering::Relaxed) & channel.bit() != 0
}

/// Record `event` if its channel is enabled.
#[inline(always)]
pub fn emit<E: Event>(event: &E) {
    #[cfg(oer_trace)]
    if enabled(E::CHANNEL) {
        record(E::KIND, event.encode());
    }
    #[cfg(not(oer_trace))]
    let _ = event;
}

#[cfg(oer_trace)]
#[inline(always)]
fn record(kind: Kind, words: [u32; 2]) {
    if let Some(trace) = installed()
        && trace.record(kind, words, trace.now_us())
    {
        disable();
    }
}

/// Stop recording `post` entries after this one, naming `trigger` as the
/// reason; `post == 0` freezes immediately. Only the first freeze of a run
/// counts. Safe to call from any interrupt: it takes no lock.
#[inline(always)]
pub fn freeze(trigger: Kind, post: u32) {
    #[cfg(oer_trace)]
    if let Some(trace) = installed()
        && trace.freeze(trigger, post)
    {
        disable();
    }
    #[cfg(not(oer_trace))]
    let _ = (trigger, post);
}

/// Copy a diagnostic window into the next snapshot slot at the named
/// `point`. `fill` pushes the words; the slot keeps as many as fit and
/// records that the rest were truncated.
#[inline(always)]
pub fn capture(point: Kind, fill: impl FnOnce(&mut SnapshotWriter<'_>)) {
    #[cfg(oer_trace)]
    if let Some(trace) = installed() {
        trace.capture(point, trace.now_us(), fill);
    }
    #[cfg(not(oer_trace))]
    let _ = (point, fill);
}

/// Name the image's trace clock: a `fn() -> u64` of monotonic time in
/// microseconds, which stamps every record. An image declares it once, like
/// a global time driver: the trace is global, so the clock is bound at link
/// time and every record calls it directly. A recording image without one
/// fails to link on `__oer_trace_now_micros`.
#[doc(hidden)]
pub use oer_memory as __memory;

#[macro_export]
macro_rules! clock {
    ($now_micros:path) => {
        $crate::__memory::link_fn! {
            define fn __oer_trace_now_micros() -> u64 = $now_micros;
        }
    };
}

#[cfg(test)]
mod tests;
