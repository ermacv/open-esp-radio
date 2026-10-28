//! The executor's system timer: the comparator owner, which the time
//! driver's writer executor holds, over the platform port's comparators,
//! and the shared counter latch published for [`crate::chip`]'s clock. Each
//! comparator's alarm part stays with its interrupt table entry's handler.

use crate::chip::{Comparators as Owner, Counter};
use oer_espressif_platform_port::Comparators as _;

pub use oer_espressif_platform_port::{AlarmPart, Comparator};

/// The running timer's comparator and counter owners, moved from the reset
/// root; the alarm parts go to their table entries.
pub struct SystemTimer {
    pub comparators: Owner,
    pub counter: Counter,
}

/// The sole writer of comparator configuration, held by the writer executor.
pub struct ComparatorWriter {
    timer: Owner,
}

impl ComparatorWriter {
    /// Consume the running timer without resetting its counter or clock:
    /// publish the counter latch and return the comparator owner, which
    /// cannot be cloned or obtained a second time.
    pub fn adopt(systimer: SystemTimer) -> Result<Self, crate::Failure<SystemTimer>> {
        crate::chip::install_counter(systimer).map(|timer| Self { timer })
    }

    /// Bind `alarm`'s comparator in one-shot mode before its handler's
    /// route is enabled; setup still holds the alarm part.
    pub fn bind<A: AlarmPart>(&mut self, alarm: &mut A) {
        self.timer.bind(alarm);
    }

    /// Program `comparator` for `ticks`; `None` only disables it.
    pub fn program(&mut self, comparator: Comparator, ticks: Option<u64>) {
        self.timer.program(comparator, ticks);
    }
}
