//! P1-only FIFO credit pool. P2 never borrows this state.
use super::*;
use core::task::Waker;

pub(super) struct Pool<Id: SlotIdentity> {
    free: RefCell<Option<Credit<Id>>>,
    waiters: RefCell<[Option<(u32, Waker)>; WAITERS]>,
    next: Cell<u32>,
}
impl<Id: SlotIdentity> Pool<Id> {
    pub fn new(credit: Credit<Id>) -> Self {
        Self {
            free: RefCell::new(Some(credit)),
            waiters: RefCell::new([const { None }; WAITERS]),
            next: Cell::new(0),
        }
    }
    fn leader(&self) -> Option<u32> {
        self.waiters
            .borrow()
            .iter()
            .flatten()
            .map(|(n, _)| *n)
            .min_by_key(|n| n.wrapping_sub(self.next.get()))
    }
    fn wake(&self) {
        if self.free.borrow().is_none() {
            return;
        }
        let Some(leader) = self.leader() else {
            return;
        };
        let waker = self
            .waiters
            .borrow()
            .iter()
            .flatten()
            .find(|(n, _)| *n == leader)
            .map(|(_, w)| w.clone());
        if let Some(waker) = waker {
            waker.wake();
        }
    }
    fn leave(&self, index: usize, ticket: u32) {
        let mut waiters = self.waiters.borrow_mut();
        if let Some(slot) = waiters.get_mut(index) {
            if slot.as_ref().is_some_and(|(n, _)| *n == ticket) {
                *slot = None;
            }
        }
        drop(waiters);
        self.wake();
    }
    pub fn release(&self, credit: Credit<Id>) {
        assert!(self.free.borrow().is_none(), "one minted command credit");
        *self.free.borrow_mut() = Some(credit);
        self.wake();
    }
    pub async fn take(&self, index: u8) -> Credit<Id> {
        let mut place = Waiting {
            pool: self,
            index: usize::from(index),
            ticket: None,
        };
        poll_fn(|cx| self.poll_take(&mut place, cx)).await
    }
    // Keep the credit wait and its typed waker in a separate polling frame;
    // inlining into the complete HCI body loses the stack analyser's target.
    #[inline(never)]
    fn poll_take(&self, place: &mut Waiting<'_, Id>, cx: &mut Context<'_>) -> Poll<Credit<Id>> {
        let ticket = *place.ticket.get_or_insert_with(|| {
            let n = self.next.get();
            self.next.set(n.wrapping_add(1));
            n
        });
        let mut waiters = self.waiters.borrow_mut();
        let slot = waiters
            .get_mut(place.index)
            .expect("minted seat index fits its declared line");
        *slot = Some((ticket, cx.waker().clone()));
        drop(waiters);
        if self.leader() != Some(ticket) {
            return Poll::Pending;
        }
        let Some(credit) = self.free.borrow_mut().take() else {
            return Poll::Pending;
        };
        self.leave(place.index, ticket);
        place.ticket = None;
        Poll::Ready(credit)
    }
}
struct Waiting<'a, Id: SlotIdentity> {
    pool: &'a Pool<Id>,
    index: usize,
    ticket: Option<u32>,
}
impl<Id: SlotIdentity> Drop for Waiting<'_, Id> {
    fn drop(&mut self) {
        if let Some(ticket) = self.ticket {
            self.pool.leave(self.index, ticket);
        }
    }
}
