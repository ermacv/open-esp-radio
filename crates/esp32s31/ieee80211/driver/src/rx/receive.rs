//! Receive on the radio executor (P2) with doorbell unmask and a fixed
//! teardown.
//!
//! [`receive`] owns one receive epoch from bring-up to park:
//!
//! - **Bring-up.** Prepare the ring, open promiscuous receive, activate the
//!   interrupt epoch (routes still silent), install the interrupt line
//!   (which clears a stale request), enable the routes, start the walker
//!   after its settle, resume the MAC runtime.
//! - **Receive.** Wait for an interrupt fact or the stop request, then run
//!   [`service`] until the frontier drained and every buffer the sink
//!   released in the pass has had one more pass to be rearmed; only then ask
//!   the line to unmask RX through the doorbell. (A released prefix that
//!   ends at the walker's LAST descriptor stays released until the walker
//!   moves on, which comes with the next RX edge.) A pending probe, a
//!   pending recycled append or an exhausted budget keeps RX masked and runs
//!   again: up to [`IMMEDIATE_CONTINUATIONS`] immediate self-wakes, then one
//!   [`CONTINUATION_WAIT`] timer wait. A pass blocked on staging capacity
//!   stays masked and waits for the capacity fact, which the upper layer
//!   posts when it returns a lease ([`super::returns`]): the walker may be
//!   exhausted then, with no RX interrupt to come. Before that capacity
//!   drain the runner reads the walker registers and the ring's view into
//!   the counters' exhaustion snapshot (report-only).
//! - **Teardown**, on the stop request or a fault, in this order: mask both
//!   banks through the doorbell and wait for the masked fact (bounded);
//!   silence the routes; close promiscuous receive and request the MAC
//!   runtime stop; wait until no lease is staged, no buffer is detached and
//!   the MAC runtime is idle (bounded); stop the walker exactly once. A fault
//!   poisons the arena before the stop; a refused stop poisons it after, and
//!   is never repeated. The mask and drain waits are measured into the
//!   counters' teardown report.
//! - **Park.** Publish the [`WifiRxExit`] to the application and hold every
//!   owner forever: nothing is quiesced, released or reset.

use core::{
    convert::Infallible,
    future::{Future, pending, poll_fn},
    pin::pin,
    task::Poll,
};

use oer_esp32s31_ieee80211_dma::{rx_storage::RxDmaStorage, space::DmaSpace};
use oer_esp32s31_ieee80211_mac::{
    init::{
        MacRuntimeStopHardware, MacSnifferHardware, activate_promiscuous_receive,
        deactivate_promiscuous_receive,
    },
    irq::{
        MacInterrupt, MacPowerInterrupt,
        consumer::{IrqConsumer, PowerIrqConsumer},
        epoch::{ActiveEpoch, Moderated},
    },
    rx::{RxDma, RxRingLive, pool::RxStagePool},
};
use oer_memory::HandlerCell;
use oer_time::{Duration, Instant, Timer};

use super::{
    arena::WifiRxParts,
    control::{ReceiveRunnerControl, WifiRxCause, WifiRxEnd, WifiRxExit, WifiRxQuarantine},
    counters::{WifiRxCounters, WifiRxExhaustion},
    frontier::{ReceiveFrontier, RxFrontierPhase},
    transaction::{Admission, Counters, Publisher, service},
};
use crate::{
    datapath::DatapathRxProgress,
    irq::{RadioDoorbell, WifiIrqLine, WifiIrqRequest, WifiIrqRequests, WifiIrqRoutes},
};

/// Immediate self-wakes before a continuation waits on the timer.
/// Provisional: measured in PR 5.
pub const IMMEDIATE_CONTINUATIONS: u32 = 4;
/// The timer wait after the immediate self-wakes. Provisional: measured in
/// PR 5.
pub const CONTINUATION_WAIT: Duration = Duration::from_micros(20);
/// How long teardown waits for the line to confirm the mask. Provisional:
/// measured in PR 5.
pub const MASK_CONFIRM_TIMEOUT: Duration = Duration::from_millis(1);
/// How long teardown waits for staged leases and the MAC runtime to drain.
/// Provisional: measured in PR 5.
pub const DRAIN_TIMEOUT: Duration = Duration::from_millis(100);
/// The timer wait between two drain checks. Provisional: measured in PR 5.
pub const DRAIN_POLL: Duration = Duration::from_micros(100);

/// Run one receive epoch over `parts`; publish its exit and park.
///
/// `hardware` is the task's radio owner (`RadioRuntimeOwner` on the chip).
/// `activate` activates the moderated interrupt epoch (on the chip
/// `InactiveEpoch::activate_rx_moderated` with the image's mask); it runs
/// while the routes are silent. `cell` is the image's static cell its MAC,
/// WDEVPWR and doorbell handlers run the line through. `publisher` and
/// `admission` take the staged units ([`QueueSink`](super::upper::QueueSink)
/// and [`CountingAdmission`](super::sink::CountingAdmission) in W1).
///
/// The returned future never completes: after teardown it publishes the
/// exit through the stop handle and holds every owner.
#[allow(
    clippy::too_many_arguments,
    reason = "each argument is one owner the epoch holds until it parks"
)]
pub async fn receive<
    H,
    I,
    P,
    R,
    D,
    T,
    E,
    M,
    const COUNT: usize,
    const BUFFER: usize,
    const STORAGE: usize,
    const STAGE_SLOTS: usize,
    S,
>(
    hardware: &mut H,
    activate: impl FnOnce() -> ActiveEpoch<Moderated, I, P>,
    parts: WifiRxParts<
        'static,
        COUNT,
        BUFFER,
        STORAGE,
        STAGE_SLOTS,
        S,
        ActiveEpoch<Moderated, I, P>,
    >,
    cell: &'static HandlerCell<WifiIrqLine<ActiveEpoch<Moderated, I, P>>>,
    routes: &mut R,
    doorbell: &D,
    timer: &T,
    publisher: &E,
    admission: &M,
) -> Infallible
where
    H: RxDma + MacSnifferHardware + MacRuntimeStopHardware,
    I: MacInterrupt,
    P: MacPowerInterrupt,
    R: WifiIrqRoutes,
    D: RadioDoorbell,
    T: Timer,
    E: Publisher<'static, BUFFER, STAGE_SLOTS>,
    M: Admission,
    S: DmaSpace,
{
    let WifiRxParts {
        rx,
        descriptor_base,
        buffer_addresses,
        stage,
        irq,
        power,
        requests,
        counters,
        control,
        line,
        // The upper layer holds its copy of the return notifier.
        returns: _,
    } = parts;
    let mut receiver = Receiver {
        hardware,
        rx,
        stage,
        irq,
        power,
        requests,
        counters,
        control,
        routes,
        doorbell,
        timer,
        publisher,
        admission,
        totals: [0; 3],
        blocked_since: None,
    };

    let mut frontier = match ReceiveFrontier::<COUNT, BUFFER>::prepare_initial(
        receiver.hardware,
        rx,
        descriptor_base,
        buffer_addresses,
    ) {
        Ok(frontier) => frontier,
        Err(error) => {
            return receiver
                .park(WifiRxExit {
                    cause: WifiRxCause::Prepare(error),
                    end: WifiRxEnd::NotStarted,
                })
                .await;
        }
    };
    activate_promiscuous_receive(receiver.hardware);
    let _installed = match line.install(activate(), cell) {
        Ok(installed) => installed,
        Err(refused) => {
            // No handler can run this line: the cell holds another one and
            // the routes were never enabled, so the executor masks it.
            refused.epoch_mut().mask_all();
            deactivate_promiscuous_receive(receiver.hardware);
            return receiver
                .park(WifiRxExit {
                    cause: WifiRxCause::LineRefused,
                    end: WifiRxEnd::NotStarted,
                })
                .await;
        }
    };
    receiver.routes.enable();

    let cause = match frontier
        .start_with_storage(receiver.timer, receiver.hardware, rx)
        .await
    {
        Err(error) => WifiRxCause::Start(error),
        Ok(()) => match frontier.live_mut() {
            Err(error) => WifiRxCause::Start(error),
            Ok(ring) => {
                receiver.hardware.resume_mac_runtime();
                receiver.run(ring).await
            }
        },
    };
    let exit = receiver.teardown(&mut frontier, cause).await;
    receiver.park(exit).await
}

/// The owners of a running epoch, except its frontier.
struct Receiver<
    'r,
    H,
    R,
    D,
    T,
    E,
    M,
    const COUNT: usize,
    const BUFFER: usize,
    const STORAGE: usize,
    const STAGE_SLOTS: usize,
    S: DmaSpace,
> {
    hardware: &'r mut H,
    rx: &'static RxDmaStorage<COUNT, BUFFER, STORAGE, S>,
    stage: &'static RxStagePool<STAGE_SLOTS, BUFFER>,
    irq: IrqConsumer<'static>,
    power: PowerIrqConsumer<'static>,
    requests: &'static WifiIrqRequests,
    counters: &'static WifiRxCounters,
    control: ReceiveRunnerControl<'static>,
    routes: &'r mut R,
    doorbell: &'r D,
    timer: &'r T,
    publisher: &'r E,
    admission: &'r M,
    /// The service's cumulative descriptors, units and bytes.
    totals: [u64; 3],
    /// When the last drain ended blocked on staging capacity, until the
    /// next wake.
    blocked_since: Option<Instant>,
}

impl<
    H,
    R,
    D,
    T,
    E,
    M,
    const COUNT: usize,
    const BUFFER: usize,
    const STORAGE: usize,
    const STAGE_SLOTS: usize,
    S,
> Receiver<'_, H, R, D, T, E, M, COUNT, BUFFER, STORAGE, STAGE_SLOTS, S>
where
    H: RxDma + MacSnifferHardware + MacRuntimeStopHardware,
    R: WifiIrqRoutes,
    D: RadioDoorbell,
    T: Timer,
    E: Publisher<'static, BUFFER, STAGE_SLOTS>,
    M: Admission,
    S: DmaSpace,
{
    /// Receive until the stop request or a fault.
    async fn run(&mut self, ring: &mut RxRingLive<'static, COUNT>) -> WifiRxCause {
        loop {
            // `None` is the stop request; otherwise whether a lease came back.
            let wake = poll_fn(|cx| {
                if self.control.poll_stop(cx).is_ready() {
                    return Poll::Ready(None);
                }
                // WDEVPWR facts are counted by the line; drain them here.
                while self.power.poll(cx).is_ready() {}
                // Any fact (RX, capacity) starts one drain.
                self.irq
                    .poll_batch(cx)
                    .map(|facts| Some(facts.rx_capacity_returned()))
            })
            .await;
            let Some(capacity) = wake else {
                return WifiRxCause::StopRequested;
            };
            let blocked_since = self.blocked_since.take();
            if capacity {
                self.counters.capacity_drain();
                if let Some(since) = blocked_since {
                    self.observe_exhaustion(ring, since);
                }
            }
            if let Err(cause) = self.drain(ring).await {
                return cause;
            }
        }
    }

    /// Service until the frontier drained or staging blocked, keeping RX
    /// masked meanwhile; unmask through the doorbell only once drained.
    async fn drain(&mut self, ring: &mut RxRingLive<'static, COUNT>) -> Result<(), WifiRxCause> {
        // Self-wakes since the last timer wait; continuations of this drain.
        let (mut streak, mut continuations) = (0, 0);
        loop {
            if self.control.stop_requested() {
                return Err(WifiRxCause::StopRequested);
            }
            let admitted_before = self.totals[1];
            let progress = self.service_once(ring)?;
            // Units this pass staged were released into the sink after the
            // pass's own rearm step: run once more to rearm them. A released
            // buffer that is still the walker's LAST descriptor cannot be
            // rearmed before the walker moves on, and that move comes with
            // the next RX edge; a pass that admitted nothing leaves it there.
            let rearm_pending =
                self.rx.released_buffer_count() != 0 && self.totals[1] != admitted_before;
            match progress {
                DatapathRxProgress::Drained | DatapathRxProgress::UpperLayerBlockedButDroppable
                    if !rearm_pending =>
                {
                    self.requests.ring(WifiIrqRequest::UnmaskRx, self.doorbell);
                    return Ok(());
                }
                DatapathRxProgress::StageCapacityBlocked => {
                    self.counters.stage_capacity_block();
                    self.blocked_since = Some(self.timer.now());
                    return Ok(());
                }
                // A released buffer is rearmed before any unmask, so a burst
                // never leaves the walker short. The service returns no
                // network or TX backpressure; were it to, the pass reruns.
                DatapathRxProgress::Drained
                | DatapathRxProgress::UpperLayerBlockedButDroppable
                | DatapathRxProgress::ProbePending
                | DatapathRxProgress::RecycledAppendPending
                | DatapathRxProgress::BudgetExhausted
                | DatapathRxProgress::NetworkBackpressured
                | DatapathRxProgress::ProtocolBlockedByTx => {
                    continuations += 1;
                    self.counters.continuation(progress, continuations);
                    self.continue_after(&mut streak).await;
                }
            }
        }
    }

    /// Read the walker registers and the ring's view after a stage-capacity
    /// block that began at `since`, before the capacity drain services
    /// anything. Only reads: no register is written and no descriptor
    /// changes hands.
    fn observe_exhaustion(&mut self, ring: &RxRingLive<'static, COUNT>, since: Instant) {
        let blocked = self.timer.now().saturating_duration_since(since);
        let next = self.hardware.next_descriptor();
        let last_low = self.hardware.last_descriptor_low();
        let base = self.hardware.descriptor_base_register();
        let walker_enabled = self.hardware.walker_enabled();
        let reload_doorbell = RxDma::reload_pending(self.hardware);
        let buffer_full = self.hardware.buffer_full_count();
        let narrow = |value: usize| u32::try_from(value).unwrap_or(u32::MAX);
        self.counters.exhaustion(WifiRxExhaustion {
            blocked_us: u32::try_from(blocked.as_micros()).unwrap_or(u32::MAX),
            next_low: next.address_low(),
            next_upper: next.has_unknown_upper_state(),
            last_low,
            base,
            walker_enabled,
            reload_doorbell,
            buffer_full,
            ring_base: ring.descriptor_base(),
            recycle_start: narrow(ring.recycle_start().get()),
            accepted_tail: narrow(ring.accepted_tail().get()),
            observed: ring.observed_mask().count_ones(),
            completed: narrow(ring.completed_frontier_len()),
            remaining: ring.accepted_list_remaining_from_next(next).map(narrow),
            reload_pending: ring.reload_pending(),
            released: narrow(self.rx.released_buffer_count()),
            detached: narrow(self.rx.detached_buffer_count()),
            staged: self.stage.claimed_slots(),
        });
    }

    fn service_once(
        &mut self,
        ring: &mut RxRingLive<'static, COUNT>,
    ) -> Result<DatapathRxProgress, WifiRxCause> {
        self.counters.service_pass();
        let [descriptors, units, bytes] = &mut self.totals;
        let result = service(
            ring,
            self.rx,
            self.stage,
            self.publisher,
            self.admission,
            Counters {
                descriptors,
                units,
                bytes,
            },
            self.hardware,
            (),
        );
        self.counters.reload_evidence(ring.reload_repair_evidence());
        result.map_err(|error| {
            self.counters.service_error();
            WifiRxCause::Service(error)
        })
    }

    async fn continue_after(&mut self, streak: &mut u32) {
        if *streak < IMMEDIATE_CONTINUATIONS {
            *streak += 1;
            self.counters.immediate_continuation();
            yield_now().await;
        } else {
            *streak = 0;
            self.counters.timed_continuation();
            match self.timer.deadline_after(CONTINUATION_WAIT) {
                Ok(deadline) => self.timer.wait_until(deadline).await,
                // As the walker settle: a wait past the timer's range never
                // ends, so the pass is never rerun early.
                Err(oer_time::TimeOverflow) => pending().await,
            }
        }
    }

    async fn teardown(
        &mut self,
        frontier: &mut ReceiveFrontier<'static, COUNT, BUFFER>,
        cause: WifiRxCause,
    ) -> WifiRxExit {
        let fault = cause != WifiRxCause::StopRequested;
        if fault {
            frontier.require_reset();
        }
        let quarantined = |reason| WifiRxExit {
            cause,
            end: WifiRxEnd::Quarantined {
                reason,
                walker_stopped: false,
            },
        };

        let masked = self.mask_all().await;
        self.routes.silence();
        deactivate_promiscuous_receive(self.hardware);
        self.hardware.request_mac_runtime_stop();
        if !masked {
            frontier.require_reset();
            return quarantined(WifiRxQuarantine::MaskTimeout);
        }
        if !self.drained().await {
            frontier.require_reset();
            return quarantined(WifiRxQuarantine::DrainTimeout);
        }
        if frontier.phase() != RxFrontierPhase::Live {
            return WifiRxExit {
                cause,
                end: WifiRxEnd::NotStarted,
            };
        }
        match frontier.stop(self.hardware) {
            Ok(()) if fault => WifiRxExit {
                cause,
                end: WifiRxEnd::Quarantined {
                    reason: WifiRxQuarantine::Fault,
                    walker_stopped: true,
                },
            },
            Ok(()) => WifiRxExit {
                cause,
                end: WifiRxEnd::Halted,
            },
            Err(error) => {
                frontier.require_reset();
                quarantined(WifiRxQuarantine::StopRefused(error))
            }
        }
    }

    /// Ask the line to mask both banks; whether it confirmed in time. The
    /// wait is counted.
    async fn mask_all(&mut self) -> bool {
        let start = self.timer.now();
        self.requests.ring(WifiIrqRequest::MaskAll, self.doorbell);
        let deadline = self.timer.deadline_after(MASK_CONFIRM_TIMEOUT).ok();
        let irq = &mut self.irq;
        let confirmed = poll_fn(|cx| {
            loop {
                match irq.poll_batch(cx) {
                    Poll::Ready(facts) if facts.masked() => return Poll::Ready(()),
                    // Facts posted before the mask end with this epoch.
                    Poll::Ready(_facts) => {}
                    Poll::Pending => return Poll::Pending,
                }
            }
        });
        let confirmed = within(self.timer, deadline, confirmed).await.is_some();
        self.counters
            .mask_confirm(self.timer.now().saturating_duration_since(start));
        confirmed
    }

    /// Wait until no lease is staged, no buffer is detached and the MAC
    /// runtime is idle; whether that happened in time. The wait and its
    /// polls are counted.
    async fn drained(&mut self) -> bool {
        let start = self.timer.now();
        let mut polls = 0;
        let drained = self.drain_checks(start, &mut polls).await;
        self.counters
            .drain(self.timer.now().saturating_duration_since(start), polls);
        drained
    }

    async fn drain_checks(&mut self, start: Instant, polls: &mut u32) -> bool {
        let deadline = start.checked_add(DRAIN_TIMEOUT);
        loop {
            if self.stage.claimed_slots() == 0
                && self.rx.detached_buffer_count() == 0
                && self.hardware.mac_runtime_active_state() == 0
            {
                return true;
            }
            if deadline.is_none_or(|deadline| self.timer.now() >= deadline) {
                return false;
            }
            match self.timer.deadline_after(DRAIN_POLL) {
                Ok(next) => {
                    *polls = polls.saturating_add(1);
                    self.timer.wait_until(next).await;
                }
                Err(oer_time::TimeOverflow) => return false,
            }
        }
    }

    /// Publish the exit and hold every owner forever.
    async fn park(&self, exit: WifiRxExit) -> Infallible {
        self.control.publish(exit);
        pending().await
    }
}

/// Wake once and let the executor run its other tasks.
async fn yield_now() {
    let mut yielded = false;
    poll_fn(|cx| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            cx.waker().wake_by_ref();
            Poll::Pending
        }
    })
    .await;
}

/// Run `future` until `deadline`; `None` when it did not finish in time.
/// A missing deadline (past the timer's range) is already reached: the
/// future is polled once.
async fn within<F: Future>(
    timer: &impl Timer,
    deadline: Option<Instant>,
    future: F,
) -> Option<F::Output> {
    let mut future = pin!(future);
    let mut expiry = pin!(async {
        if let Some(deadline) = deadline {
            timer.wait_until(deadline).await;
        }
    });
    poll_fn(|cx| {
        if let Poll::Ready(output) = future.as_mut().poll(cx) {
            return Poll::Ready(Some(output));
        }
        expiry.as_mut().poll(cx).map(|()| None)
    })
    .await
}

#[cfg(test)]
mod tests;
