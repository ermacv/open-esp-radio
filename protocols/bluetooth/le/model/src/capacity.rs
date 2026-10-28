//! FIFO over durable capacity and body-loaned seats. Waiters carry seat
//! identities directly; cancellation removes the corresponding queue entry.
use super::{queues::CAPACITY, storage::Local};
use alloc::{collections::VecDeque, rc::Rc, vec::Vec};
use core::{
    future::poll_fn,
    task::{Poll, Waker},
};
use oer_bluetooth_le_port::CommandCapacity;
use oer_radio_port::{Child, Permit, PermitSupply, Root, SlotIdentity};

type Capacity<Id> = Permit<'static, CommandCapacity, Child<Root<Id>, 0>>;
struct Waiter {
    index: u8,
    waker: Rc<Waker>,
}
pub(crate) struct Pool<Id: SlotIdentity> {
    free: Local<Vec<Capacity<Id>>>,
    waiters: Local<VecDeque<Waiter>>,
}
impl<Id: SlotIdentity> Pool<Id> {
    pub const fn vacant() -> Self {
        Self {
            free: Local::new(Vec::new()),
            waiters: Local::new(VecDeque::new()),
        }
    }
    pub fn new(
        supply: PermitSupply<'static, CommandCapacity, CAPACITY, Child<Root<Id>, 0>>,
    ) -> Self {
        Self {
            free: Local::new(supply.into_permits().into_iter().collect()),
            waiters: Local::new(VecDeque::new()),
        }
    }
    fn wake(&self) {
        if self.free.with(|free| free.is_empty()) {
            return;
        }
        let wake = self
            .waiters
            .with(|waiters| waiters.front().map(|w| w.waker.clone()));
        if let Some(waker) = wake {
            waker.wake_by_ref();
        }
    }
    fn leave(&self, index: u8) {
        let removed = self.waiters.with(|waiters| {
            let position = waiters.iter().position(|w| w.index == index)?;
            waiters.remove(position)
        });
        drop(removed);
        self.wake();
    }
    pub fn release(&self, permit: Capacity<Id>) {
        self.free.with(|free| free.push(permit));
        self.wake();
    }
    pub async fn take(&self, index: u8) -> Capacity<Id> {
        let mut place = Place {
            pool: self,
            index,
            registered: false,
        };
        poll_fn(move |cx| {
            let place = &mut place;
            let waker = Rc::new(cx.waker().clone());
            let previous = self.waiters.with(|waiters| {
                if let Some(waiter) = waiters.iter_mut().find(|w| w.index == index) {
                    Some(core::mem::replace(&mut waiter.waker, waker))
                } else {
                    waiters.push_back(Waiter { index, waker });
                    None
                }
            });
            place.registered = true;
            drop(previous);
            if self
                .waiters
                .with(|waiters| waiters.front().map(|w| w.index))
                != Some(index)
            {
                return Poll::Pending;
            }
            let Some(permit) = self.free.with(Vec::pop) else {
                return Poll::Pending;
            };
            place.registered = false;
            self.leave(index);
            Poll::Ready(permit)
        })
        .await
    }
}
struct Place<'a, Id: SlotIdentity> {
    pool: &'a Pool<Id>,
    index: u8,
    registered: bool,
}
impl<Id: SlotIdentity> Drop for Place<'_, Id> {
    fn drop(&mut self) {
        if self.registered {
            self.pool.leave(self.index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::{future::Future, pin::pin, task::Context};
    #[test]
    fn waiter_identity_is_not_an_array_offset_and_cancellation_removes_it() {
        let pool = Pool::<oer_radio_port::LocalId<'static>>::vacant();
        {
            let mut waiter = pin!(pool.take(u8::MAX));
            assert!(
                waiter
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_pending()
            );
            assert_eq!(
                pool.waiters
                    .with(|waiters| waiters.front().map(|w| w.index)),
                Some(u8::MAX)
            );
        }
        assert!(pool.waiters.with(|waiters| waiters.is_empty()));
    }
}
