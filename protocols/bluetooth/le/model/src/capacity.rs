//! FIFO over durable capacity and body-loaned seats. Only tokens cross the
//! private BodyLoan conversion; a borrowed waiter seat never has to move.
#![allow(
    clippy::indexing_slicing,
    reason = "the typed seats are minted once with exactly SEATS indices"
)]
use super::queues::{CAPACITY, SEATS};
use alloc::vec::Vec;
use core::{
    cell::{Cell, RefCell},
    future::poll_fn,
    task::{Poll, Waker},
};
use oer_bluetooth_le_port::CommandCapacity;
use oer_radio_port::{Child, Permit, PermitSupply, Root, SlotIdentity};

pub(crate) struct Pool<Id: SlotIdentity> {
    free: RefCell<Vec<Permit<'static, CommandCapacity, Child<Root<Id>, 0>>>>,
    waiters: RefCell<[Option<(u32, Waker)>; SEATS]>,
    next: Cell<u32>,
}
impl<Id: SlotIdentity> Pool<Id> {
    pub fn new(
        supply: PermitSupply<'static, CommandCapacity, CAPACITY, Child<Root<Id>, 0>>,
    ) -> Self {
        Self {
            free: RefCell::new(supply.into_permits().into_iter().collect()),
            waiters: RefCell::new([const { None }; SEATS]),
            next: Cell::new(0),
        }
    }
    fn leader(&self) -> Option<u32> {
        self.waiters
            .borrow()
            .iter()
            .flatten()
            .map(|(ticket, _)| *ticket)
            .min_by_key(|ticket| ticket.wrapping_sub(self.next.get()))
    }
    fn wake(&self) {
        if self.free.borrow().is_empty() {
            return;
        }
        if let Some(leader) = self.leader() {
            let wake = self
                .waiters
                .borrow()
                .iter()
                .flatten()
                .find(|(ticket, _)| *ticket == leader)
                .map(|(_, w)| w.clone());
            if let Some(w) = wake {
                w.wake();
            }
        }
    }
    fn leave(&self, index: usize, ticket: u32) {
        let mut waiters = self.waiters.borrow_mut();
        if waiters[index].as_ref().is_some_and(|(t, _)| *t == ticket) {
            waiters[index] = None;
        }
        drop(waiters);
        self.wake();
    }
    pub fn release(&self, permit: Permit<'static, CommandCapacity, Child<Root<Id>, 0>>) {
        self.free.borrow_mut().push(permit);
        self.wake();
    }
    pub async fn take(&self, index: u8) -> Permit<'static, CommandCapacity, Child<Root<Id>, 0>> {
        let mut place = Place {
            pool: self,
            index: usize::from(index),
            ticket: None,
        };
        poll_fn(move |cx| {
            let place = &mut place;
            let ticket = match place.ticket {
                Some(ticket) => ticket,
                None => {
                    let ticket = self.next.get();
                    self.next.set(ticket.wrapping_add(1));
                    place.ticket = Some(ticket);
                    ticket
                }
            };
            self.waiters.borrow_mut()[place.index] = Some((ticket, cx.waker().clone()));
            if self.leader() != Some(ticket) {
                return Poll::Pending;
            }
            let Some(permit) = self.free.borrow_mut().pop() else {
                return Poll::Pending;
            };
            self.leave(place.index, ticket);
            place.ticket = None;
            Poll::Ready(permit)
        })
        .await
    }
}
struct Place<'a, Id: SlotIdentity> {
    pool: &'a Pool<Id>,
    index: usize,
    ticket: Option<u32>,
}
impl<Id: SlotIdentity> Drop for Place<'_, Id> {
    fn drop(&mut self) {
        if let Some(ticket) = self.ticket {
            self.pool.leave(self.index, ticket);
        }
    }
}
