//! Destination/transport queues of owners in the endpoint's execution context.

use core::task::{Context, Poll, Waker};

use crate::datapath::{DestinationTxHead, TxQueues};
use oer_memory::{ContextCell, Entered, ExecutionContext};
use oer_network_port::TransportFlow;

use super::{
    AccessRefused, QueuedPacket,
    tx_budget::{TxBudget, TxCredit},
    wake_replaced,
};

struct State<const N: usize> {
    queues: TxQueues<[u8; 6], QueuedPacket, N, TransportFlow>,
    receiver_waker: Option<Waker>,
    receiver_wait: Option<ReceiverWait>,
    radio_cursor: usize,
    publication: Publication,
}

impl<const N: usize> State<N> {
    const fn new() -> Self {
        Self {
            queues: TxQueues::new(),
            receiver_waker: None,
            receiver_wait: None,
            radio_cursor: 0,
            publication: Publication::Idle,
        }
    }
}

/// What the radio waits for.
#[derive(Clone, Copy)]
enum ReceiverWait {
    /// At least `minimum` frames for one destination.
    Destination([u8; 6], usize),
    /// A frame for any destination.
    Any,
}

/// Coalesced "a packet was published" hint for the radio's publication wait.
enum Publication {
    Idle,
    Waiting(Waker),
    Signaled,
}

pub(super) struct TxQueue<X: ExecutionContext, const N: usize> {
    state: ContextCell<State<N>, Entered<X>>,
    budget: TxBudget,
}

impl<X: ExecutionContext, const N: usize> TxQueue<X, N> {
    pub(super) const fn new() -> Self {
        Self {
            budget: TxBudget::new(N),
            state: ContextCell::new(State::new()),
        }
    }

    // The closure borrows the state in place. No owner is dropped and no
    // Waker callback runs inside it, so a callback may reenter afterwards.
    // Outside `X`, or nested inside this borrow, the state stays unchanged.
    fn with_state<R>(&self, action: impl FnOnce(&mut State<N>) -> R) -> Result<R, AccessRefused> {
        let proof = Entered::<X>::enter().ok_or(AccessRefused)?;
        self.state.with(&proof, action).ok_or(AccessRefused)
    }

    pub(super) fn register_sender(&self, waker: &Waker) {
        self.budget.register_sender(waker);
    }

    pub(super) fn len(&self) -> Result<usize, AccessRefused> {
        self.with_state(|state| state.queues.len())
    }

    pub(super) fn is_empty(&self) -> Result<bool, AccessRefused> {
        Ok(self.len()? == 0)
    }

    pub(super) fn is_full(&self) -> bool {
        self.budget.is_full()
    }

    /// Queue one admitted owner and signal its publication. A full budget, a
    /// failed insertion or a refused access returns the unchanged owner.
    pub(super) fn push(&self, packet: QueuedPacket) -> Result<(), QueuedPacket> {
        // Classify once on the producer core, outside the queue state.
        // Invalid frames retain ordinary radio rejection.
        let destination = packet
            .packet
            .get(..6)
            .and_then(|bytes| bytes.try_into().ok())
            .unwrap_or([0; 6]);
        let flow = TransportFlow::from_ethernet(&packet.packet);
        if !self.budget.try_admit() {
            return Err(packet);
        }
        // A refused access never runs the closure, and a failed insertion puts
        // the owner back: an empty slot means exactly that the owner is queued.
        let mut slot = Some(packet);
        let wakes = self.with_state(|state| {
            let packet = slot.take()?;
            if let Err(packet) = state.queues.push_flow(destination, flow, packet) {
                slot = Some(packet);
                return None;
            }
            let mut receiver = None;
            if state.receiver_wait.is_some_and(|wait| match wait {
                ReceiverWait::Destination(selected, minimum) => {
                    destination == selected && state.queues.len_for(selected) >= minimum
                }
                ReceiverWait::Any => true,
            }) {
                state.receiver_wait = None;
                receiver = state.receiver_waker.take();
            }
            let publication =
                match core::mem::replace(&mut state.publication, Publication::Signaled) {
                    Publication::Waiting(waker) => Some(waker),
                    Publication::Idle | Publication::Signaled => None,
                };
            Some((receiver, publication))
        });
        match slot {
            Some(packet) => {
                // Queue insertion normally cannot fail: queued owners are a
                // subset of the shared budget. A refused access or a failed
                // insertion returns the owner and its admission explicitly.
                drop(self.budget.claim());
                Err(packet)
            }
            None => {
                // Wake the radio only after the state borrow has ended.
                if let Ok(Some((receiver, publication))) = wakes {
                    if let Some(wake) = receiver {
                        wake.wake();
                    }
                    if let Some(wake) = publication {
                        wake.wake();
                    }
                }
                Ok(())
            }
        }
    }

    pub(super) fn poll_ready_for(
        &self,
        destination: [u8; 6],
        minimum: usize,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), AccessRefused>> {
        self.poll_ready(
            ReceiverWait::Destination(destination, minimum.max(1)),
            context,
        )
    }

    pub(super) fn poll_ready_any(
        &self,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), AccessRefused>> {
        self.poll_ready(ReceiverWait::Any, context)
    }

    fn poll_ready(
        &self,
        wait: ReceiverWait,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), AccessRefused>> {
        // Clone before borrowing state; even a custom clone callback may reenter.
        let mut next = Some(context.waker().clone());
        let outcome = self.with_state(|state| {
            let ready = match wait {
                ReceiverWait::Destination(destination, minimum) => {
                    state.queues.len_for(destination) >= minimum
                }
                ReceiverWait::Any => !state.queues.is_empty(),
            };
            if ready {
                state.receiver_wait = None;
                (Poll::Ready(()), None)
            } else {
                state.receiver_wait = Some(wait);
                let previous = core::mem::replace(&mut state.receiver_waker, next.take());
                (Poll::Pending, previous)
            }
        });
        let ready = match outcome {
            Ok((ready, previous)) => {
                wake_replaced(previous, context.waker());
                ready.map(Ok)
            }
            Err(refused) => Poll::Ready(Err(refused)),
        };
        // An unused clone (ready or refused) also drops outside the borrow.
        drop(next);
        ready
    }

    /// Poll the coalesced publication hint, consuming it when ready.
    pub(super) fn poll_publication(
        &self,
        context: &mut Context<'_>,
    ) -> Poll<Result<(), AccessRefused>> {
        // Clone before borrowing state; even a custom clone callback may reenter.
        let mut next = Some(context.waker().clone());
        let outcome = self.with_state(|state| {
            match core::mem::replace(&mut state.publication, Publication::Idle) {
                Publication::Signaled => (Poll::Ready(()), None),
                Publication::Waiting(registered) if registered.will_wake(context.waker()) => {
                    state.publication = Publication::Waiting(registered);
                    (Poll::Pending, None)
                }
                Publication::Waiting(registered) => {
                    if let Some(waker) = next.take() {
                        state.publication = Publication::Waiting(waker);
                    }
                    (Poll::Pending, Some(registered))
                }
                Publication::Idle => {
                    if let Some(waker) = next.take() {
                        state.publication = Publication::Waiting(waker);
                    }
                    (Poll::Pending, None)
                }
            }
        });
        let ready = match outcome {
            Ok((ready, previous)) => {
                wake_replaced(previous, context.waker());
                ready.map(Ok)
            }
            Err(refused) => Poll::Ready(Err(refused)),
        };
        drop(next);
        ready
    }

    pub(super) fn next_head_after(
        &self,
        after: Option<[u8; 6]>,
    ) -> Result<Option<([u8; 6], DestinationTxHead)>, AccessRefused> {
        self.with_state(|state| {
            let (destination, head, pending_frames) = state.queues.next_head_after(after)?;
            Some((
                destination,
                DestinationTxHead {
                    ethernet_bytes: head.packet.len(),
                    pending_frames,
                },
            ))
        })
    }

    pub(super) fn head_for(
        &self,
        destination: [u8; 6],
    ) -> Result<Option<DestinationTxHead>, AccessRefused> {
        self.with_state(|state| {
            let (head, pending_frames) = state.queues.head(destination)?;
            Some(DestinationTxHead {
                ethernet_bytes: head.packet.len(),
                pending_frames,
            })
        })
    }

    pub(super) fn pop(
        &self,
        destination: Option<[u8; 6]>,
    ) -> Result<Option<(QueuedPacket, TxCredit<'_>)>, AccessRefused> {
        let packet = self.with_state(|state| {
            let destination =
                destination.or_else(|| state.queues.next_key(&mut state.radio_cursor))?;
            state.queues.pop(destination)
        })?;
        // Dequeue transfers an existing credit; it does not free admission.
        Ok(packet.map(|packet| (packet, self.budget.claim())))
    }
}

#[cfg(test)]
#[allow(clippy::disallowed_macros, clippy::disallowed_methods)]
mod tests {
    extern crate std;
    use self::std::boxed::Box;
    use super::*;
    use oer_memory::{HostContext, Never};
    use xarxa_driver::{PacketBufAllocator, PacketPool, PacketPoolStorage};

    fn allocator() -> PacketBufAllocator {
        let storage = Box::leak(Box::new(PacketPoolStorage::<2>::new()));
        Box::leak(Box::new(PacketPool::new(storage))).allocator()
    }

    #[test]
    fn nested_access_retains_queue_input_and_admission() {
        let pool = allocator();
        let queue = TxQueue::<HostContext, 2>::new();
        let packet = pool.try_alloc().unwrap();
        let address = packet.as_ptr();
        let returned = queue
            .with_state(|_| {
                assert_eq!(queue.len(), Err(AccessRefused));
                assert!(matches!(queue.pop(None), Err(AccessRefused)));
                queue.push(QueuedPacket { epoch: 7, packet }).err().unwrap()
            })
            .unwrap();
        assert_eq!(returned.packet.as_ptr(), address);
        assert_eq!(returned.epoch, 7);
        assert_eq!(queue.len(), Ok(0));
        // A refused insertion must have returned its admission too.
        assert!(queue.push(returned).is_ok());
        assert!(
            queue
                .push(QueuedPacket {
                    epoch: 7,
                    packet: pool.try_alloc().unwrap()
                })
                .is_ok()
        );
        assert!(queue.is_full());
    }

    #[test]
    fn unavailable_context_returns_owners_and_typed_refusals() {
        let pool = allocator();
        let queue = TxQueue::<Never, 1>::new();
        let packet = pool.try_alloc().unwrap();
        let address = packet.as_ptr();
        let returned = queue.push(QueuedPacket { epoch: 9, packet }).err().unwrap();
        assert_eq!(returned.packet.as_ptr(), address);
        assert!(!queue.is_full());
        assert_eq!(queue.len(), Err(AccessRefused));
        assert_eq!(queue.head_for([0; 6]), Err(AccessRefused));
        assert!(matches!(queue.pop(None), Err(AccessRefused)));
        let waker = core::task::Waker::noop();
        let mut cx = Context::from_waker(waker);
        assert_eq!(
            queue.poll_ready_any(&mut cx),
            Poll::Ready(Err(AccessRefused))
        );
        assert_eq!(
            queue.poll_publication(&mut cx),
            Poll::Ready(Err(AccessRefused))
        );
    }
}
