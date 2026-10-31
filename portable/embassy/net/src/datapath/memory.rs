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

use super::{DestinationTxHead, DestinationTxQueues, SoftwareTxFrame, TxQueues};

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

/// A model operation refused without changing queued ownership.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemoryQueueError {
    QueueBorrowed,
    WakerBorrowed,
    MissingDestination,
    Capacity,
}

impl<F: SoftwareTxFrame, const N: usize> MemoryTxQueues<F, N> {
    pub const fn new() -> Self {
        Self {
            queues: RefCell::new(TxQueues::new()),
            waker: RefCell::new(None),
        }
    }

    /// Queue a frame, retaining it on every refusal, including reentrant access.
    pub fn push(&self, frame: F) -> Result<(), (MemoryQueueError, F)> {
        let Some(destination) = frame
            .ethernet()
            .get(..6)
            .and_then(|bytes| <[u8; 6]>::try_from(bytes).ok())
        else {
            return Err((MemoryQueueError::MissingDestination, frame));
        };
        let Ok(mut queues) = self.queues.try_borrow_mut() else {
            return Err((MemoryQueueError::QueueBorrowed, frame));
        };
        let Ok(mut waker) = self.waker.try_borrow_mut() else {
            return Err((MemoryQueueError::WakerBorrowed, frame));
        };
        queues
            .push(destination, frame)
            .map_err(|frame| (MemoryQueueError::Capacity, frame))?;
        let wake = waker.take();
        drop(waker);
        drop(queues);
        if let Some(wake) = wake {
            wake.wake();
        }
        Ok(())
    }

    pub fn len(&self) -> Result<usize, MemoryQueueError> {
        Ok(self
            .queues
            .try_borrow()
            .map_err(|_| MemoryQueueError::QueueBorrowed)?
            .len())
    }

    pub fn is_empty(&self) -> Result<bool, MemoryQueueError> {
        Ok(self
            .queues
            .try_borrow()
            .map_err(|_| MemoryQueueError::QueueBorrowed)?
            .is_empty())
    }

    fn register(&self, context: &Context<'_>) -> Result<(), MemoryQueueError> {
        // Custom clone/drop callbacks may reenter. Neither runs under a borrow.
        let next = context.waker().clone();
        let previous = self
            .waker
            .try_borrow_mut()
            .map_err(|_| MemoryQueueError::WakerBorrowed)?
            .replace(next);
        drop(previous);
        Ok(())
    }
}

impl<F: SoftwareTxFrame, const N: usize> DestinationTxQueues for MemoryTxQueues<F, N> {
    type Frame = F;
    type Error = MemoryQueueError;

    fn next_head_after(
        &self,
        after: Option<[u8; 6]>,
    ) -> Result<Option<([u8; 6], DestinationTxHead)>, Self::Error> {
        let queues = self
            .queues
            .try_borrow()
            .map_err(|_| MemoryQueueError::QueueBorrowed)?;
        Ok(queues
            .next_head_after(after)
            .map(|(destination, frame, pending_frames)| {
                (
                    destination,
                    DestinationTxHead {
                        ethernet_bytes: frame.ethernet().len(),
                        pending_frames,
                    },
                )
            }))
    }

    fn head_for(&self, destination: [u8; 6]) -> Result<Option<DestinationTxHead>, Self::Error> {
        let queues = self
            .queues
            .try_borrow()
            .map_err(|_| MemoryQueueError::QueueBorrowed)?;
        Ok(queues
            .head(destination)
            .map(|(frame, pending_frames)| DestinationTxHead {
                ethernet_bytes: frame.ethernet().len(),
                pending_frames,
            }))
    }

    fn try_take_for(&self, destination: [u8; 6]) -> Result<Option<F>, Self::Error> {
        Ok(self
            .queues
            .try_borrow_mut()
            .map_err(|_| MemoryQueueError::QueueBorrowed)?
            .pop(destination))
    }

    fn poll_ready_any(&self, context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        if self.is_empty()? {
            self.register(context)?;
            Poll::Pending
        } else {
            Poll::Ready(Ok(()))
        }
    }

    fn poll_ready_for(
        &self,
        destination: [u8; 6],
        minimum: usize,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), Self::Error>> {
        let ready = self
            .queues
            .try_borrow()
            .map_err(|_| MemoryQueueError::QueueBorrowed)?
            .len_for(destination)
            >= minimum.max(1);
        if ready {
            Poll::Ready(Ok(()))
        } else {
            self.register(context)?;
            Poll::Pending
        }
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_macros, clippy::disallowed_methods)]
mod tests {
    use core::task::Waker;

    use oer_network_port::NetworkInterfaceId;

    use super::*;

    pub(super) struct Frame(pub(super) [u8; 15]);

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
        let (first, head) = queues.next_head_after(None).unwrap().unwrap();
        assert_eq!((first, head.pending_frames), ([2; 6], 2));
        assert_eq!(
            queues.next_head_after(Some([2; 6])).unwrap().unwrap().0,
            [4; 6]
        );
        assert_eq!(queues.try_take_for([2; 6]).unwrap().unwrap().0[14], 1);
        assert_eq!(queues.try_take_for([2; 6]).unwrap().unwrap().0[14], 3);
        assert!(queues.try_take_for([2; 6]).unwrap().is_none());
        assert_eq!(queues.head_for([4; 6]).unwrap().unwrap().ethernet_bytes, 15);
        // Full queues hand the frame back.
        for sequence in 4..7 {
            assert!(queues.push(frame(6, sequence)).is_ok());
        }
        assert_eq!(queues.push(frame(6, 7)).err().unwrap().1.0[14], 7);
        assert_eq!(queues.len().unwrap(), 4);
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
        assert_eq!(queues.poll_ready_any(&mut context), Poll::Ready(Ok(())));
        assert_eq!(
            queues.poll_ready_for([2; 6], 1, &mut context),
            Poll::Ready(Ok(()))
        );
        assert_eq!(
            queues.poll_ready_for([2; 6], 2, &mut context),
            Poll::Pending
        );
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_macros, clippy::disallowed_methods)]
mod regressions {
    use super::{tests::Frame, *};

    #[test]
    fn borrowed_queue_returns_errors_without_consuming_the_input() {
        let queues = MemoryTxQueues::<Frame, 2>::new();
        let held = queues.queues.borrow_mut();
        let (error, frame) = queues.push(Frame([7; 15])).err().unwrap();
        assert_eq!(error, MemoryQueueError::QueueBorrowed);
        assert_eq!(frame.0, [7; 15]);
        assert_eq!(queues.len(), Err(MemoryQueueError::QueueBorrowed));
        assert_eq!(queues.is_empty(), Err(MemoryQueueError::QueueBorrowed));
        assert_eq!(
            queues.head_for([7; 6]),
            Err(MemoryQueueError::QueueBorrowed)
        );
        assert_eq!(
            queues.next_head_after(None),
            Err(MemoryQueueError::QueueBorrowed)
        );
        assert!(matches!(
            queues.try_take_for([7; 6]),
            Err(MemoryQueueError::QueueBorrowed)
        ));
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(
            queues.poll_ready_any(&mut cx),
            Poll::Ready(Err(MemoryQueueError::QueueBorrowed))
        );
        assert_eq!(
            queues.poll_ready_for([7; 6], 1, &mut cx),
            Poll::Ready(Err(MemoryQueueError::QueueBorrowed))
        );
        drop(held);
        queues.push(frame).ok().unwrap();
        assert_eq!(queues.try_take_for([7; 6]).unwrap().unwrap().0, [7; 15]);
    }

    #[test]
    fn borrowed_waker_refuses_publication_and_wait_registration_atomically() {
        let queues = MemoryTxQueues::<Frame, 2>::new();
        let held = queues.waker.borrow_mut();
        let (error, frame) = queues.push(Frame([8; 15])).err().unwrap();
        assert_eq!(error, MemoryQueueError::WakerBorrowed);
        assert_eq!(queues.len(), Ok(0));
        let mut cx = Context::from_waker(Waker::noop());
        assert_eq!(
            queues.poll_ready_any(&mut cx),
            Poll::Ready(Err(MemoryQueueError::WakerBorrowed))
        );
        assert_eq!(
            queues.poll_ready_for([8; 6], 1, &mut cx),
            Poll::Ready(Err(MemoryQueueError::WakerBorrowed))
        );
        drop(held);
        queues.push(frame).ok().unwrap();
        assert_eq!(queues.len(), Ok(1));
    }

    #[test]
    fn malformed_destination_and_capacity_refusals_preserve_frames() {
        struct Short;
        impl SoftwareTxFrame for Short {
            fn interface(&self) -> oer_network_port::NetworkInterfaceId {
                oer_network_port::NetworkInterfaceId::new(0)
            }
            fn ethernet(&self) -> &[u8] {
                &[1; 5]
            }
        }
        let queues = MemoryTxQueues::<Short, 1>::new();
        assert!(matches!(
            queues.push(Short),
            Err((MemoryQueueError::MissingDestination, Short))
        ));
        let queues = MemoryTxQueues::<Frame, 1>::new();
        queues.push(Frame([1; 15])).ok().unwrap();
        let (error, frame) = queues.push(Frame([2; 15])).err().unwrap();
        assert_eq!(error, MemoryQueueError::Capacity);
        assert_eq!(frame.0, [2; 15]);
        assert_eq!(queues.try_take_for([1; 6]).unwrap().unwrap().0, [1; 15]);
    }
    #[test]
    fn reentrant_frame_inspection_refuses_mutation_and_returns_the_exact_owner() {
        extern crate std;
        use std::{cell::Cell, rc::Rc};
        struct Frame {
            bytes: [u8; 6],
            inspect: Option<Rc<dyn Fn()>>,
        }
        impl SoftwareTxFrame for Frame {
            fn interface(&self) -> oer_network_port::NetworkInterfaceId {
                oer_network_port::NetworkInterfaceId::new(0)
            }
            fn ethernet(&self) -> &[u8] {
                if let Some(inspect) = &self.inspect {
                    inspect();
                }
                &self.bytes
            }
        }
        let queues = Rc::new(MemoryTxQueues::<Frame, 2>::new());
        let weak = Rc::downgrade(&queues);
        let armed = Rc::new(Cell::new(false));
        let returned = Rc::new(RefCell::new(None));
        let inspect = {
            let armed = armed.clone();
            let returned = returned.clone();
            Rc::new(move || {
                if armed.replace(false) {
                    let frame = Frame {
                        bytes: [9; 6],
                        inspect: None,
                    };
                    *returned.borrow_mut() = Some(weak.upgrade().unwrap().push(frame));
                }
            })
        };
        queues
            .push(Frame {
                bytes: [7; 6],
                inspect: Some(inspect),
            })
            .ok()
            .unwrap();
        armed.set(true);
        assert_eq!(queues.head_for([7; 6]).unwrap().unwrap().pending_frames, 1);
        let (error, frame) = returned.borrow_mut().take().unwrap().err().unwrap();
        assert_eq!(error, MemoryQueueError::QueueBorrowed);
        assert_eq!(frame.bytes, [9; 6]);
        assert_eq!(queues.len(), Ok(1));
        queues.push(frame).ok().unwrap();
        assert_eq!(queues.len(), Ok(2));
        assert_eq!(queues.try_take_for([9; 6]).unwrap().unwrap().bytes, [9; 6]);
    }
}
