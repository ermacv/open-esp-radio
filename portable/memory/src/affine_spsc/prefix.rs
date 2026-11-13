//! A synchronous, in-place prefix of owning values. The cursor guard is
//! private: safe callers cannot forget it after taking an owner from a cell.

use core::sync::atomic::Ordering;

use super::{AffineSpscQueue, AffineSpscReceiver, SpscCursor};

struct Commit<'a, T, const DEPTH: usize, C: SpscCursor> {
    queue: &'a AffineSpscQueue<T, DEPTH, C>,
    cursor: usize,
    taken: usize,
}

impl<T, const DEPTH: usize, C: SpscCursor> Drop for Commit<'_, T, DEPTH, C> {
    fn drop(&mut self) {
        if self.taken != 0 {
            self.queue
                .consumer
                .counter()
                .store(self.cursor, Ordering::Release);
        }
    }
}

impl<T, const DEPTH: usize, C: SpscCursor> AffineSpscReceiver<'_, T, DEPTH, C> {
    /// Read at most `limit` of the prefix published now, in place. `each`
    /// returns whether to continue; its last value counts too. Values left
    /// in their cells are destroyed after the call, before freeing the cells
    /// together. To retain an owner, use a cell such as `Option<T>` and take it.
    ///
    /// No reference to a value escapes the call. A panic in `each` leaves
    /// that still-valid value queued; earlier values stay consumed. A panic
    /// in a value's destructor also consumes that value, exactly once.
    ///
    /// A cell loan cannot escape its callback:
    /// ```compile_fail,E0521
    /// use oer_portable_memory::AffineSpscQueue;
    /// let mut queue = AffineSpscQueue::<u32, 2>::new();
    /// let (send, mut receive) = queue.split();
    /// send.try_send(1).unwrap();
    /// let mut escaped = None;
    /// receive.consume_prefix(1, |cell| { escaped = Some(cell); true });
    /// ```
    pub fn consume_prefix(&mut self, limit: usize, mut each: impl FnMut(&mut T) -> bool) -> usize {
        if limit == 0 {
            return 0;
        }
        let consumer = self.queue.consumer.counter().load(Ordering::Relaxed);
        let producer = self.queue.producer.counter().load(Ordering::Acquire);
        let count = AffineSpscQueue::<T, DEPTH, C>::cursor_distance(consumer, producer)
            .min(DEPTH)
            .min(limit);
        let mut commit = Commit {
            queue: self.queue,
            cursor: consumer,
            taken: 0,
        };
        for _ in 0..count {
            let slot = self.queue.slot(commit.cursor);
            #[allow(
                unsafe_code,
                reason = "the exclusive consumer borrows one published prefix cell"
            )]
            // SAFETY: the Acquire snapshot includes this initialized cell.
            // The sole receiver is borrowed exclusively for the call; its
            // cursor has not passed the cell, so the producer cannot reuse it.
            let more = each(unsafe { (*slot.get()).assume_init_mut() });
            // Mark it consumed before destruction: if T::drop panics, the
            // guard still excludes this partially destroyed value from the
            // queue. A panic in `each` above instead leaves it initialized.
            commit.cursor = AffineSpscQueue::<T, DEPTH, C>::advance(commit.cursor);
            commit.taken = commit.taken.saturating_add(1);
            #[allow(
                unsafe_code,
                reason = "the prefix owns the initialized value until its destruction"
            )]
            // SAFETY: the callback's loan ended and left a valid T. This is
            // its one destruction; the private guard releases the cell even
            // if destruction unwinds, and publishes nothing before it ends.
            unsafe {
                (*slot.get()).assume_init_drop()
            };
            if !more {
                break;
            }
        }
        commit.taken
    }
}

#[cfg(test)]
#[allow(clippy::panic)]
mod tests {
    extern crate std;
    use super::*;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize},
    };
    use std::vec::Vec;

    struct Owner(usize, Arc<AtomicUsize>);
    impl Drop for Owner {
        fn drop(&mut self) {
            self.1.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn an_owning_prefix_retains_one_owner_and_drops_the_rest_once() {
        let drops = Arc::new(AtomicUsize::new(0));
        let mut queue = AffineSpscQueue::<Option<Owner>, 4>::new();
        let (send, mut receive) = queue.split();
        for id in 0..4 {
            assert!(send.try_send(Some(Owner(id, drops.clone()))).is_ok());
        }
        let mut kept = None;
        assert_eq!(
            receive.consume_prefix(3, |cell| {
                if cell.as_ref().unwrap().0 == 1 {
                    kept = cell.take();
                }
                true
            }),
            3
        );
        assert_eq!(drops.load(Ordering::SeqCst), 2);
        assert_eq!(receive.len(), 1);
        drop(kept);
        assert_eq!(receive.consume_prefix(4, |_| true), 1);
        assert_eq!(drops.load(Ordering::SeqCst), 4);
    }

    #[test]
    fn a_prefix_does_not_follow_publications_made_by_its_callback() {
        let mut queue = AffineSpscQueue::<usize, 4>::new();
        let (send, mut receive) = queue.split();
        send.try_send(1).unwrap();
        let mut seen = Vec::new();
        assert_eq!(
            receive.consume_prefix(4, |value| {
                seen.push(*value);
                send.try_send(2).unwrap();
                true
            }),
            1
        );
        assert_eq!(seen, [1]);
        assert_eq!(receive.try_receive().unwrap(), 2);
    }

    #[test]
    fn stopping_and_wrapping_keep_the_remaining_prefix_in_order() {
        let mut queue = AffineSpscQueue::<usize, 3>::new();
        let (send, mut receive) = queue.split();
        for base in 0..12 {
            for offset in 0..3 {
                send.try_send(base * 3 + offset).unwrap();
            }
            assert_eq!(
                receive.consume_prefix(0, |_| panic!("zero reads nothing")),
                0
            );
            assert_eq!(receive.consume_prefix(3, |_| false), 1);
            let mut rest = Vec::new();
            assert_eq!(
                receive.consume_prefix(9, |v| {
                    rest.push(*v);
                    true
                }),
                2
            );
            assert_eq!(rest, [base * 3 + 1, base * 3 + 2]);
        }
    }

    #[test]
    fn a_callback_panic_keeps_its_valid_cell_and_commits_the_completed_prefix() {
        let drops = Arc::new(AtomicUsize::new(0));
        let mut queue = AffineSpscQueue::<Option<Owner>, 3>::new();
        let (send, mut receive) = queue.split();
        for id in 0..3 {
            assert!(send.try_send(Some(Owner(id, drops.clone()))).is_ok());
        }
        let result = catch_unwind(AssertUnwindSafe(|| {
            receive.consume_prefix(3, |cell| {
                if cell.as_ref().unwrap().0 == 1 {
                    panic!("callback");
                }
                true
            })
        }));
        assert!(result.is_err());
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(receive.len(), 2);
        assert_eq!(receive.try_receive().unwrap().unwrap().0, 1);
        assert_eq!(receive.consume_prefix(3, |_| true), 1);
        assert_eq!(drops.load(Ordering::SeqCst), 3);
    }

    #[test]
    fn a_destructor_panic_never_exposes_the_destroyed_owner_again() {
        struct Panics(Arc<AtomicBool>, Arc<AtomicUsize>);
        impl Drop for Panics {
            fn drop(&mut self) {
                self.1.fetch_add(1, Ordering::SeqCst);
                if self.0.swap(false, Ordering::SeqCst) {
                    panic!("destructor");
                }
            }
        }
        let once = Arc::new(AtomicBool::new(true));
        let drops = Arc::new(AtomicUsize::new(0));
        let mut queue = AffineSpscQueue::<Panics, 2>::new();
        let (send, mut receive) = queue.split();
        for _ in 0..2 {
            assert!(send.try_send(Panics(once.clone(), drops.clone())).is_ok());
        }
        assert!(catch_unwind(AssertUnwindSafe(|| receive.consume_prefix(2, |_| true))).is_err());
        assert_eq!(receive.len(), 1);
        assert_eq!(receive.consume_prefix(2, |_| true), 1);
        assert_eq!(drops.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn concurrent_publications_and_in_place_prefixes_transfer_every_owner_once() {
        let drops = Arc::new(AtomicUsize::new(0));
        let mut queue = AffineSpscQueue::<Option<Owner>, 7>::new();
        let (send, mut receive) = queue.split();
        std::thread::scope(|threads| {
            let counted = drops.clone();
            threads.spawn(move || {
                for id in 0..512 {
                    let mut owner = Some(Owner(id, counted.clone()));
                    loop {
                        match send.try_send(owner) {
                            Ok(()) => break,
                            Err(held) => {
                                owner = held.0;
                                std::thread::yield_now();
                            }
                        }
                    }
                }
            });
            threads.spawn(move || {
                let mut next = 0;
                while next != 512 {
                    receive.consume_prefix(5, |cell| {
                        assert_eq!(cell.as_ref().unwrap().0, next);
                        next += 1;
                        true
                    });
                    std::thread::yield_now();
                }
            });
        });
        assert_eq!(drops.load(Ordering::SeqCst), 512);
    }
}
