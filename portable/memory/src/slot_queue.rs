//! A single-producer, single-consumer queue whose values stay in their
//! slots: the producer fills a free slot in place and the consumer reads
//! the oldest filled one in place, so a large value is written once and
//! never moved through the queue. Built from [`ClaimCell`] slots and two
//! affine queues of slot indices, it needs no unsafe code of its own.

use crate::{AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, ClaimCell};
use atomic_waker::AtomicWaker;
use core::future::poll_fn;
use core::task::Poll;

#[cfg(test)]
mod tests;

/// Slots, the indices of filled and free ones, and the wake state. The
/// composition splits it once and moves the two endpoints to their tasks.
pub struct WakingSlotQueue<T, const DEPTH: usize> {
    slots: [ClaimCell<T>; DEPTH],
    filled: AffineSpscQueue<u8, DEPTH>,
    free: AffineSpscQueue<u8, DEPTH>,
    consumer: AtomicWaker,
    producer: AtomicWaker,
    installed: bool,
}

/// Why a send filled no slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlotSendError<E> {
    /// Every slot is filled or being read.
    Full,
    /// The fill refused; the slot stays free.
    Refused(E),
    /// A slot the indices named was not free: a broken queue.
    Lost,
}

impl<T, const DEPTH: usize> WakingSlotQueue<T, DEPTH> {
    const FITS: () = {
        if DEPTH == 0 || DEPTH > u8::MAX as usize + 1 {
            panic!("a slot queue names its slots in u8 and has at least one");
        }
    };

    pub const fn new() -> Self {
        let () = Self::FITS;
        Self {
            slots: [const { ClaimCell::new() }; DEPTH],
            filled: const { AffineSpscQueue::new() },
            free: const { AffineSpscQueue::new() },
            consumer: AtomicWaker::new(),
            producer: AtomicWaker::new(),
            installed: false,
        }
    }

    /// Exclusively borrow the storage to grant its sole endpoint pair. The
    /// first split installs every slot with `init`'s value; a later one
    /// reuses the slots and receives the values its predecessor left
    /// filled, in their order. Every other slot is free again, including
    /// one a dropped producer held between a refused fill and its next.
    pub fn split(
        &mut self,
        mut init: impl FnMut() -> T,
    ) -> (SlotSender<'_, T, DEPTH>, SlotReceiver<'_, T, DEPTH>) {
        let (filled_send, filled_receive) = self.filled.split();
        let (free_send, free_receive) = self.free.split();
        if !self.installed {
            for slot in &self.slots {
                // A fresh queue: every slot is empty.
                let _installed = slot.install(init());
            }
            self.installed = true;
        }
        // Rebuild both index queues: the filled ones in their order, then
        // every other index free. Both have room for all `DEPTH`.
        let mut order = [0_u8; DEPTH];
        let mut filled = [false; DEPTH];
        let mut count = 0_usize;
        while let Ok(index) = filled_receive.try_receive() {
            if let (Some(seen), Some(place)) =
                (filled.get_mut(usize::from(index)), order.get_mut(count))
                && !*seen
            {
                *seen = true;
                *place = index;
                count = count.saturating_add(1);
            }
        }
        while free_receive.try_receive().is_ok() {}
        for &index in order.iter().take(count) {
            let _queued = filled_send.try_send(index);
        }
        for (index, seen) in (0_u8..).zip(filled) {
            if !seen {
                let _queued = free_send.try_send(index);
            }
        }
        (
            SlotSender {
                slots: &self.slots,
                filled: filled_send,
                free: free_receive,
                spare: None,
                consumer: &self.consumer,
                producer: &self.producer,
            },
            SlotReceiver {
                slots: &self.slots,
                filled: filled_receive,
                free: free_send,
                consumer: &self.consumer,
                producer: &self.producer,
            },
        )
    }
}

impl<T, const DEPTH: usize> Default for WakingSlotQueue<T, DEPTH> {
    fn default() -> Self {
        Self::new()
    }
}

/// The producer: fills free slots in place.
pub struct SlotSender<'a, T, const DEPTH: usize> {
    slots: &'a [ClaimCell<T>; DEPTH],
    filled: AffineSpscSender<'a, u8, DEPTH>,
    free: AffineSpscReceiver<'a, u8, DEPTH>,
    /// A free slot a refused fill left: the next fill takes it first.
    spare: Option<u8>,
    consumer: &'a AtomicWaker,
    producer: &'a AtomicWaker,
}

impl<T, const DEPTH: usize> SlotSender<'_, T, DEPTH> {
    /// Slots free now; a concurrent receive may free another.
    pub fn free_capacity(&self) -> usize {
        self.free
            .len()
            .saturating_add(usize::from(self.spare.is_some()))
    }

    /// Fill the oldest free slot with `fill` and publish it, waking the
    /// consumer. A refused fill leaves the slot free and publishes nothing.
    ///
    /// # Errors
    ///
    /// [`SlotSendError`]: no free slot, the fill's refusal, or a broken
    /// queue.
    pub fn try_send_with<R, E>(
        &mut self,
        fill: impl FnOnce(&mut T) -> Result<R, E>,
    ) -> Result<R, SlotSendError<E>> {
        let Some(index) = self.spare.take().or_else(|| self.free.try_receive().ok()) else {
            return Err(SlotSendError::Full);
        };
        let slot = self
            .slots
            .get(usize::from(index))
            .ok_or(SlotSendError::Lost)?;
        let filled = slot.lend(fill).map_err(|_| SlotSendError::Lost)?;
        let value = match filled {
            Ok(value) => value,
            Err(refused) => {
                // The slot stays free, for the next fill.
                self.spare = Some(index);
                return Err(SlotSendError::Refused(refused));
            }
        };
        // The index was free, so the filled queue has room for it.
        self.filled
            .try_send(index)
            .map_err(|_| SlotSendError::Lost)?;
        self.consumer.wake();
        Ok(value)
    }

    /// Wait until a slot is free. Cancellation consumes nothing.
    pub async fn ready(&mut self) {
        poll_fn(|cx| {
            self.producer.register(cx.waker());
            if self.spare.is_none() && self.free.is_empty() {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
    }
}

/// The consumer: reads the oldest filled slot in place.
pub struct SlotReceiver<'a, T, const DEPTH: usize> {
    slots: &'a [ClaimCell<T>; DEPTH],
    filled: AffineSpscReceiver<'a, u8, DEPTH>,
    free: AffineSpscSender<'a, u8, DEPTH>,
    consumer: &'a AtomicWaker,
    producer: &'a AtomicWaker,
}

/// Why a receive read no slot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SlotReceiveError {
    /// No slot is filled.
    Empty,
    /// A slot the indices named was not filled: a broken queue.
    Lost,
}

impl<T, const DEPTH: usize> SlotReceiver<'_, T, DEPTH> {
    /// Filled slots now.
    pub fn len(&self) -> usize {
        self.filled.len()
    }

    pub fn is_empty(&self) -> bool {
        self.filled.is_empty()
    }

    /// Run `read` on the oldest filled slot, then free it and wake the
    /// producer. The value stays in its slot for the next fill.
    ///
    /// # Errors
    ///
    /// [`SlotReceiveError`]: nothing filled, or a broken queue.
    pub fn try_receive_with<R>(
        &mut self,
        read: impl FnOnce(&mut T) -> R,
    ) -> Result<R, SlotReceiveError> {
        let index = self
            .filled
            .try_receive()
            .map_err(|_| SlotReceiveError::Empty)?;
        let slot = self
            .slots
            .get(usize::from(index))
            .ok_or(SlotReceiveError::Lost)?;
        let value = slot.lend(read).map_err(|_| SlotReceiveError::Lost)?;
        // The index was filled, so the free queue has room for it.
        self.free
            .try_send(index)
            .map_err(|_| SlotReceiveError::Lost)?;
        self.producer.wake();
        Ok(value)
    }

    /// Wait for a filled slot without reading it. Cancellation leaves it.
    pub async fn ready(&mut self) {
        poll_fn(|cx| {
            self.consumer.register(cx.waker());
            if self.filled.is_empty() {
                Poll::Pending
            } else {
                Poll::Ready(())
            }
        })
        .await;
    }
}
