//! Diagnostic, with `oer_trace` only: the aggregate cycle's two spans in
//! the driver, from an aggregate's end seen to the next publication (the
//! gap) and from a publication to its end seen (the air), counted and
//! summed in microseconds. Statics, so the driver's types stay the same.
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

/// From an aggregate's end seen to the next publication.
pub static GAP: Span = Span::new();
/// From a publication to its end seen.
pub static AIR: Span = Span::new();
/// The last end seen and the last publication, in microseconds; 0 none.
static ENDED: AtomicU32 = AtomicU32::new(0);
static PUBLISHED: AtomicU32 = AtomicU32::new(0);

/// An aggregate's end seen at `now_us`.
pub(crate) fn ended(now_us: u64) {
    let now = now_us as u32 | 1;
    let published = PUBLISHED.swap(0, Ordering::Relaxed);
    if published != 0 {
        AIR.add(now.wrapping_sub(published));
    }
    ENDED.store(now, Ordering::Relaxed);
}

/// An aggregate published at `now_us`.
pub(crate) fn published(now_us: u64) {
    let now = now_us as u32 | 1;
    let ended = ENDED.swap(0, Ordering::Relaxed);
    if ended != 0 {
        GAP.add(now.wrapping_sub(ended));
    }
    PUBLISHED.store(now, Ordering::Relaxed);
}
