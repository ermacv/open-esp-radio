//! Staged leases reach the application (P1) through an SPSC queue.
//!
//! [`QueueSink`] is the receive runner's publisher on the radio executor
//! (P2): it marks each staged lease's first handoff (the monotonic time and
//! the frame's raw MAC stamp), counts it, sends it into the queue and wakes
//! the application. [`FrameReceiver`] is the application's end: it takes
//! the leases in order and holds each through the [`RxReturnNotifier`], so
//! dropping a frame wakes the runner to rearm its buffer.
//!
//! The queue is as deep as the staging pool and every queued or held frame
//! keeps its pool slot, so whenever the pool has a credit the queue has a
//! vacant cell: a send after a credit never refuses. Producer and consumer
//! share only the queue's cursors and an [`AtomicWaker`]; no lock.
//!
//! **Teardown.** The runner waits, bounded, until no lease is staged before
//! it stops the walker; queued frames are staged. The application keeps
//! receiving and dropping frames after it requests the stop, until the
//! exit is published; a receiver that stops early leaves the epoch
//! quarantined with [`DrainTimeout`](super::control::WifiRxQuarantine::DrainTimeout).

use core::{future::poll_fn, task::Poll};
use oer_interrupt_table::signal::SignalExecutor;

use atomic_waker::AtomicWaker;
use oer_esp32s31_ieee80211_mac::rx::{
    NormalizedRxFrame, RxError, RxIngressConfig, decode_rx_local_timestamp, first_segment_layout,
    pool::{NetworkRxFrame, RxTimes},
};
use oer_memory::{
    AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, AffineSpscTryReceiveError,
    AffineSpscTrySendError,
};
use oer_time::Clock;

use super::{
    counters::WifiRxCounters,
    returns::{HeldRxFrame, RxReturnNotifier},
    sink::{frame_control_preview, unclassified_preview},
    transaction::{CompletedUnit, Preview, Publisher},
};

/// The W1 ingress configuration: single-descriptor units without CSI, and
/// neither strict RXEND nor strict dump-length refusal; what those checks
/// would refuse is counted instead ([`FrameReceiver::normalize`]).
pub const W1_INGRESS: RxIngressConfig = RxIngressConfig {
    ring_entry_limit: 1,
    csi_config: 0,
    flags: 0,
};

/// The queue between the runner and the application, and the waker of the
/// application's receive.
pub(crate) struct RxFrameQueue<'a, const SLOTS: usize, const BUFFER: usize> {
    frames: AffineSpscQueue<NetworkRxFrame<'a, SLOTS, BUFFER>, SLOTS>,
    waker: AtomicWaker,
}

impl<'a, const SLOTS: usize, const BUFFER: usize> RxFrameQueue<'a, SLOTS, BUFFER> {
    pub(crate) fn init_in_place(storage: &mut core::mem::MaybeUninit<Self>) -> &mut Self {
        oer_memory::init_in_place!(storage, Self {
            frames => AffineSpscQueue::init_in_place,
            waker: AtomicWaker::new(),
        })
    }

    /// Lend the runner's end and the application's end.
    pub(crate) fn split<X: SignalExecutor>(
        &'a mut self,
        counters: &'a WifiRxCounters,
        returns: RxReturnNotifier<'a, X>,
    ) -> RxFrameEnds<'a, SLOTS, BUFFER, X> {
        let Self { frames, waker } = self;
        let (sender, receiver) = frames.split();
        let waker = &*waker;
        RxFrameEnds {
            sender: FrameSender {
                frames: sender,
                waker,
                counters,
            },
            receiver: FrameReceiver {
                frames: receiver,
                waker,
                returns,
                counters,
            },
        }
    }
}

/// The two ends of the staged-frame queue a bound arena lends.
pub struct RxFrameEnds<'a, const SLOTS: usize, const BUFFER: usize, X: SignalExecutor> {
    /// The runner's end; [`QueueSink::new`] makes it a publisher.
    pub sender: FrameSender<'a, SLOTS, BUFFER>,
    /// The application's end.
    pub receiver: FrameReceiver<'a, SLOTS, BUFFER, X>,
}

/// The runner's end of the queue.
pub struct FrameSender<'a, const SLOTS: usize, const BUFFER: usize> {
    frames: AffineSpscSender<'a, NetworkRxFrame<'a, SLOTS, BUFFER>, SLOTS>,
    waker: &'a AtomicWaker,
    counters: &'a WifiRxCounters,
}

/// The runner's publisher: marks, counts and queues each staged lease.
pub struct QueueSink<'a, C, const SLOTS: usize, const BUFFER: usize> {
    sender: FrameSender<'a, SLOTS, BUFFER>,
    clock: &'a C,
}

impl<'a, C: Clock, const SLOTS: usize, const BUFFER: usize> QueueSink<'a, C, SLOTS, BUFFER> {
    /// A publisher over `sender` that stamps each handoff with `clock`.
    pub const fn new(sender: FrameSender<'a, SLOTS, BUFFER>, clock: &'a C) -> Self {
        Self { sender, clock }
    }
}

impl<'a, C: Clock, const SLOTS: usize, const BUFFER: usize> Publisher<'a, BUFFER, SLOTS>
    for QueueSink<'a, C, SLOTS, BUFFER>
{
    /// As deep as the staging pool: a credit always has a vacant cell.
    const DEPTH: usize = SLOTS;

    fn free_capacity(&self) -> usize {
        self.sender.frames.free_capacity()
    }

    fn preview(&self, unit: CompletedUnit, bytes: [u8; 24]) -> Preview {
        frame_control_preview(unit, bytes)
    }

    fn unclassified_preview(&self, unit: CompletedUnit) -> Preview {
        unclassified_preview(unit)
    }

    fn try_send(
        &self,
        mut frame: NetworkRxFrame<'a, SLOTS, BUFFER>,
    ) -> Result<(), NetworkRxFrame<'a, SLOTS, BUFFER>> {
        let stamp = decode_rx_local_timestamp(frame.segment().buffer);
        frame.mark_handoff(RxTimes {
            handoff: self.clock.now(),
            stamp,
        });
        let length = frame.length();
        self.sender
            .frames
            .try_send(frame)
            .map_err(|AffineSpscTrySendError(frame)| frame)?;
        self.sender.counters.received(length);
        self.sender.waker.wake();
        Ok(())
    }
}

/// The application's end of the queue.
pub struct FrameReceiver<'a, const SLOTS: usize, const BUFFER: usize, X: SignalExecutor> {
    frames: AffineSpscReceiver<'a, NetworkRxFrame<'a, SLOTS, BUFFER>, SLOTS>,
    waker: &'a AtomicWaker,
    returns: RxReturnNotifier<'a, X>,
    counters: &'a WifiRxCounters,
}

impl<'a, const SLOTS: usize, const BUFFER: usize, X: SignalExecutor>
    FrameReceiver<'a, SLOTS, BUFFER, X>
{
    /// Take an already staged frame without waiting, retaining the same
    /// affine lease as `next`. A finite dwell drains this queue before stop.
    pub fn try_next(&mut self) -> Option<HeldRxFrame<'a, 'a, SLOTS, BUFFER, X>> {
        self.frames
            .try_receive()
            .ok()
            .map(|frame| self.returns.hold(frame))
    }

    /// The next staged frame, in reception order. Dropping it returns the
    /// lease and wakes the runner.
    pub async fn next(&mut self) -> HeldRxFrame<'a, 'a, SLOTS, BUFFER, X> {
        poll_fn(|cx| {
            // Registration precedes the receive, so a send is never missed.
            self.waker.register(cx.waker());
            match self.frames.try_receive() {
                Ok(frame) => Poll::Ready(self.returns.hold(frame)),
                Err(AffineSpscTryReceiveError::Empty) => Poll::Pending,
            }
        })
        .await
    }

    /// `frame` at the normalized boundary under [`W1_INGRESS`]. A refusal
    /// is counted by its reason; an admitted frame the strict RXEND or
    /// dump-length check would have refused is counted as such.
    pub fn normalize<'f>(
        &self,
        frame: &'f NetworkRxFrame<'a, SLOTS, BUFFER>,
    ) -> Result<NormalizedRxFrame<'f>, RxError> {
        let decoded = frame.normalized(W1_INGRESS).and_then(|normalized| {
            first_segment_layout(&frame.segment(), W1_INGRESS).map(|layout| (normalized, layout))
        });
        match decoded {
            Ok((normalized, layout)) => {
                self.counters
                    .strict_refusable(layout.rxend_state != 0, !layout.dump_length_matches);
                Ok(normalized)
            }
            Err(error) => {
                self.counters.prefix_refused(error);
                Err(error)
            }
        }
    }
}

#[cfg(test)]
mod tests;
