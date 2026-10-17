//! A host model of a network's TX source: software owners queued by Ethernet
//! destination, which a radio service selects from as it would from the
//! network stack's queues.
//!
//! It keeps the frames it is given, in order per destination, and hands each
//! out exactly once; the frame type is the caller's, so a test can observe
//! when the radio returns an owner by dropping it.

use core::{
    cell::RefCell,
    task::{Context, Poll, Waker},
};

use crate::{DestinationTxHead, DestinationTxQueues, SoftwareTxFrame, TxQueues};

/// Up to `N` frames, queued by their Ethernet destination.
pub struct MemoryTxQueues<F, const N: usize> {
    queues: RefCell<TxQueues<[u8; 6], F, N>>,
    waker: RefCell<Option<Waker>>,
}

impl<F: SoftwareTxFrame, const N: usize> Default for MemoryTxQueues<F, N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<F: SoftwareTxFrame, const N: usize> MemoryTxQueues<F, N> {
    pub const fn new() -> Self {
        Self {
            queues: RefCell::new(TxQueues::new()),
            waker: RefCell::new(None),
        }
    }

    /// Queue `frame` for its Ethernet destination and wake a waiting radio;
    /// the frame back when the queues are full or it has no destination.
    pub fn push(&self, frame: F) -> Result<(), F> {
        let Some(destination) = frame
            .ethernet()
            .get(..6)
            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
        else {
            return Err(frame);
        };
        self.queues.borrow_mut().push(destination, frame)?;
        if let Some(waker) = self.waker.borrow_mut().take() {
            waker.wake();
        }
        Ok(())
    }

    /// Frames waiting, for every destination.
    pub fn len(&self) -> usize {
        self.queues.borrow().len()
    }

    pub fn is_empty(&self) -> bool {
        self.queues.borrow().is_empty()
    }

    fn register(&self, context: &Context<'_>) {
        *self.waker.borrow_mut() = Some(context.waker().clone());
    }
}

impl<F: SoftwareTxFrame, const N: usize> DestinationTxQueues for MemoryTxQueues<F, N> {
    type Frame = F;

    fn next_head_after(&self, after: Option<[u8; 6]>) -> Option<([u8; 6], DestinationTxHead)> {
        let queues = self.queues.borrow();
        let (destination, frame, pending_frames) = queues.next_head_after(after)?;
        Some((
            destination,
            DestinationTxHead {
                ethernet_bytes: frame.ethernet().len(),
                pending_frames,
            },
        ))
    }

    fn head_for(&self, destination: [u8; 6]) -> Option<DestinationTxHead> {
        let queues = self.queues.borrow();
        let (frame, pending_frames) = queues.head(destination)?;
        Some(DestinationTxHead {
            ethernet_bytes: frame.ethernet().len(),
            pending_frames,
        })
    }

    fn try_take_for(&self, destination: [u8; 6]) -> Option<F> {
        self.queues.borrow_mut().pop(destination)
    }

    fn poll_ready_any(&self, context: &mut Context<'_>) -> Poll<()> {
        if self.is_empty() {
            self.register(context);
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    }

    fn poll_ready_for(
        &self,
        destination: [u8; 6],
        minimum: usize,
        context: &mut Context<'_>,
    ) -> Poll<()> {
        if self.queues.borrow().len_for(destination) >= minimum.max(1) {
            Poll::Ready(())
        } else {
            self.register(context);
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use core::task::Waker;

    use oer_network_interface::NetworkInterfaceId;

    use super::*;

    struct Frame([u8; 15]);

    impl SoftwareTxFrame for Frame {
        fn interface(&self) -> NetworkInterfaceId {
            NetworkInterfaceId::new(0)
        }

        fn ethernet(&self) -> &[u8] {
            &self.0
        }
    }

    fn frame(destination: u8, sequence: u8) -> Frame {
        let mut bytes = [0; 15];
        bytes[..6].fill(destination);
        bytes[14] = sequence;
        Frame(bytes)
    }

    #[test]
    fn frames_leave_in_order_per_destination_and_destinations_rotate() {
        let queues = MemoryTxQueues::<Frame, 4>::new();
        for (destination, sequence) in [(2, 1), (4, 2), (2, 3)] {
            assert!(queues.push(frame(destination, sequence)).is_ok());
        }
        let (first, head) = queues.next_head_after(None).unwrap();
        assert_eq!((first, head.pending_frames), ([2; 6], 2));
        assert_eq!(queues.next_head_after(Some([2; 6])).unwrap().0, [4; 6]);
        assert_eq!(queues.try_take_for([2; 6]).unwrap().0[14], 1);
        assert_eq!(queues.try_take_for([2; 6]).unwrap().0[14], 3);
        assert!(queues.try_take_for([2; 6]).is_none());
        assert_eq!(queues.head_for([4; 6]).unwrap().ethernet_bytes, 15);
        // Full queues hand the frame back.
        for sequence in 4..7 {
            assert!(queues.push(frame(6, sequence)).is_ok());
        }
        assert_eq!(queues.push(frame(6, 7)).err().unwrap().0[14], 7);
        assert_eq!(queues.len(), 4);
    }

    #[test]
    fn a_wait_for_any_destination_is_ready_once_a_frame_is_queued() {
        let queues = MemoryTxQueues::<Frame, 4>::new();
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(queues.poll_ready_any(&mut context), Poll::Pending);
        assert_eq!(
            queues.poll_ready_for([2; 6], 1, &mut context),
            Poll::Pending
        );
        queues.push(frame(2, 1)).ok().unwrap();
        assert_eq!(queues.poll_ready_any(&mut context), Poll::Ready(()));
        assert_eq!(
            queues.poll_ready_for([2; 6], 1, &mut context),
            Poll::Ready(())
        );
        assert_eq!(
            queues.poll_ready_for([2; 6], 2, &mut context),
            Poll::Pending
        );
    }
}
