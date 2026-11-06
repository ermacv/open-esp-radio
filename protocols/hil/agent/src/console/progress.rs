//! Completion of USB event writes, independently of producer scheduling.

use core::{
    cell::Cell,
    future::poll_fn,
    task::{Poll, Waker},
};

use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};

const RECEIVERS: usize = 16;

/// An impossible terminal-barrier fan-out. The image retains these observations
/// and resets without performing the action guarded by the failed barrier.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReceiverLimit {
    pub sequence: u32,
    pub next: Option<u32>,
    pub capacity: usize,
}

/// One waiter's place, held from its first poll until its future ends.
struct Place {
    taken: Cell<bool>,
    waker: Cell<Option<Waker>>,
}

struct Progress {
    next: Cell<u32>,
    places: [Place; RECEIVERS],
}

pub struct SerializedEvents {
    progress: Mutex<CriticalSectionRawMutex, Progress>,
}

impl Default for SerializedEvents {
    fn default() -> Self {
        Self::new()
    }
}

/// A place held by a waiter; dropping it, a cancellation too, gives it back.
struct Seat<'a> {
    events: &'a SerializedEvents,
    index: usize,
}

impl Drop for Seat<'_> {
    fn drop(&mut self) {
        let _released = self.events.progress.lock(|progress| {
            progress.places.get(self.index).and_then(|place| {
                place.taken.set(false);
                place.waker.take()
            })
        });
    }
}

impl SerializedEvents {
    pub const fn new() -> Self {
        Self {
            progress: Mutex::new(Progress {
                next: Cell::new(0),
                places: [const {
                    Place {
                        taken: Cell::new(false),
                        waker: Cell::new(None),
                    }
                }; RECEIVERS],
            }),
        }
    }

    pub fn publish_next(&self, next: u32) {
        // Wakers run and drop after the lock: nothing foreign runs under it.
        let wakers = self.progress.lock(|progress| {
            progress.next.set(next);
            progress.places.each_ref().map(|place| place.waker.take())
        });
        for waker in wakers.into_iter().flatten() {
            waker.wake();
        }
    }

    pub async fn wait_for(&self, sequence: u32) -> Result<(), ReceiverLimit> {
        let target = sequence.wrapping_add(1);
        let seat = self.seat(sequence)?;
        poll_fn(|cx| {
            let waker = cx.waker().clone();
            // Whichever waker leaves here drops after the lock.
            let (poll, _stale) = self.progress.lock(|progress| {
                let next = progress.next.get();
                // The writer can serialize several events before this producer
                // runs. Sequence order is unambiguous within the bounded
                // in-flight window.
                if next.wrapping_sub(target) < (1 << 31) {
                    return (Poll::Ready(Ok(())), Some(waker));
                }
                match progress.places.get(seat.index) {
                    Some(place) => (Poll::Pending, place.waker.replace(Some(waker))),
                    None => (Poll::Ready(Err(limit(sequence, next))), Some(waker)),
                }
            });
            poll
        })
        .await
    }

    fn seat(&self, sequence: u32) -> Result<Seat<'_>, ReceiverLimit> {
        self.progress
            .lock(|progress| {
                let free = progress.places.iter().position(|place| !place.taken.get());
                let place = free.and_then(|index| progress.places.get(index));
                match (free, place) {
                    (Some(index), Some(place)) => {
                        place.taken.set(true);
                        Ok(index)
                    }
                    _ => Err(limit(sequence, progress.next.get())),
                }
            })
            .map(|index| Seat {
                events: self,
                index,
            })
    }
}

fn limit(sequence: u32, next: u32) -> ReceiverLimit {
    ReceiverLimit {
        sequence,
        next: Some(next),
        capacity: RECEIVERS,
    }
}
