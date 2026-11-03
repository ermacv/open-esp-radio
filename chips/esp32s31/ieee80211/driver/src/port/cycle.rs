//! Diagnostic, with `oer_trace` only: from a TX completion interrupt to the
//! driver seeing its aggregate's end (in CPU cycles, reported in
//! microseconds at 320 MHz), and how often that end published the next
//! aggregate at once. Statics, so the driver's types stay the same.
use core::sync::atomic::{AtomicU32, Ordering};

/// One span's count, sum and longest, in microseconds.
pub struct Span {
    count: AtomicU32,
    total_us: AtomicU32,
    longest_us: AtomicU32,
}

impl Span {
    const fn new() -> Self {
        Self {
            count: AtomicU32::new(0),
            total_us: AtomicU32::new(0),
            longest_us: AtomicU32::new(0),
        }
    }
    fn add(&self, span_us: u32) {
        self.count.fetch_add(1, Ordering::Relaxed);
        self.total_us.fetch_add(span_us, Ordering::Relaxed);
        self.longest_us.fetch_max(span_us, Ordering::Relaxed);
    }
    /// The count, the sum and the longest.
    pub fn read(&self) -> (u32, u32, u32) {
        (
            self.count.load(Ordering::Relaxed),
            self.total_us.load(Ordering::Relaxed),
            self.longest_us.load(Ordering::Relaxed),
        )
    }
}

/// From a TX completion interrupt to the driver seeing the end.
pub static SEEN: Span = Span::new();
/// Ends that published the next aggregate at once (count only).
pub static KICKED: Span = Span::new();
/// The cycle of the first TX completion interrupt not seen yet; 0 none.
static IRQ_AT: AtomicU32 = AtomicU32::new(0);

const CYCLES_PER_US: u32 = 320;

fn cycles() -> u32 {
    #[cfg(target_arch = "riscv32")]
    {
        oer_portable_riscv::cycle32() | 1
    }
    #[cfg(not(target_arch = "riscv32"))]
    {
        1
    }
}

/// A TX completion interrupt.
pub(crate) fn tx_irq() {
    let _first = IRQ_AT.compare_exchange(0, cycles(), Ordering::Relaxed, Ordering::Relaxed);
}

/// The driver saw an aggregate's end.
pub(crate) fn seen() {
    let at = IRQ_AT.swap(0, Ordering::Relaxed);
    if at != 0 {
        SEEN.add(cycles().wrapping_sub(at) / CYCLES_PER_US);
    }
}

/// An end published the next aggregate at once.
pub(crate) fn kicked() {
    KICKED.add(0);
}
