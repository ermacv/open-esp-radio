//! Platform-owned reset-retained tracing. The image places the storage and
//! installs its recorder; this module owns commit/clear and previous-boot policy.
//! The retained OETR layout is version 1. Retained words use loads/stores only;
//! recorder counters and the global pointer/mask live in ordinary atomic RAM.

mod store;
use oer_trace::{Channel, Event, Kind};
pub use store::{
    Boot, Geometry, Previous, Retained, SnapshotRecord, SnapshotWriter, Trace, Trigger,
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
    let [low, high] = &MASK;
    let word = if channel.bit_index() < 32 { low } else { high };
    word.load(Ordering::Relaxed) & (1 << (channel.bit_index() % 32)) != 0
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
#[macro_export]
macro_rules! trace_clock {
    ($now_micros:path) => {
        $crate::__memory::link_fn! {
            define fn __oer_trace_now_micros() -> u64 = $now_micros;
        }
    };
}

#[cfg(test)]
mod tests;
