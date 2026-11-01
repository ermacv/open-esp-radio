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
pub(crate) struct Fifo<X, const N: usize> {
    // Occupied cells form a prefix. Construction and the methods below are
    // the only writers; a checked free cell reserves capacity before insert.
    items: [Option<X>; N],
}

impl<X, const N: usize> Fifo<X, N> {
    pub(crate) const fn new() -> Self {
        Self {
            items: [const { None }; N],
        }
    }

    pub(crate) fn push(&mut self, item: X) -> Result<(), X> {
        match self.items.iter_mut().find(|slot| slot.is_none()) {
            Some(slot) => {
                *slot = Some(item);
                Ok(())
            }
            None => Err(item),
        }
    }

    /// Insert by the original operation order, preserving older returns.
    pub(crate) fn insert_by(&mut self, item: X, before: impl Fn(&X, &X) -> bool) -> Result<(), X> {
        // The last free cell is reserved before any ownership moves. With
        // the occupied-prefix invariant it is the final destination of carry.
        let Some((free, occupied)) = self.items.split_last_mut() else {
            return Err(item);
        };
        if free.is_some() {
            return Err(item);
        }
        let mut carry = item;
        let mut inserting = false;
        for slot in occupied {
            match slot {
                Some(other) => {
                    if inserting || before(&carry, other) {
                        core::mem::swap(&mut carry, other);
                        inserting = true;
                    }
                }
                None => {
                    *slot = Some(carry);
                    return Ok(());
                }
            }
        }
        *free = Some(carry);
        Ok(())
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &X> {
        self.items.iter().flatten()
    }

    /// The first item `matches` takes.
    pub(crate) fn first(&self, matches: impl Fn(&X) -> bool) -> Option<&X> {
        self.iter().find(|item| matches(item))
    }

    /// Remove the first item `matches` takes, keeping the others' order.
    pub(crate) fn remove_first(&mut self, matches: impl Fn(&X) -> bool) -> Option<X> {
        let index = self
            .items
            .iter()
            .position(|item| item.as_ref().is_some_and(&matches))?;
        let (_, from) = self.items.split_at_mut_checked(index)?;
        let (found, tail) = from.split_first_mut()?;
        let item = found.take()?;
        let mut vacant = found;
        for next in tail {
            *vacant = next.take();
            vacant = next;
        }
        Some(item)
    }

    /// Initialize an empty queue in place.
    #[cfg(feature = "placement")]
    pub(crate) fn init_in_place(place: &mut core::mem::MaybeUninit<Self>) -> &mut Self {
        oer_memory::init_in_place!(place, Self {
            items => |items| oer_memory::init_array(items, |_| None),
        })
    }

    pub(crate) fn pop(&mut self) -> Option<X> {
        let (front, tail) = self.items.split_first_mut()?;
        let item = front.take()?;
        let mut vacant = front;
        for next in tail {
            *vacant = next.take();
            vacant = next;
        }
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
