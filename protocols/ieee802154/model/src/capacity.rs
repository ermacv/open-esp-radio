//! FIFO capacity. A borrowed seat identifies a waiter without a ticket counter.
use super::storage::Local;
use alloc::{collections::VecDeque, rc::Rc, vec::Vec};
use core::{
    future::{Future, poll_fn},
    task::{Poll, Waker},
};
use oer_radio_port::{
    BodyLoan, LocalId, Permit, PermitPool, PermitSupply, PoolPath, Root, Seat, SlotOwner,
};

struct Waiter {
    seat: u8,
    waker: Rc<Waker>,
}

/// A first-in, first-out pool of the model's permits, with a place for every
/// borrowed seat. Cancelling a taker removes its place without consuming capacity.
pub struct ModelPermitPool<'id, Kind, Path: PoolPath = Root<LocalId<'id>>> {
    free: Local<Vec<Permit<'id, Kind, Path>>>,
    line: Local<VecDeque<Waiter>>,
}

impl<'id, Kind, Path: PoolPath> ModelPermitPool<'id, Kind, Path> {
    /// A pool owning the supplied capacity.
    pub fn new<const N: usize>(supply: PermitSupply<'id, Kind, N, Path>) -> Self {
        Self {
            free: Local::new(supply.into_permits().into_iter().collect()),
            line: Local::default(),
        }
    }

    fn wake_head(&self) {
        if self.free.with(|free| free.is_empty()) {
            return;
        }
        let head = self
            .line
            .with(|line| line.front().map(|waiter| waiter.waker.clone()));
        if let Some(waker) = head {
            waker.wake_by_ref();
        }
    }

    fn leave(&self, seat: u8) {
        let removed = self.line.with(|line| {
            let position = line.iter().position(|waiter| waiter.seat == seat)?;
            line.remove(position)
        });
        drop(removed);
        self.wake_head();
    }

    fn taking(&self, seat: u8) -> impl Future<Output = Permit<'id, Kind, Path>> + '_ {
        let mut place = Place {
            pool: self,
            seat,
            registered: false,
        };
        poll_fn(move |context| {
            let place = &mut place;
            // RawWaker clone/drop callbacks may reenter the pool. All queue
            // ownership is published before dropping the old registration.
            let waker = Rc::new(context.waker().clone());
            let previous = self.line.with(|line| {
                if let Some(waiter) = line.iter_mut().find(|waiter| waiter.seat == seat) {
                    Some(core::mem::replace(&mut waiter.waker, waker))
                } else {
                    line.push_back(Waiter { seat, waker });
                    None
                }
            });
            place.registered = true;
            drop(previous);
            if self
                .line
                .with(|line| line.front().map(|waiter| waiter.seat))
                != Some(seat)
            {
                return Poll::Pending;
            }
            let Some(permit) = self.free.with(Vec::pop) else {
                return Poll::Pending;
            };
            place.registered = false;
            self.leave(seat);
            Poll::Ready(permit)
        })
    }
}

struct Place<'a, 'id, Kind, Path: PoolPath> {
    pool: &'a ModelPermitPool<'id, Kind, Path>,
    seat: u8,
    registered: bool,
}

impl<Kind, Path: PoolPath> Drop for Place<'_, '_, Kind, Path> {
    fn drop(&mut self) {
        if self.registered {
            self.pool.leave(self.seat);
        }
    }
}

impl<'id, Kind, Path: PoolPath> PermitPool<'id, Kind, Path> for ModelPermitPool<'id, Kind, Path> {
    fn take<'a>(
        &'a self,
        seat: &'a mut Seat<'id, Kind, Path>,
    ) -> impl Future<Output = Permit<'id, Kind, Path>> + 'a {
        self.taking(seat.index())
    }

    fn release(&self, permit: Permit<'id, Kind, Path>) {
        self.free.with(|free| free.push(permit));
        self.wake_head();
    }
}

impl<Kind, Path: PoolPath> ModelPermitPool<'static, Kind, Path> {
    pub(crate) async fn take_loaned<'a, 'p: 'a, S: SlotOwner<Path::Identity> + 'a>(
        &'a self,
        seat: &'a mut Seat<'p, Kind, Path>,
        loan: BodyLoan<'p, Path::Identity, S>,
    ) -> Permit<'p, Kind, Path> {
        loan.lend_permit(self.taking(seat.index()).await)
    }
}
