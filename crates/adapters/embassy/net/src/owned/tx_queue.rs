//! Cross-core synchronization for destination/transport queues of owners.

use core::{
    cell::RefCell,
    task::{Context, Poll, Waker},
};

use embassy_sync::{
    blocking_mutex::{Mutex, raw::RawMutex},
    waitqueue::WakerRegistration,
};
use oer_ieee80211_datapath::{DestinationTxHead, TxQueues};
use oer_network_interface::TransportFlow;

use super::{
    QueuedPacket,
    tx_budget::{TxBudget, TxCredit},
};

struct State<const N: usize> {
    queues: TxQueues<[u8; 6], QueuedPacket, N, TransportFlow>,
    receiver_waker: WakerRegistration,
    receiver_wait: Option<ReceiverWait>,
    radio_cursor: usize,
}

/// What the radio waits for.
#[derive(Clone, Copy)]
enum ReceiverWait {
    /// At least `minimum` frames for one destination.
    Destination([u8; 6], usize),
    /// A frame for any destination.
    Any,
}

pub(super) struct TxQueue<M: RawMutex, const N: usize> {
    state: Mutex<M, RefCell<State<N>>>,
    budget: TxBudget<M>,
}

impl<M: RawMutex, const N: usize> TxQueue<M, N> {
    pub(super) const fn new() -> Self {
        Self {
            budget: TxBudget::new(N),
            state: Mutex::new(RefCell::new(State {
                queues: TxQueues::new(),
                receiver_waker: WakerRegistration::new(),
                receiver_wait: None,
                radio_cursor: 0,
            })),
        }
    }

    pub(super) fn register_sender(&self, waker: &Waker) {
        self.budget.register_sender(waker);
    }

    pub(super) fn len(&self) -> usize {
        self.state.lock(|state| state.borrow().queues.len())
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub(super) fn is_full(&self) -> bool {
        self.budget.is_full()
    }

    pub(super) fn push(&self, packet: QueuedPacket) -> Result<(), QueuedPacket> {
        // Classify once on the producer core, outside the shared metadata
        // lock. Invalid frames retain ordinary radio rejection.
        let destination = packet
            .packet
            .get(..6)
            .and_then(|bytes| bytes.try_into().ok())
            .unwrap_or([0; 6]);
        let flow = TransportFlow::from_ethernet(&packet.packet);
        if !self.budget.try_admit() {
            return Err(packet);
        }
        let result = self.state.lock(|state| {
            let mut state = state.borrow_mut();
            state.queues.push_flow(destination, flow, packet)?;
            if state.receiver_wait.is_some_and(|wait| match wait {
                ReceiverWait::Destination(selected, minimum) => {
                    destination == selected && state.queues.len_for(selected) >= minimum
                }
                ReceiverWait::Any => true,
            }) {
                state.receiver_wait = None;
                state.receiver_waker.wake();
            }
            Ok(())
        });
        if result.is_err() {
            // Queue insertion normally cannot fail: queued owners are a subset
            // of the shared budget. Keep failure ownership explicit nonetheless.
            drop(self.budget.claim());
        }
        result
    }

    pub(super) fn poll_ready_for(
        &self,
        destination: [u8; 6],
        minimum: usize,
        context: &mut Context<'_>,
    ) -> Poll<()> {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if state.queues.len_for(destination) >= minimum.max(1) {
                state.receiver_wait = None;
                Poll::Ready(())
            } else {
                state.receiver_wait = Some(ReceiverWait::Destination(destination, minimum.max(1)));
                state.receiver_waker.register(context.waker());
                Poll::Pending
            }
        })
    }

    pub(super) fn poll_ready_any(&self, context: &mut Context<'_>) -> Poll<()> {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if state.queues.is_empty() {
                state.receiver_wait = Some(ReceiverWait::Any);
                state.receiver_waker.register(context.waker());
                Poll::Pending
            } else {
                state.receiver_wait = None;
                Poll::Ready(())
            }
        })
    }

    pub(super) fn next_head_after(
        &self,
        after: Option<[u8; 6]>,
    ) -> Option<([u8; 6], DestinationTxHead)> {
        self.state.lock(|state| {
            let state = state.borrow();
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

    pub(super) fn head_for(&self, destination: [u8; 6]) -> Option<DestinationTxHead> {
        self.state.lock(|state| {
            let state = state.borrow();
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
    ) -> Option<(QueuedPacket, TxCredit<'_, M>)> {
        let packet = self.state.lock(|state| {
            let mut state = state.borrow_mut();
            let state = &mut *state;
            let destination =
                destination.or_else(|| state.queues.next_key(&mut state.radio_cursor))?;
            let packet = state.queues.pop(destination)?;
            Some(packet)
        })?;
        // Dequeue transfers an existing credit; it does not free admission.
        Some((packet, self.budget.claim()))
    }
}
