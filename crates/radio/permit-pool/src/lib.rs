#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! An allocation-free, first-in, first-out pool of a radio port install's
//! permits ([`oer_radio_port::PermitPool`]).
//!
//! A backend keeps a [`FifoPermitPool`] in its held part of the install
//! (`PortBackend::Held`) and fills it with permits its `resources` mint
//! ([`Permits`](oer_radio_port::Permits)), so every permit in
//! it is real capacity of that install. Its line holds a place for each
//! seat of its kind: the backend mints them with the pool's [`Line`]
//! ([`Permits`](oer_radio_port::Permits)) and hands each taker its
//! seat. Takers wait in arrival order, never refused:
//!
//! - a taker takes its seat's place in the line at its first poll, and is
//!   served once it leads the line and a permit is free;
//! - a released permit wakes the leader; a served taker that leaves a
//!   permit free wakes the next leader, so two releases before any taker
//!   runs serve two takers;
//! - a taker whose future drops gives up its place, and the taker behind it
//!   leads; its seat stays with it.
//!
//! The line has a place for every seat, so it is never full and arrival
//! order holds for every taker. Every component retains its sealed layout path. Even two pools of the
//! same kind have different permit and seat types; a foreign seat cannot
//! index this line. Separate permit pools may intentionally use the same
//! waiter domain by consuming their supplies with the same copyable Line.
//!
//! The pool is `!Sync`: it lives in one task, the port body's, and is
//! shared there by reference between the body's parts. Nothing outside the
//! task reaches it.
//!
//! No caller makes a permit, so no caller adds capacity:
//!
//! ```compile_fail,E0451
//! # use oer_radio_port::Permit;
//! fn forge<'id, K>() -> Permit<'id, K> {
//!     Permit { index: 0, brand: core::marker::PhantomData, kind: core::marker::PhantomData }
//! }
//! ```
//!
//! A pool of one install does not take another's permits, and its line
//! holds a place for exactly the seats minted with it:
//!
//! ```compile_fail
//! # use oer_radio_permit_pool::FifoPermitPool;
//! # use oer_radio_port::{Line, PermitSupply};
//! fn mix<'a, 'b, K>(supply: PermitSupply<'a, K, 1>, line: Line<'b, K, 2>) -> FifoPermitPool<'b, K, 1, 2> {
//!     FifoPermitPool::new(supply, line)
//! }
//! ```
//!
//! ```compile_fail,E0308
//! # use oer_radio_permit_pool::FifoPermitPool;
//! # use oer_radio_port::{Line, PermitSupply};
//! fn short<'id, K>(supply: PermitSupply<'id, K, 1>, line: Line<'id, K, 2>) -> FifoPermitPool<'id, K, 1, 1> {
//!     FifoPermitPool::new(supply, line)
//! }
//! ```
//!
//! Its compiling twin fills a pool with its own install's permits and the
//! line of its seats:
//!
//! ```
//! # use oer_radio_permit_pool::FifoPermitPool;
//! # use oer_radio_port::{Line, PermitSupply};
//! fn fill<'id, K>(supply: PermitSupply<'id, K, 1>, line: Line<'id, K, 2>) -> FifoPermitPool<'id, K, 1, 2> {
//!     FifoPermitPool::new(supply, line)
//! }
//! ```
//!
//! The pool is not `Sync`, so no other task shares it:
//!
//! ```compile_fail,E0277
//! # use oer_radio_permit_pool::FifoPermitPool;
//! fn shared<T: Sync>() {}
//! fn check<'id, K>() {
//!     shared::<FifoPermitPool<'id, K, 1, 2>>();
//! }
//! ```
//!
//! ```
//! # use oer_radio_permit_pool::FifoPermitPool;
//! fn owned<T>() {}
//! fn check<'id, K>() {
//!     owned::<FifoPermitPool<'id, K, 1, 2>>();
//! }
//! ```

use core::{
    cell::{Cell, RefCell},
    future::{Future, poll_fn},
    task::{Context, Poll, Waker},
};

use oer_radio_port::{Line, Permit, PermitPool, PermitSupply, PoolPath, Root, Seat};

/// A first-in, first-out pool of `PERMITS` permits of one install,
/// counting for `K`, with a line of a place for each of its `SEATS` seats.
pub struct FifoPermitPool<
    'id,
    K,
    const PERMITS: usize,
    const SEATS: usize,
    Pool: PoolPath = Root,
    Waiters: PoolPath = Pool,
> {
    /// The permits not held, at most `PERMITS`.
    free: RefCell<[Option<Permit<'id, K, Pool>>; PERMITS]>,
    /// The line: each seat's place, with the ticket and the waker of the
    /// taker waiting on it.
    line: RefCell<[Option<(u32, Waker)>; SEATS]>,
    /// Nominal authority for the seats accepted by this line.
    _authority: Line<'id, K, SEATS, Waiters>,
    /// The next ticket.
    next: Cell<u32>,
}

impl<'id, K, const PERMITS: usize, const SEATS: usize, Pool: PoolPath, Waiters: PoolPath>
    FifoPermitPool<'id, K, PERMITS, SEATS, Pool, Waiters>
{
    /// A pool of `permits`, every one free, whose line holds a place for
    /// each seat [`Permits`](oer_radio_port::Permits) made with
    /// `line`. A line with seats requires at least one permit: a zero-capacity
    /// waiter would have no operation whose terminal could ever wake it.
    ///
    /// ```compile_fail,E0080
    /// use oer_radio_permit_pool::FifoPermitPool;
    /// use oer_radio_port::{Mint, Permits};
    /// fn build(mint: Mint<'static, Permits<(), 0, 1>>) {
    ///     let tokens = mint.into_pools();
    ///     let _pool = FifoPermitPool::new(tokens.supply, tokens.line);
    /// }
    /// fn main() { let _: fn(Mint<'static, Permits<(), 0, 1>>) = build; }
    /// ```
    ///
    /// ```
    /// use oer_radio_permit_pool::FifoPermitPool;
    /// use oer_radio_port::{Mint, Permits};
    /// fn build(mint: Mint<'static, Permits<(), 1, 1>>) {
    ///     let tokens = mint.into_pools();
    ///     let _pool = FifoPermitPool::new(tokens.supply, tokens.line);
    /// }
    /// fn main() { let _: fn(Mint<'static, Permits<(), 1, 1>>) = build; }
    /// ```
    pub fn new(
        supply: PermitSupply<'id, K, PERMITS, Pool>,
        line: Line<'id, K, SEATS, Waiters>,
    ) -> Self {
        const {
            assert!(
                PERMITS != 0 || SEATS == 0,
                "FIFO waiters require nonzero permit capacity"
            );
        }
        Self {
            _authority: line,
            free: RefCell::new(supply.into_permits().map(Some)),
            line: RefCell::new([const { None }; SEATS]),
            next: Cell::new(0),
        }
    }

    /// The permits not held now.
    pub fn available(&self) -> usize {
        self.free.borrow().iter().flatten().count()
    }

    /// The takers waiting now.
    pub fn waiting(&self) -> usize {
        self.line.borrow().iter().flatten().count()
    }

    /// The ticket that leads the line.
    fn leader(&self) -> Option<u32> {
        self.line
            .borrow()
            .iter()
            .flatten()
            .map(|(ticket, _)| *ticket)
            .min_by_key(|ticket| ticket.wrapping_sub(self.next.get()))
    }

    /// Wake the leader of the line.
    fn wake_leader(&self) {
        let Some(leader) = self.leader() else {
            return;
        };
        if let Some((_, waker)) = self
            .line
            .borrow()
            .iter()
            .flatten()
            .find(|(ticket, _)| *ticket == leader)
        {
            waker.wake_by_ref();
        }
    }

    /// Give up the place of `seat`, held with `ticket`.
    fn leave(&self, seat: usize, ticket: u32) {
        let led = self.leader() == Some(ticket);
        {
            let mut line = self.line.borrow_mut();
            if line[seat].as_ref().is_some_and(|(held, _)| *held == ticket) {
                line[seat] = None;
            }
        }
        if led && self.available() > 0 {
            self.wake_leader();
        }
    }

    /// One poll of the taker on `seat`, holding `ticket`, or none yet.
    fn poll_take(
        &self,
        context: &mut Context<'_>,
        seat: usize,
        ticket: &mut Option<u32>,
    ) -> Poll<Permit<'id, K, Pool>> {
        let leader = self.leader();
        let (mine, replaced) = {
            let mut line = self.line.borrow_mut();
            let place = &mut line[seat];
            match (*ticket, place.as_mut()) {
                (Some(mine), Some((held, waker))) if *held == mine => {
                    waker.clone_from(context.waker());
                    (mine, None)
                }
                // The seat's first poll. A place its seat holds under
                // another ticket is a take that leaked without its drop
                // (`mem::forget`), which no one polls again: the seat's new
                // take replaces it.
                _ => {
                    let issued = self.next.get();
                    self.next.set(issued.wrapping_add(1));
                    let replaced = place.replace((issued, context.waker().clone()));
                    *ticket = Some(issued);
                    (issued, replaced.map(|(stale, _)| stale))
                }
            }
        };
        // A leaked leader no one polls again was the one a release woke:
        // with a permit free, wake the leader that follows it.
        if replaced.is_some() && replaced == leader && self.available() > 0 {
            self.wake_leader();
        }
        if self.leader() != Some(mine) {
            return Poll::Pending;
        }
        let served = self.free.borrow_mut().iter_mut().find_map(Option::take);
        let Some(permit) = served else {
            return Poll::Pending;
        };
        *ticket = None;
        self.leave(seat, mine);
        Poll::Ready(permit)
    }
}

/// A taker's place in a [`FifoPermitPool`]'s line, given up when its future
/// drops before it is served.
struct Place<
    'p,
    'id,
    K,
    const PERMITS: usize,
    const SEATS: usize,
    Pool: PoolPath,
    Waiters: PoolPath,
> {
    pool: &'p FifoPermitPool<'id, K, PERMITS, SEATS, Pool, Waiters>,
    seat: usize,
    ticket: Option<u32>,
}

impl<K, const PERMITS: usize, const SEATS: usize, Pool: PoolPath, Waiters: PoolPath> Drop
    for Place<'_, '_, K, PERMITS, SEATS, Pool, Waiters>
{
    fn drop(&mut self) {
        if let Some(ticket) = self.ticket.take() {
            self.pool.leave(self.seat, ticket);
        }
    }
}

impl<'id, K, const PERMITS: usize, const SEATS: usize, Pool: PoolPath, Waiters: PoolPath>
    PermitPool<'id, K, Pool, Waiters> for FifoPermitPool<'id, K, PERMITS, SEATS, Pool, Waiters>
{
    fn take<'a>(
        &'a self,
        seat: &'a mut Seat<'id, K, Waiters>,
    ) -> impl Future<Output = Permit<'id, K, Pool>> + 'a {
        // The line's nominal path admits only its own seats. `Permits`
        // numbers every seat below SEATS, so this index is in bounds.
        let mut place = Place {
            pool: self,
            seat: usize::from(seat.index()),
            ticket: None,
        };
        poll_fn(move |context| {
            // The seat stays borrowed as long as the future lives, and the
            // whole place moves into the future, so dropping the future
            // drops it.
            let place = &mut place;
            self.poll_take(context, place.seat, &mut place.ticket)
        })
    }

    fn release(&self, permit: Permit<'id, K, Pool>) {
        let mut free = self.free.borrow_mut();
        // The indivisible supply numbers its permits below PERMITS. The
        // installation brand and component path admit only that supply;
        // this affine permit's place is empty until its owner returns it.
        let index = usize::from(permit.index());
        free[index] = Some(permit);
        drop(free);
        self.wake_leader();
    }
}

#[cfg(test)]
mod tests;
