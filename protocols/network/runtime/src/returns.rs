//! A block's way home with no socket, and how that way closes.
//!
//! Order between [`Returner::give`] and closing is fixed by one rule: `give`
//! stores its block in a slot, then looks at `closed` again. Closing sets `closed`,
//! then reads the slots. A fence stands between each pair, so at least one side sees
//! the other: `give` takes its own block back and refuses, or the closer finds it.
//! So a `give` that says `Ok` left its block where whoever reads next will find it,
//! and one that refuses still has it. No order leaves a block with neither.
//!
//! Slots stay reachable after closing: if nobody drained them, [`Returner::reclaim`]
//! still reaches what they hold.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering, fence};
use core::task::Context;

use oer_portable_memory::{AtomicWaker, ClaimCell};
use oer_protocols_network_core::{Block, Pool};

/// One block given home, and the `give` that left it there.
struct Given<'m, B, const SIZE: usize> {
    block: Block<'m, B, SIZE>,
    ticket: u32,
}

/// The storage of a way home: a slot for every block of a pool.
pub struct ReturnLane<'m, B, const SIZE: usize, const BLOCKS: usize> {
    slots: [ClaimCell<Option<Given<'m, B, SIZE>>>; BLOCKS],
    /// Final: the receiving end has gone or closed.
    closed: AtomicBool,
    /// A block may wait: a hint for the reader, set after a slot is filled.
    pending: AtomicBool,
    /// Names each `give`, so one takes back only its own block.
    tickets: AtomicU32,
    waker: AtomicWaker,
}

impl<B, const SIZE: usize, const BLOCKS: usize> ReturnLane<'_, B, SIZE, BLOCKS> {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            slots: [const { ClaimCell::new() }; BLOCKS],
            closed: AtomicBool::new(false),
            pending: AtomicBool::new(false),
            tickets: AtomicU32::new(0),
            waker: AtomicWaker::new(),
        }
    }
}

impl<B, const SIZE: usize, const BLOCKS: usize> Default for ReturnLane<'_, B, SIZE, BLOCKS> {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a, 'm, B, const SIZE: usize, const BLOCKS: usize> ReturnLane<'m, B, SIZE, BLOCKS> {
    /// The lane's two sides: any number of copies of the [`Returner`], and the
    /// one [`ReturnEnd`], which moves with whoever holds the pool.
    pub fn split(
        &'a mut self,
    ) -> (
        Returner<'a, 'm, B, SIZE, BLOCKS>,
        ReturnEnd<'a, 'm, B, SIZE, BLOCKS>,
    ) {
        for slot in &self.slots {
            // `&mut` for 'a means this runs once on a lane no one else reaches.
            let _first = slot.install(None);
        }
        let lane = &*self;
        (Returner { lane }, ReturnEnd { lane })
    }

    /// Store `block` under `ticket`, in the first slot free for this call.
    fn store(&self, block: Block<'m, B, SIZE>, ticket: u32) -> Result<usize, Block<'m, B, SIZE>> {
        let mut block = Some(block);
        let mut last = 0;
        for (index, slot) in self.slots.iter().enumerate() {
            last = index;
            let _free_or_held = slot.lend(|place| {
                if place.is_none() {
                    *place = block.take().map(|block| Given { block, ticket });
                }
            });
            if block.is_none() {
                return Ok(index);
            }
        }
        // Past the loop `block` is here: a store returns at once.
        block.map_or(Ok(last), Err)
    }

    /// Take the block of `ticket` back from slot `index`, if still there.
    fn take_back(&self, index: usize, ticket: u32) -> Option<Block<'m, B, SIZE>> {
        let slot = self.slots.get(index)?;
        slot.lend(|place| {
            if place.as_ref().is_some_and(|given| given.ticket == ticket) {
                place.take().map(|given| given.block)
            } else {
                None
            }
        })
        .ok()
        .flatten()
    }

    /// The next block in any slot.
    fn take_any(&self) -> Option<Block<'m, B, SIZE>> {
        self.slots.iter().find_map(|slot| {
            let taken = slot.lend(|place| place.take().map(|given| given.block));
            taken.ok().flatten()
        })
    }

    fn drain<const POOL: usize>(&self, pool: &mut Pool<'m, B, SIZE, POOL>) -> bool {
        let mut moved = false;
        while let Some(block) = self.take_any() {
            pool.give(block);
            moved = true;
        }
        moved
    }

    fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
        fence(Ordering::SeqCst);
    }

    /// As [`Returner::give`], with the moments a higher context could run between
    /// its steps named, for a test to stand in one.
    fn give_at(
        &self,
        block: Block<'m, B, SIZE>,
        after_check: impl FnOnce(),
        after_store: impl FnOnce(),
    ) -> Result<(), NotHome<'m, B, SIZE>> {
        if self.closed.load(Ordering::Acquire) {
            return Err(NotHome {
                reason: ReturnRefusal::Closed,
                block,
            });
        }
        after_check();
        let ticket = self.tickets.fetch_add(1, Ordering::Relaxed);
        let index = match self.store(block, ticket) {
            Ok(index) => index,
            Err(block) => {
                return Err(NotHome {
                    reason: ReturnRefusal::Busy,
                    block,
                });
            }
        };
        after_store();
        fence(Ordering::SeqCst);
        if self.closed.load(Ordering::SeqCst)
            && let Some(block) = self.take_back(index, ticket)
        {
            return Err(NotHome {
                reason: ReturnRefusal::Closed,
                block,
            });
        }
        self.pending.store(true, Ordering::Release);
        self.waker.wake();
        Ok(())
    }
}

/// Why a block did not go home.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReturnRefusal {
    /// The receiving end has gone, for good: this block stays with its caller.
    Closed,
    /// No slot was free for this call: every block the lane could take is in it,
    /// but for a slot a context below this one holds for one operation, as a
    /// reader scanning does. That operation ends when this context yields, so
    /// a later `give` finds the slot.
    Busy,
}

/// A block that did not go home, back with its caller.
#[derive(Debug)]
pub struct NotHome<'m, B, const SIZE: usize> {
    pub reason: ReturnRefusal,
    pub block: Block<'m, B, SIZE>,
}

/// A way home for blocks, to whichever service holds the pool now.
pub struct Returner<'a, 'm, B, const SIZE: usize, const BLOCKS: usize> {
    lane: &'a ReturnLane<'m, B, SIZE, BLOCKS>,
}

impl<B, const SIZE: usize, const BLOCKS: usize> Clone for Returner<'_, '_, B, SIZE, BLOCKS> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<B, const SIZE: usize, const BLOCKS: usize> Copy for Returner<'_, '_, B, SIZE, BLOCKS> {}

impl<'m, B, const SIZE: usize, const BLOCKS: usize> Returner<'_, 'm, B, SIZE, BLOCKS> {
    /// Send `block` home. `Ok` is the acceptance point: from there it is the
    /// lane's, for the receiving end, or for [`Self::reclaim`] once that has gone.
    ///
    /// # Errors
    /// [`NotHome`], with the block.
    pub fn give(&self, block: Block<'m, B, SIZE>) -> Result<(), NotHome<'m, B, SIZE>> {
        self.lane.give_at(block, || {}, || {})
    }

    /// Once the lane has closed, a block it still holds, one call each. Open, it
    /// holds them for the receiving end, and this gives none.
    #[must_use]
    pub fn reclaim(&self) -> Option<Block<'m, B, SIZE>> {
        if !self.lane.closed.load(Ordering::Acquire) {
            return None;
        }
        self.lane.take_any()
    }
}

/// The receiving end of a [`ReturnLane`], which moves with the pool's holder from
/// service to service. Moving it closes nothing: the lane closes when this ends.
pub struct ReturnEnd<'a, 'm, B, const SIZE: usize, const BLOCKS: usize> {
    lane: &'a ReturnLane<'m, B, SIZE, BLOCKS>,
}

impl<B, const SIZE: usize, const BLOCKS: usize> Drop for ReturnEnd<'_, '_, B, SIZE, BLOCKS> {
    fn drop(&mut self) {
        self.lane.close();
    }
}

impl<'m, B, const SIZE: usize, const BLOCKS: usize> ReturnEnd<'_, 'm, B, SIZE, BLOCKS> {
    /// Bring home what has arrived; whether anything had.
    pub(crate) fn take_home<const POOL: usize>(&self, pool: &mut Pool<'m, B, SIZE, POOL>) -> bool {
        self.lane.pending.swap(false, Ordering::AcqRel) && self.lane.drain(pool)
    }

    /// Whether a block waits; `cx` hears the next.
    pub(crate) fn poll_waiting(&self, cx: &Context<'_>) -> bool {
        self.lane.waker.register(cx.waker());
        self.lane.pending.load(Ordering::Acquire)
    }

    /// Close the lane for good and bring its blocks home, so what was accepted
    /// outlives this end in `pool`. A later `give` is refused.
    pub fn close<const POOL: usize>(self, pool: &mut Pool<'m, B, SIZE, POOL>) {
        self.lane.close();
        let _any = self.lane.drain(pool);
    }
}

#[cfg(test)]
mod tests;
