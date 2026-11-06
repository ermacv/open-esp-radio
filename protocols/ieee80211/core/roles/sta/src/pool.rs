//! A bounded pool of tokens, whose capacity a const check proves.

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

    /// The items in their order, through `pop`: the Vec's own iterator indexes.
    fn in_order(mut items: heapless::Vec<X, N>) -> impl Iterator<Item = X> {
        let length = items.len();
        let mut cells = [const { None }; N];
        for cell in cells.iter_mut().take(length).rev() {
            *cell = items.pop();
        }
        cells.into_iter().flatten()
    }

    pub(crate) fn from_collection(items: heapless::Vec<X, N>) -> Self {
        let mut items = Self::in_order(items);
        Self {
            items: core::array::from_fn(|_| items.next()),
        }
    }

    /// Partition a bounded bank.
    pub(crate) fn partition(items: heapless::Vec<X, N>, active: usize) -> (Self, Self) {
        let mut items = Self::in_order(items);
        let free = Self {
            items: core::array::from_fn(|index| if index < active { items.next() } else { None }),
        };
        let spare = Self {
            items: core::array::from_fn(|_| items.next()),
        };
        (free, spare)
    }

    /// Drain into the same fixed capacity, retaining vacant cells as values.
    pub(crate) fn take_all(&mut self) -> [Option<X>; N] {
        self.items.each_mut().map(Option::take)
    }

    pub(crate) fn take(&mut self) -> Option<X> {
        self.items.iter_mut().find_map(Option::take)
    }

    pub(crate) fn len(&self) -> usize {
        self.items.iter().flatten().count()
    }

    #[cfg(any(test, feature = "test-support"))]
    pub(crate) fn iter(&self) -> impl Iterator<Item = &X> {
        self.items.iter().flatten()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.items.iter().all(Option::is_none)
    }

    /// Keep `item`, a token of its kind the machine was granted and gave
    /// out: the tokens are unique and their number checked at the pool's
    /// creation, so a free entry waits for it. A token beyond them (not
    /// one of the machine's) is dropped and reported to `events`.
    pub(crate) fn put(&mut self, item: X, events: &mut impl EventSink<StaEvent>) {
        match self.items.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => *slot = Some(item),
            None => report(events, || StaEvent::TokenLost),
        }
    }
}

#[cfg(test)]
mod partition_tests {
    use super::*;

    #[test]
    fn partition_preserves_all_owners_and_order_at_every_boundary() {
        struct Owner(u8);
        for active in [0, 3, POOL_CAPACITY, usize::MAX] {
            let values =
                heapless::Vec::from_array(core::array::from_fn::<_, POOL_CAPACITY, _>(|i| {
                    Owner(i as u8)
                }));
            let (mut free, mut spare): (Pool<Owner>, _) = Pool::partition(values, active);
            assert_eq!(free.len(), active.min(POOL_CAPACITY));
            let values: std::vec::Vec<_> = free
                .take_all()
                .into_iter()
                .chain(spare.take_all())
                .flatten()
                .map(|owner| owner.0)
                .collect();
            assert_eq!(
                values,
                (0..POOL_CAPACITY as u8).collect::<std::vec::Vec<_>>()
            );
            assert!(free.is_empty());
            assert!(spare.is_empty());
        }
    }
}
