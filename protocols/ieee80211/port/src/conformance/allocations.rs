//! Fixture allocation numbers belong to live owners, independently of addresses.
use core::cell::Cell;

/// Task-local allocation-number pool. A freed number is available to the next
/// allocation even if its storage address is reused.
#[derive(Debug)]
pub struct Ids<const N: usize>(Cell<[bool; N]>);
impl<const N: usize> Ids<N> {
    /// All numbers are initially free.
    pub const fn new() -> Self {
        Self(Cell::new([false; N]))
    }
}
impl<const N: usize> Default for Ids<N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> AsRef<Self> for Ids<N> {
    fn as_ref(&self) -> &Self {
        self
    }
}

/// Owned by the actual external body; dropping that body frees its number.
#[derive(Debug)]
pub struct Allocation<P: AsRef<Ids<N>>, const N: usize> {
    pool: P,
    id: u8,
}
impl<P: AsRef<Ids<N>>, const N: usize> Allocation<P, N> {
    /// Claim the first free number, retaining the registry through the owner.
    pub fn new(pool: P) -> Option<Self> {
        let mut live = pool.as_ref().0.get();
        let index = live.iter().position(|used| !used)?;
        let id = u8::try_from(index).ok()?;
        *live.get_mut(index)? = true;
        pool.as_ref().0.set(live);
        Some(Self { pool, id })
    }
    /// Stable for exactly this allocation's lifetime.
    pub const fn id(&self) -> u8 {
        self.id
    }
}
impl<P: AsRef<Ids<N>>, const N: usize> Drop for Allocation<P, N> {
    fn drop(&mut self) {
        let mut live = self.pool.as_ref().0.get();
        // Only new() mints this private number, from this same array.
        for (index, used) in live.iter_mut().enumerate() {
            if index == usize::from(self.id) {
                *used = false;
            }
        }
        self.pool.as_ref().0.set(live);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn live_allocations_are_distinct_and_free_releases_the_number() {
        let ids = Ids::<2>::new();
        let first = Allocation::new(&ids).unwrap();
        let second = Allocation::new(&ids).unwrap();
        assert_eq!((first.id(), second.id()), (0, 1));
        assert!(Allocation::new(&ids).is_none());
        drop(first);
        let reused = Allocation::new(&ids).unwrap();
        assert_eq!(reused.id(), 0);
        assert_eq!(second.id(), 1);
    }
    #[test]
    fn refusal_free_reuses_zero_while_a_retained_body_keeps_its_number() {
        let ids = Ids::<4>::new();
        for _ in 0..4 {
            assert_eq!(Allocation::new(&ids).unwrap().id(), 0);
        }
        let retained = Allocation::new(&ids).unwrap();
        assert_eq!(retained.id(), 0);
        assert_eq!(Allocation::new(&ids).unwrap().id(), 1);
        drop(retained);
        assert_eq!(Allocation::new(&ids).unwrap().id(), 0);
    }
}
