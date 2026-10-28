//! Cross-core synchronization for destination/transport queues of owners.

use core::{
    cell::Cell,
    task::{Context, Poll, Waker},
};

use embassy_sync::blocking_mutex::{Mutex, raw::RawMutex};
use oer_ieee80211_datapath::{DestinationTxHead, TxQueues};
use oer_network::TransportFlow;

use super::{
    QueuedPacket,
    tx_budget::{TxBudget, TxCredit},
    wake_replaced,
};

struct State<const N: usize> {
    queues: TxQueues<[u8; 6], QueuedPacket, N, TransportFlow>,
    receiver_waker: Option<Waker>,
    receiver_wait: Option<ReceiverWait>,
    radio_cursor: usize,
}

impl<const N: usize> State<N> {
    const fn new() -> Self {
        Self {
            queues: TxQueues::new(),
            receiver_waker: None,
            receiver_wait: None,
            radio_cursor: 0,
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

pub(super) struct TxQueue<M: RawMutex, const N: usize> {
    state: Mutex<M, Cell<State<N>>>,
    budget: TxBudget<M>,
}

impl<M: RawMutex, const N: usize> TxQueue<M, N> {
    pub(super) const fn new() -> Self {
        Self {
            budget: TxBudget::new(N),
            state: Mutex::new(Cell::new(State::new())),
        }
    }

    // The queue-owning State<N> is a stack temporary for this synchronous
    // closure. No owner is dropped and no Waker callback runs until restored.
    fn with_state<R>(&self, action: impl FnOnce(&mut State<N>) -> R) -> R {
        self.state.lock(|cell| {
            let mut state = cell.replace(State::new());
            let result = action(&mut state);
            cell.set(state);
            result
        })
    }

    pub(super) fn register_sender(&self, waker: &Waker) {
        self.budget.register_sender(waker);
    }

    pub(super) fn len(&self) -> usize {
        self.with_state(|state| state.queues.len())
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
        let (result, wake) = self.with_state(|state| {
            if let Err(packet) = state.queues.push_flow(destination, flow, packet) {
                return (Err(packet), None);
            }
            let mut wake = None;
            if state.receiver_wait.is_some_and(|wait| match wait {
                ReceiverWait::Destination(selected, minimum) => {
                    destination == selected && state.queues.len_for(selected) >= minimum
                }
                ReceiverWait::Any => true,
            }) {
                state.receiver_wait = None;
                wake = state.receiver_waker.take();
            }
            (Ok(()), wake)
        });
        if let Some(wake) = wake {
            wake.wake();
        }
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
        self.poll_ready(
            ReceiverWait::Destination(destination, minimum.max(1)),
            context,
        )
    }

    pub(super) fn poll_ready_any(&self, context: &mut Context<'_>) -> Poll<()> {
        self.poll_ready(ReceiverWait::Any, context)
    }

    fn poll_ready(&self, wait: ReceiverWait, context: &mut Context<'_>) -> Poll<()> {
        // Clone before taking state; even a custom clone callback may reenter.
        let mut next = Some(context.waker().clone());
        let (ready, previous) = self.with_state(|state| {
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
        wake_replaced(previous, context.waker());
        // An unused clone (already ready) also drops outside the lock.
        drop(next);
        ready
    }

    pub(super) fn next_head_after(
        &self,
        after: Option<[u8; 6]>,
    ) -> Option<([u8; 6], DestinationTxHead)> {
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

    pub(super) fn head_for(&self, destination: [u8; 6]) -> Option<DestinationTxHead> {
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
    ) -> Option<(QueuedPacket, TxCredit<'_, M>)> {
        let packet = self.with_state(|state| {
            let destination =
                destination.or_else(|| state.queues.next_key(&mut state.radio_cursor))?;
            let packet = state.queues.pop(destination)?;
            Some(packet)
        })?;
        // Dequeue transfers an existing credit; it does not free admission.
        Some((packet, self.budget.claim()))
    }
}
