#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! Typed, reset-retained event trace and snapshot slots.
//!
//! A driver defines its trace points as [`Event`] types: each has a [`Kind`]
//! (a [`Domain`] plus the domain's own event id), a [`Channel`] that the host
//! enables or disables per session, and an encoding into two words. The
//! host decodes a drained [`Record`] with the same type, so no event is
//! identified by a bare number.
//!
//! The image places one [`Retained`] in memory that survives the resets it
//! cares about and builds one [`Trace`] over it; with the `record` feature it
//! names, with [`clock!`], the function that reads its monotonic time in
//! microseconds:
//!
//! ```ignore
//! #[unsafe(link_section = ".rtc_fast.persistent")]
//! static RETAINED: oer_trace::Retained<512, 2, 1024> = oer_trace::Retained::new();
//! static TRACE: oer_trace::Trace = oer_trace::Trace::new(&RETAINED);
//! oer_trace::clock!(oer_time_embassy::now_micros);
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
//! use `.critical.data.*` on RISC-V. After a reset the order of the
//! retained entries comes back from their 16-bit sequence tags, and an entry
//! a reset interrupted fails its commit word and is dropped.
//!
//! With the `record` feature disabled, [`emit`], [`freeze`] and [`capture`]
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

use core::ptr;
use core::sync::atomic::{AtomicPtr, AtomicU32, Ordering};

/// Enabled channels, one bit each; zero unless a trace is running.
#[allow(
    unsafe_code,
    reason = "panic/interrupt trace state must stay in uncached memory"
)]
#[cfg_attr(
    target_arch = "riscv32",
    unsafe(link_section = ".critical.data.trace_mask")
)]
static MASK: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];
#[allow(
    unsafe_code,
    reason = "panic/interrupt trace state must stay in uncached memory"
)]
#[cfg_attr(
    target_arch = "riscv32",
    unsafe(link_section = ".critical.data.trace_pointer")
)]
static INSTALLED: AtomicPtr<Trace> = AtomicPtr::new(ptr::null_mut());

/// Make `trace` the image's trace. Recording stays off, and whatever the
/// previous boot left stays readable, until [`start`].
pub fn install(trace: &'static Trace) -> Boot {
    disable();
    let boot = trace.hold();
    INSTALLED.store(ptr::from_ref(trace).cast_mut(), Ordering::Release);
    boot
}

/// The installed trace, for draining.
#[inline(always)]
pub fn installed() -> Option<&'static Trace> {
    let trace = INSTALLED.load(Ordering::Acquire);
    #[allow(
        unsafe_code,
        reason = "a static reference published through an atomic pointer"
    )]
    // SAFETY: `install` is the only writer, and it stores a pointer derived
    // from a `&'static Trace`; the pointee is therefore valid, never mutably
    // borrowed (all its state is atomic) and lives for the rest of the program.
    unsafe {
        trace.as_ref()
    }
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
    #[cfg(feature = "record")]
    if enabled(E::CHANNEL) {
        record(E::KIND, event.encode());
    }
    #[cfg(not(feature = "record"))]
    let _ = event;
}

#[cfg(feature = "record")]
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
    #[cfg(feature = "record")]
    if let Some(trace) = installed()
        && trace.freeze(trigger, post)
    {
        disable();
    }
    #[cfg(not(feature = "record"))]
    let _ = (trigger, post);
}

/// Copy a diagnostic window into the next snapshot slot at the named
/// `point`. `fill` pushes the words; the slot keeps as many as fit and
/// records that the rest were truncated.
#[inline(always)]
pub fn capture(point: Kind, fill: impl FnOnce(&mut SnapshotWriter<'_>)) {
    #[cfg(feature = "record")]
    if let Some(trace) = installed() {
        trace.capture(point, trace.now_us(), fill);
    }
    #[cfg(not(feature = "record"))]
    let _ = (point, fill);
}

/// Name the image's trace clock: a `fn() -> u64` of monotonic time in
/// microseconds, which stamps every record. An image declares it once, like
/// a global time driver: the trace is global, so the clock is bound at link
/// time and every record calls it directly. A recording image without one
/// fails to link on `__oer_trace_now_micros`.
#[macro_export]
macro_rules! clock {
    ($now_micros:path) => {
        const _: () = {
            #[allow(unsafe_code, reason = "the trace's clock is bound at link time")]
            #[unsafe(export_name = "__oer_trace_now_micros")]
            fn now_micros() -> u64 {
                $now_micros()
            }
        };
    };
}

#[cfg(test)]
mod tests;
