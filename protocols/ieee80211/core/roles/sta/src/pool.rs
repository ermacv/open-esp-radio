//! A bounded pool of tokens, whose capacity a const check proves.

use crate::events::StaEvent;
use oer_events::{EventSink, report};

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

    pub(crate) fn from_collection(items: heapless::Vec<X, N>) -> Self {
        let mut items = items.into_iter();
        Self {
            items: core::array::from_fn(|_| items.next()),
        }
    }

    /// Partition a bounded bank without collecting through a panicking Vec iterator.
    pub(crate) fn partition(items: heapless::Vec<X, N>, active: usize) -> (Self, Self) {
        let mut items = items.into_iter();
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

/// A first-in, first-out queue of at most `N` items. A machine sizes it by
/// a const parameter checked at build against the tokens its items hold;
/// an item beyond `N` comes back to the caller.
///
/// The items lie in a ring from `head`: taking the first item, or the first
/// one a predicate takes when it is at the head, moves no other item, so a
/// queue of large items costs no copies on its usual path. An item taken
/// from inside the ring, or inserted inside it, moves the items before it
/// (or after it) by one cell.
pub(crate) struct Fifo<X, const N: usize> {
    // The `len` items lie in cells `head`, `head + 1`, ... modulo `N`; every
    // other cell is empty. Construction and the methods below are the only
    // writers.
    items: [Option<X>; N],
    head: usize,
    len: usize,
}

impl<X, const N: usize> Fifo<X, N> {
    pub(crate) const fn new() -> Self {
        Self {
            items: [const { None }; N],
            head: 0,
            len: 0,
        }
    }

    /// The cell of the item `offset` places after the head; `None` only
    /// for a queue of no cells.
    fn cell(&self, offset: usize) -> Option<usize> {
        self.head.checked_add(offset)?.checked_rem(N)
    }

    /// Exchange the contents of the cells `offset` and `offset + 1` places
    /// after the head.
    fn swap_next(&mut self, offset: usize) {
        let (Some(this), Some(next)) = (self.cell(offset), self.cell(offset.saturating_add(1)))
        else {
            return;
        };
        if let Ok([this, next]) = self.items.get_disjoint_mut([this, next]) {
            core::mem::swap(this, next);
        }
    }

    pub(crate) fn push(&mut self, item: X) -> Result<(), X> {
        if self.len >= N {
            return Err(item);
        }
        match self
            .cell(self.len)
            .and_then(|cell| self.items.get_mut(cell))
        {
            Some(slot @ None) => {
                *slot = Some(item);
                self.len = self.len.saturating_add(1);
                Ok(())
            }
            _ => Err(item),
        }
    }

    /// Insert by the original operation order, preserving older returns:
    /// before the first item `before` puts it ahead of.
    pub(crate) fn insert_by(&mut self, item: X, before: impl Fn(&X, &X) -> bool) -> Result<(), X> {
        if self.len >= N {
            return Err(item);
        }
        let at = self
            .iter()
            .position(|other| before(&item, other))
            .unwrap_or(self.len);
        // The free cell after the last item moves back to `at`.
        let mut offset = self.len;
        while offset > at {
            offset = offset.saturating_sub(1);
            self.swap_next(offset);
        }
        match self.cell(at).and_then(|cell| self.items.get_mut(cell)) {
            Some(slot @ None) => {
                *slot = Some(item);
                self.len = self.len.saturating_add(1);
                Ok(())
            }
            _ => Err(item),
        }
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &X> {
        (0..self.len).filter_map(move |offset| {
            self.cell(offset)
                .and_then(|cell| self.items.get(cell))
                .and_then(Option::as_ref)
        })
    }

    /// The first item `matches` takes.
    pub(crate) fn first(&self, matches: impl Fn(&X) -> bool) -> Option<&X> {
        self.iter().find(|item| matches(item))
    }

    /// Remove the first item `matches` takes, keeping the others' order.
    pub(crate) fn remove_first(&mut self, matches: impl Fn(&X) -> bool) -> Option<X> {
        let at = self.iter().position(matches)?;
        let item = self
            .cell(at)
            .and_then(|cell| self.items.get_mut(cell))
            .and_then(Option::take)?;
        // The emptied cell moves forward to the head.
        let mut offset = at;
        while offset > 0 {
            offset = offset.saturating_sub(1);
            self.swap_next(offset);
        }
        self.advance();
        Some(item)
    }

    /// Drop the emptied head cell from the ring.
    fn advance(&mut self) {
        self.head = self.cell(1).unwrap_or(0);
        self.len = self.len.saturating_sub(1);
    }

    /// Initialize an empty queue in place.
    #[cfg(feature = "placement")]
    pub(crate) fn init_in_place(place: &mut core::mem::MaybeUninit<Self>) -> &mut Self {
        oer_memory::init_in_place!(place, Self {
            items => |items| oer_memory::init_array(items, |_| None),
            head: 0,
            len: 0,
        })
    }

    pub(crate) fn pop(&mut self) -> Option<X> {
        if self.len == 0 {
            return None;
        }
        let item = self
            .cell(0)
            .and_then(|cell| self.items.get_mut(cell))
            .and_then(Option::take)?;
        self.advance();
        Some(item)
    }
}

#[cfg(test)]
mod tests {
    use super::Fifo;

    #[test]
    fn fifo_moves_non_clone_owners_and_preserves_return_order() {
        struct Owned(u8);
        let mut queue = Fifo::<Owned, 3>::new();
        assert!(queue.push(Owned(3)).is_ok());
        assert!(queue.insert_by(Owned(1), |a, b| a.0 < b.0).is_ok());
        assert!(queue.insert_by(Owned(2), |a, b| a.0 < b.0).is_ok());
        assert_eq!(queue.push(Owned(4)).err().unwrap().0, 4);
        assert_eq!(
            queue
                .iter()
                .map(|value| value.0)
                .collect::<std::vec::Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(queue.pop().unwrap().0, 1);
        assert!(queue.push(Owned(4)).is_ok());
        assert_eq!(queue.pop().unwrap().0, 2);
        assert_eq!(queue.pop().unwrap().0, 3);
        assert_eq!(queue.pop().unwrap().0, 4);
        assert!(queue.pop().is_none());
        assert!(
            Fifo::<Owned, 0>::new()
                .insert_by(Owned(7), |a, b| a.0 < b.0)
                .is_err()
        );
    }

    #[test]
    fn a_queue_wrapping_its_ring_keeps_order_through_every_operation() {
        let values = |queue: &Fifo<u8, 4>| queue.iter().copied().collect::<std::vec::Vec<_>>();
        let mut queue = Fifo::<u8, 4>::new();
        for value in [1, 2, 3] {
            assert!(queue.push(value).is_ok());
        }
        // The head moves on: the next pushes wrap around the ring.
        assert_eq!(queue.pop(), Some(1));
        assert_eq!(queue.pop(), Some(2));
        for value in [4, 5, 6] {
            assert!(queue.push(value).is_ok());
        }
        assert_eq!(queue.push(7), Err(7));
        assert_eq!(values(&queue), [3, 4, 5, 6]);
        // From inside the ring, across its wrap: the others keep their order.
        assert_eq!(queue.remove_first(|value| *value == 5), Some(5));
        assert_eq!(values(&queue), [3, 4, 6]);
        assert_eq!(queue.remove_first(|value| *value == 9), None);
        // Ordered insertion across the wrap.
        assert!(queue.insert_by(5, |a, b| a < b).is_ok());
        assert_eq!(values(&queue), [3, 4, 5, 6]);
        assert_eq!(queue.insert_by(1, |a, b| a < b), Err(1));
        assert_eq!(queue.remove_first(|value| *value == 3), Some(3));
        assert!(queue.insert_by(0, |a, b| a < b).is_ok());
        assert_eq!(values(&queue), [0, 4, 5, 6]);
        assert_eq!(queue.first(|value| *value > 4), Some(&5));
        while queue.pop().is_some() {}
        assert!(queue.iter().next().is_none());
        assert!(Fifo::<u8, 0>::new().push(1).is_err());
        assert_eq!(Fifo::<u8, 0>::new().pop(), None);
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
