//! The network's transmit memory on its way home and the network's stop, as
//! a role's runner holds them: the station's datapath actor and the access
//! point's runner, each over the station's network end ([`NetworkEnds`]).
//!
//! A memory whose frame ended waits in the runner's freed queue until the
//! return queue has room; the freed queue holds every memory the runner can
//! hold (`HELD`, the runner's own bound), so none waits outside it. A stop
//! owes the network its stop: first every memory the runner holds goes back
//! (ended frames, frames left in the runner's queues and in the transport
//! queue, each as data left behind), then the network is asked to stop, and
//! its report settles it. A network lost before it reported proves no stop:
//! its memory still goes back through the return queue, and the stop settles
//! once nothing of the network is left with the runner.

use core::task::{Context, Poll};

use crate::datapath::{NetworkEnds, ReturnBurst};
use crate::station::{NetworkStop, Notifies, Outgoing, TxEnded};

/// Who hears of each return burst: the runner's port, when it observes
/// them (`OBSERVES`), or nobody.
pub(crate) trait ReturnObserver {
    const OBSERVES: bool;
    fn returned(burst: ReturnBurst);
}

/// No observer: a runner whose port reads no burst.
pub(crate) enum Unobserved {}

impl ReturnObserver for Unobserved {
    const OBSERVES: bool = false;
    fn returned(_: ReturnBurst) {}
}

/// How far the network side's stop came.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum NetworkStopping {
    /// No stop asked it to stop.
    NotAsked,
    /// A stop owes it its stop once the runner holds none of the network's
    /// transmit memory: the stopped radio's ended frames and the MSDUs left
    /// in the runner's and the transport queues went back through
    /// `returned`.
    Owed,
    /// Asked, and it has not reported.
    Asked,
    /// It reported its stop. Units it holds stay lent: they return to the
    /// lend count, which outlives the installation, whenever they drop.
    Stopped,
    /// Its participant was lost: its stop will not be confirmed. The runner
    /// still returns what it holds of the network through the return queue;
    /// the stop settles once nothing of the network is left with the
    /// runner. Units the network holds lent do not hold it.
    Lost,
}

/// Keep `memory`, whose MSDU ended, for the network; the bound holds every
/// memory the runner can hold, so a refusal keeps it quarantined rather than
/// dropping it.
pub(crate) fn free<M, const HELD: usize>(freed: &mut heapless::Deque<TxEnded<M>, HELD>, memory: M) {
    if let Err(ended) = freed.push_back(TxEnded { memory }) {
        let _quarantined = core::mem::ManuallyDrop::new(ended);
    }
}

/// The network's memory a runner holds on its way home, and the network's
/// stop: `HELD` is every memory the runner can hold at once.
pub(crate) struct NetworkReturn<M, const HELD: usize> {
    /// The memory of ended frames, until the return queue takes it.
    pub(crate) freed: heapless::Deque<TxEnded<M>, HELD>,
    stopping: NetworkStopping,
    /// The network's MSDUs a stop found left in the transport queue, at its
    /// last count: what a stop still holds of the network's memory beside
    /// the ended frames'.
    queued: usize,
}

impl<M, const HELD: usize> NetworkReturn<M, HELD> {
    pub(crate) const fn new() -> Self {
        Self {
            freed: heapless::Deque::new(),
            stopping: NetworkStopping::NotAsked,
            queued: 0,
        }
    }

    /// Keep `memory`, whose frame ended, for the network.
    pub(crate) fn free(&mut self, memory: M) {
        free(&mut self.freed, memory);
    }

    pub(crate) const fn stopping(&self) -> NetworkStopping {
        self.stopping
    }

    /// Whether the network's participant was lost before it reported.
    pub(crate) const fn lost(&self) -> bool {
        matches!(self.stopping, NetworkStopping::Lost)
    }

    /// Whether the network was asked and has not reported.
    pub(crate) const fn asked(&self) -> bool {
        matches!(self.stopping, NetworkStopping::Asked)
    }

    /// The network's memory the runner holds on its way home: ended frames'
    /// and MSDUs a stop found in the transport queue.
    pub(crate) fn held(&self) -> usize {
        self.freed.len().saturating_add(self.queued)
    }

    /// A new run: no stop is owed. Freed memory stays on its way home.
    pub(crate) fn reset(&mut self) {
        self.stopping = NetworkStopping::NotAsked;
        self.queued = 0;
    }

    /// A stop owes the network its stop, unless one already did: whether it
    /// does now.
    pub(crate) fn owe_stop(&mut self) -> bool {
        if self.stopping == NetworkStopping::NotAsked {
            self.stopping = NetworkStopping::Owed;
        }
        self.stopping != NetworkStopping::NotAsked
    }

    /// Whether a stop owes the transport queue's MSDUs their way home and
    /// the queue holds some.
    pub(crate) fn owes_queued<L, S: NetworkStop, const TX: usize, const RX: usize, N: Notifies>(
        &self,
        network: &NetworkEnds<'_, M, L, S, TX, RX, N>,
    ) -> bool {
        matches!(self.stopping, NetworkStopping::Owed | NetworkStopping::Lost)
            && !network.transmit.is_empty()
    }

    /// Give the network the memory of ended frames while its queue has
    /// room, as one burst: the burst is published at once at its end, which
    /// tells a waiting network once, so its turn finds all of them. `O`
    /// hears of the burst when it observes them.
    pub(crate) fn return_freed<
        O: ReturnObserver,
        L,
        S: NetworkStop,
        const TX: usize,
        const RX: usize,
        N: Notifies,
    >(
        &mut self,
        network: &mut NetworkEnds<'_, M, L, S, TX, RX, N>,
    ) {
        let mut burst = false;
        // Only an observing runner reads the queue for its burst.
        let before = if O::OBSERVES {
            TX.saturating_sub(network.returned.free_capacity())
        } else {
            0
        };
        let mut queued = 0_usize;
        while network.returned.free_capacity() != 0 {
            let Some(memory) = self.freed.pop_front() else {
                break;
            };
            let mut memory = Some(memory);
            let _sent = network.returned.try_stage_with(|slot| {
                *slot = memory.take();
                Ok::<_, core::convert::Infallible>(())
            });
            burst = true;
            if O::OBSERVES {
                queued = queued.saturating_add(1);
            }
            if let Some(memory) = memory {
                // The room was checked: the queue only keeps it when broken.
                let _quarantined = core::mem::ManuallyDrop::new(memory);
            }
        }
        if burst {
            network.returned.flush();
            if O::OBSERVES {
                O::returned(ReturnBurst {
                    queued,
                    before,
                    after: TX.saturating_sub(network.returned.free_capacity()),
                    held: self.freed.len(),
                });
            }
        }
    }

    /// While a stop owes the network its stop: hand back the MSDUs the
    /// runner's own queues hold (`queued`) and those the network left in the
    /// transport queue, each as data left behind (`left_behind`), one at a
    /// time through the ended frames' way home while that way is clear; and
    /// once the runner holds none of the network's memory and `may_ask`
    /// (its radio stopped, nothing of its own still owes an end), ask the
    /// network to stop.
    pub(crate) fn return_left<
        O: ReturnObserver,
        L,
        S: NetworkStop,
        const TX: usize,
        const RX: usize,
        N: Notifies,
    >(
        &mut self,
        network: &mut NetworkEnds<'_, M, L, S, TX, RX, N>,
        mut queued: impl FnMut() -> Option<M>,
        mut left_behind: impl FnMut(),
        may_ask: bool,
    ) {
        if !matches!(self.stopping, NetworkStopping::Owed | NetworkStopping::Lost) {
            return;
        }
        while self.freed.is_empty() && network.returned.free_capacity() != 0 {
            // What the runner's queues took first, then what the transport
            // queue still holds.
            let memory = match queued() {
                Some(memory) => memory,
                None => match network.transmit.try_receive_with(Option::take) {
                    Ok(Some(Outgoing { memory, .. })) => memory,
                    _ => break,
                },
            };
            left_behind();
            // The deque is empty, so it holds this one memory. The radio
            // never took it: it never went.
            self.free(memory);
            self.return_freed::<O, L, S, TX, RX, N>(network);
        }
        self.queued = network.transmit.len();
        if self.stopping == NetworkStopping::Owed
            && may_ask
            && self.freed.is_empty()
            && self.queued == 0
        {
            network.network.request_stop();
            self.stopping = NetworkStopping::Asked;
        }
    }

    /// Follow the network side's stop and its report; whether it moved.
    pub(crate) fn poll_stop<L, S: NetworkStop, const TX: usize, const RX: usize, N: Notifies>(
        &mut self,
        cx: &mut Context<'_>,
        network: &NetworkEnds<'_, M, L, S, TX, RX, N>,
    ) -> bool {
        let before = self.stopping;
        match self.stopping {
            NetworkStopping::Asked => {
                self.stopping = match network.network.poll_stopped(cx) {
                    Poll::Ready(Ok(())) => NetworkStopping::Stopped,
                    Poll::Ready(Err(_)) => NetworkStopping::Lost,
                    Poll::Pending => NetworkStopping::Asked,
                };
            }
            // A participant lost before the stop asked it: its memory still
            // goes home through the return queue, and no stop is asked.
            NetworkStopping::Owed => {
                if let Poll::Ready(Err(_)) = network.network.poll_stopped(cx) {
                    self.stopping = NetworkStopping::Lost;
                }
            }
            _ => {}
        }
        self.stopping != before
    }

    /// Whether the runner holds nothing of the network's any more: every
    /// ended frame's memory went to the return queue, no MSDU waits in the
    /// transport queue, and `runner_home` (nothing in the runner's own
    /// queues, nothing of its own owes an end).
    pub(crate) fn home(&self, runner_home: bool) -> bool {
        runner_home && self.freed.is_empty() && self.queued == 0
    }

    /// Whether the network side owes the stop nothing: no stop asked it to
    /// stop, or it stopped, or it was lost and nothing of it is left with
    /// the runner ([`Self::home`]).
    pub(crate) fn settled(&self, runner_home: bool) -> bool {
        match self.stopping {
            NetworkStopping::NotAsked | NetworkStopping::Stopped => true,
            NetworkStopping::Lost => self.home(runner_home),
            NetworkStopping::Owed | NetworkStopping::Asked => false,
        }
    }
}
