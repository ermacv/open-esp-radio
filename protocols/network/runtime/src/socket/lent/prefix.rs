//! The synchronous application prefix. It never awaits while it owns a
//! return batch: physical memory and its lend count precede socket credit.

use oer_portable_memory::BatchReturn;
use oer_protocols_network_core::Datagram;
use oer_protocols_network_port::EthernetFrame;

use super::{LendingSocket, Lent, Returns, SocketClosed, Waiting};

/// What one application receive prefix handed over. A copied datagram keeps
/// its ordinary block owner and must be released or sent by its caller.
#[must_use = "a copied prefix owns the block its caller must release or send"]
pub enum ReceivedPrefix<'m, B, const SIZE: usize> {
    /// The callback saw this many lent datagrams, in place. Each left in its
    /// cell was returned in this prefix; each taken remains its caller's.
    Lent(usize),
    /// A copy's turn: outside the ordinary lending loop, with its owner.
    Copied(Datagram<'m, B, SIZE>),
}

struct Credits<'a> {
    returns: &'a Returns,
    count: u32,
}

impl Drop for Credits<'_> {
    fn drop(&mut self) {
        self.returns.finished(self.count);
    }
}

struct Returning<'a, H: BatchReturn> {
    // Fields drop in this order: publish physical returns first, then the
    // credits which allow a service to lend more memory, and its continuation.
    frames: H::Batch,
    credits: Credits<'a>,
}

impl<H: BatchReturn> Returning<'_, H> {
    fn give(&mut self, cell: &mut Option<Lent<'_, H>>) {
        let Some(lent) = cell.as_ref() else {
            return;
        };
        // The callback can replace its cell with a retained owner from
        // another socket. That owner's physical return and credit stay
        // together on its individual Drop, outside this socket's batch.
        if !lent
            ._back
            .0
            .is_some_and(|returns| core::ptr::eq(returns, self.credits.returns))
        {
            return_substituted(cell);
            return;
        }
        let Some(Lent {
            frame, mut _back, ..
        }) = cell.take()
        else {
            return;
        };
        frame.return_in(&mut self.frames);
        // Only this socket's owners join its credit batch. A retained
        // owner keeps its Back and returns individually.
        _back.0 = None;
        self.credits.count = self.credits.count.saturating_add(1);
    }
}

// Keep the exceptional owner's extraction behind a borrowed-cell boundary:
// merging it with the ordinary field-wise return makes LLVM materialize
// intermediate frame owners even for this socket's own frames.
#[cold]
#[inline(never)]
fn return_substituted<H>(cell: &mut Option<Lent<'_, H>>) {
    drop(cell.take());
}

impl<
    'a,
    'm,
    B,
    const SIZE: usize,
    const DEPTH: usize,
    const RECEIVED: usize,
    const CREDITS: usize,
    H: EthernetFrame + BatchReturn,
> LendingSocket<'a, 'm, B, SIZE, DEPTH, RECEIVED, CREDITS, H>
{
    /// Process the available prefix, at most `limit`, without moving its
    /// lent owners through intermediate values. `each` borrows their cells;
    /// take an owner to retain it, or leave it to return with this prefix.
    /// Return `false` after the last datagram to process (for example Echo).
    ///
    /// The callback is synchronous. The prefix never waits for more frames,
    /// and finishes all unretained memory/credit returns before it returns.
    /// The two lanes retain their alternating fairness: while a copy waits,
    /// at most one loan precedes its own next prefix. Zero does no work.
    // Keep the bounded synchronous entry visible in machine code rather
    // than merging it into the application's much larger async poll.
    #[inline(never)]
    pub fn try_receive_prefix(
        &mut self,
        limit: usize,
        mut each: impl FnMut(&mut Option<Lent<'a, H>>) -> bool,
    ) -> Option<ReceivedPrefix<'m, B, SIZE>> {
        if limit == 0 {
            self.returns.finished(0);
            return Some(ReceivedPrefix::Lent(0));
        }
        if self.receive_control_pending() {
            self.receive_control();
        }
        let mut returning = Returning::<H> {
            frames: H::Batch::default(),
            credits: Credits {
                returns: self.returns,
                count: 0,
            },
        };
        let copies = self
            .socket
            .inbound
            .as_ref()
            .is_some_and(|inbound| !inbound.datagrams.is_empty());
        if self.copy_next && copies {
            self.copy_next = false;
            return self.socket.try_receive().map(ReceivedPrefix::Copied);
        }
        let mut handed = 0_usize;
        let _cells = self
            .lent
            .consume_prefix(if copies { 1 } else { limit }, |cell| {
                // A callback which unwound may have taken its owner, leaving a
                // valid empty cell. Remove that cell without inventing a datagram.
                if cell.is_none() {
                    return true;
                }
                handed = handed.saturating_add(1);
                let more = each(cell);
                returning.give(cell);
                more
            });
        if handed != 0 {
            self.copy_next = true;
            Some(ReceivedPrefix::Lent(handed))
        } else {
            self.copy_next = false;
            self.socket.try_receive().map(ReceivedPrefix::Copied)
        }
    }

    /// [`Self::try_receive_prefix`], waiting only until a prefix or close.
    /// A registered wait promises one synchronous servicing opportunity;
    /// the service defers further RX until this callback finishes, or the
    /// wait is cancelled. No promise waits for a retained frame to return.
    pub async fn receive_prefix(
        &mut self,
        limit: usize,
        mut each: impl FnMut(&mut Option<Lent<'a, H>>) -> bool,
    ) -> Result<ReceivedPrefix<'m, B, SIZE>, SocketClosed> {
        let mut waiting = Waiting(Some(self.returns));
        loop {
            self.returns.active();
            if let Some(prefix) = self.try_receive_prefix(limit, &mut each) {
                // The synchronous prefix already published returns and
                // released the servicing promise together.
                waiting.0 = None;
                return Ok(prefix);
            }
            if self.socket.closed {
                return Err(SocketClosed);
            }
            self.waiting().await?;
        }
    }
}
