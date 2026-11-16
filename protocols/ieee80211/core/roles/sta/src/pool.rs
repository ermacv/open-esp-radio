//! A bounded pool of the station's own places, whose capacity a const
//! check proves.

use crate::events::StaEvent;
use oer_portable_events::{EventSink, report};

/// The tokens of one kind a station machine holds at most.
pub(crate) const POOL_CAPACITY: usize = 8;

/// A bounded pool of at most `N` tokens of one kind.
pub(crate) struct Pool<X, const N: usize = POOL_CAPACITY> {
    items: [Option<X>; N],
}

impl<X, const N: usize> Pool<X, N> {
    /// A pool of `items`, at most `N`: checked at compile time, so no
    /// granted token is ever lost.
    pub(crate) fn new<const M: usize>(items: [X; M]) -> Self {
        const {
            if M > N {
                panic!("a pool holds no more tokens than its capacity");
            }
        };
        let mut pool = Self {
            items: [const { None }; N],
        };
        for (slot, item) in pool.items.iter_mut().zip(items) {
            *slot = Some(item);
        }
        pool
    }

    pub(crate) fn take(&mut self) -> Option<X> {
        self.items.iter_mut().find_map(Option::take)
    }

    pub(crate) fn len(&self) -> usize {
        self.items.iter().flatten().count()
    }

    /// Keep `item`, a token of its kind the machine was granted and gave
    /// out: the tokens are unique and their number checked at the pool's
    /// creation, so a free entry waits for it. A token beyond them (not
    /// one of the machine's) is dropped and reported to `events`.
    pub(crate) fn put(&mut self, item: X, events: &mut impl EventSink<StaEvent>) {
        match self.items.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => *slot = Some(item),
            None => report(events, || StaEvent::PlaceLost),
        }
    }
}
