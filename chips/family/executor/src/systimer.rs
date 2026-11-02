//! The executor's system timer: the comparator owner, which the time
//! driver's writer executor holds, over the platform port's comparators,
//! and the shared counter latch published in the binding's storage for
//! the executor's clock. Each comparator's alarm part stays with its
//! interrupt table entry's handler.

use crate::binding::Binding;
use crate::chip::{Comparators as Owner, Counter, PortOf};
use oer_espressif_platform_port::{Comparators as _, ExecutorPlatform};

pub use oer_espressif_platform_port::{AlarmPart, Comparator};

/// The running timer's comparator and counter owners of port `P`, moved
/// from the reset root; the alarm parts go to their table entries.
pub struct SystemTimer<P: ExecutorPlatform> {
    pub comparators: Owner<P>,
    pub counter: Counter<P>,
}

/// The sole writer of comparator configuration, held by the writer executor.
pub struct ComparatorWriter<P: ExecutorPlatform> {
    timer: Owner<P>,
}

impl<P: ExecutorPlatform> ComparatorWriter<P> {
    /// Consume the running timer without resetting its counter or clock:
    /// publish the counter latch in binding `B`'s storage and return the
    /// comparator owner, which cannot be cloned or obtained a second time.
    pub fn adopt<B: Binding<Port = P>>(
        systimer: SystemTimer<P>,
    ) -> Result<Self, crate::Failure<PortOf<B>, SystemTimer<P>>> {
        crate::chip::install_counter::<B>(systimer).map(|timer| Self { timer })
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
