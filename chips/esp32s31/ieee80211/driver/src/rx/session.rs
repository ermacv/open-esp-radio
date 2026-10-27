//! Reusable receive ownership across stopped channel transactions. The line
//! stays in its handler cell and the radio handler holds the interrupt
//! banks; the session asks it to activate the epoch at each start and to
//! close it after the normal mask/drain/walker stop. The CPU route stays as
//! the image's binding wrote it.

use super::{
    arena::WifiRxParts,
    control::{ReceiveRunnerControl, WifiRxCause, WifiRxEnd, WifiRxExit},
    counters::WifiRxCounters,
    frontier::ReceiveFrontier,
    receive::Receiver,
    sink::CountingAdmission,
    transaction::Publisher,
};
use crate::irq::{RadioDoorbell, WifiIrqActivation, WifiIrqLine, WifiIrqRequest, WifiIrqRequests};
use core::task::{Context, Poll};
use oer_esp32s31_hal::{
    ieee80211::dma::{rx_storage::RxDmaStorage, space::ChipDma},
    owner::RadioAccess,
};
use oer_esp32s31_ieee80211_mac::{
    init::{MacRuntimeStopHardware, activate_promiscuous_receive},
    irq::consumer::{IrqBatch, IrqConsumer, PowerIrqConsumer},
    rx::pool::RxStagePool,
};
use oer_espressif_interrupt_table::signal::SignalExecutor;
use oer_memory::HandlerCell;
use oer_time::{Instant, Timer};

/// The receive owners of one Wi-Fi arena across its channel transactions.
///
/// `V` is the router's proof that the image binds the radio handler's part
/// ([`Bound`](oer_espressif_interrupt_table::Bound) of a
/// [`WifiIrqPart`](crate::irq::WifiIrqPart)); the session holds it, and
/// every activation takes it.
pub struct ReceiveSession<
    const N: usize,
    const B: usize,
    const S: usize,
    const Q: usize,
    X: SignalExecutor,
    V: WifiIrqActivation,
> {
    rx: &'static RxDmaStorage<N, B, S>,
    stage: &'static RxStagePool<Q, B>,
    irq: IrqConsumer<'static, X>,
    power: PowerIrqConsumer<'static, X>,
    requests: &'static WifiIrqRequests,
    counters: &'static WifiRxCounters,
    control: ReceiveRunnerControl<'static>,
    frontier: ReceiveFrontier<'static, N, B>,
    bound: V,
    totals: [u64; 3],
    full_since: Option<(Instant, u32)>,
    port_settle: Option<Instant>,
    port_mask_started: Option<Instant>,
    port_masked: bool,
    port_frozen: bool,
    port_stop_deadline: Option<Instant>,
    port_close_started: Option<Instant>,
}

impl<const N: usize, const B: usize, const S: usize, const Q: usize, X: SignalExecutor, V>
    ReceiveSession<N, B, S, Q, X, V>
where
    V: WifiIrqActivation,
{
    /// Bind once while the role is stopped: install the interrupt line in
    /// the radio handler's `cell`, keep `bound`, the proof that the image's
    /// interrupt table binds the handler's part. Failure is terminal for
    /// this arena; the caller retains its role/register owners for
    /// quarantine or reset.
    pub fn new(
        parts: WifiRxParts<'static, N, B, S, Q, ChipDma, X>,
        bound: V,
        registers: &mut RadioAccess,
        cell: &'static HandlerCell<WifiIrqLine<X>>,
    ) -> Result<Self, WifiRxCause> {
        let WifiRxParts {
            rx,
            stage,
            irq,
            power,
            requests,
            counters,
            control,
            line,
            returns: _,
        } = parts;
        let storage = rx.storage();
        let (mut mac, dma) = registers.split_rx_dma();
        let frontier =
            ReceiveFrontier::prepare_initial(&mut mac, dma, rx).map_err(WifiRxCause::Prepare)?;
        if line.install(cell).is_err() {
            return Err(WifiRxCause::LineRefused);
        }
        Ok(Self {
            rx: storage,
            stage,
            irq,
            power,
            requests,
            counters,
            control,
            frontier,
            bound,
            totals: [0; 3],
            full_since: None,
            port_settle: None,
            port_mask_started: None,
            port_masked: false,
            port_frozen: false,
            port_stop_deadline: None,
            port_close_started: None,
        })
    }

    pub const fn counters(&self) -> &'static WifiRxCounters {
        self.counters
    }

    /// Start the initial prepared ring, or the same ring after a retune:
    /// the radio handler activates the interrupt epoch before the walker
    /// starts.
    pub async fn start(
        &mut self,
        registers: &mut RadioAccess,
        timer: &impl Timer,
        doorbell: &impl RadioDoorbell,
    ) -> Result<(), WifiRxCause> {
        let (mut mac, dma) = registers.split_rx_dma();
        self.frontier
            .prepare_with_storage(dma, self.rx)
            .map_err(WifiRxCause::Start)?;
        activate_promiscuous_receive(&mut mac);
        self.requests.activate(&self.bound, doorbell);
        super::storage::walker_enable_settle(timer).await;
        self.frontier
            .start_prepared(dma)
            .map_err(WifiRxCause::Start)?;
        mac.start_mac_runtime(self.frontier.live_mut().map_err(WifiRxCause::Start)?);
        Ok(())
    }

    /// Every MAC fact is returned, including TX completion/timeout/color facts.
    /// The caller services TX in the same task before discarding the batch.
    pub fn poll(&mut self, cx: &mut Context<'_>) -> Poll<Result<IrqBatch, WifiRxCause>> {
        while let Poll::Ready(Ok(_)) = self.power.poll(cx) {}
        self.irq
            .poll_batch(cx)
            .map(|r| r.map_err(|_| WifiRxCause::ForeignExecutor))
    }

    /// Drain through the same bounded service/reload path as W1. Frames remain
    /// affine leases in the supplied publisher until its receiver drops them.
    pub async fn drain<P: Publisher<'static, B, Q>>(
        &mut self,
        registers: &mut RadioAccess,
        doorbell: &impl RadioDoorbell,
        timer: &impl Timer,
        publisher: &P,
    ) -> Result<(), WifiRxCause> {
        let (mut mac, dma) = registers.split_rx_dma();
        let admission = CountingAdmission::new(self.counters);
        let mut receiver = Receiver {
            hardware: &mut mac,
            rx_dma: dma,
            rx: self.rx,
            stage: self.stage,
            irq: &mut self.irq,
            power: &mut self.power,
            requests: self.requests,
            counters: self.counters,
            control: &mut self.control,
            doorbell,
            timer,
            publisher,
            admission: &admission,
            totals: &mut self.totals,
            full_since: &mut self.full_since,
        };
        receiver
            .drain(self.frontier.live_mut().map_err(WifiRxCause::Start)?, None)
            .await
    }

    /// Stop once, then close the interrupt epoch, only after the normal
    /// mask, lease drain and walker stop. The caller must drain its frame
    /// receiver before awaiting this boundary; no held frame is discarded.
    pub async fn pause<P: Publisher<'static, B, Q>>(
        &mut self,
        registers: &mut RadioAccess,
        doorbell: &impl RadioDoorbell,
        timer: &impl Timer,
        publisher: &P,
    ) -> Result<(), WifiRxExit> {
        let (mut mac, dma) = registers.split_rx_dma();
        let admission = CountingAdmission::new(self.counters);
        let mut receiver = Receiver {
            hardware: &mut mac,
            rx_dma: dma,
            rx: self.rx,
            stage: self.stage,
            irq: &mut self.irq,
            power: &mut self.power,
            requests: self.requests,
            counters: self.counters,
            control: &mut self.control,
            doorbell,
            timer,
            publisher,
            admission: &admission,
            totals: &mut self.totals,
            full_since: &mut self.full_since,
        };
        let exit = receiver
            .teardown(&mut self.frontier, WifiRxCause::StopRequested)
            .await;
        if exit.end != WifiRxEnd::Halted {
            return Err(exit);
        }
        let refused = WifiRxExit {
            cause: WifiRxCause::LineRefused,
            end: WifiRxEnd::Halted,
        };
        self.requests.ring(WifiIrqRequest::Close, doorbell);
        let deadline = timer
            .deadline_after(super::receive::MASK_CONFIRM_TIMEOUT)
            .map_err(|_| refused)?;
        while self.requests.is_pending() {
            if timer.now() >= deadline {
                return Err(refused);
            }
            timer
                .wait_for(super::receive::DRAIN_POLL)
                .await
                .map_err(|_| refused)?;
        }
        Ok(())
    }

    /// Prepare a closed session to start again: the next start activates a
    /// fresh interrupt epoch.
    pub fn reattach(&mut self) {
        self.port_mask_started = None;
        self.port_masked = false;
        self.port_frozen = false;
        self.port_stop_deadline = None;
        self.port_close_started = None;
    }

    pub(crate) fn port_deadline(&self) -> Option<Instant> {
        [
            self.port_settle,
            self.port_mask_started
                .map(|start| start.saturating_add(super::receive::MASK_CONFIRM_TIMEOUT)),
            self.port_stop_deadline,
            self.port_close_started
                .map(|start| start.saturating_add(super::receive::MASK_CONFIRM_TIMEOUT)),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Cancellation-safe start: the prepared ring stays in the session while
    /// the executor waits for the walker-enable settle.
    pub(crate) fn port_start(
        &mut self,
        registers: &mut RadioAccess,
        timer: &impl Timer,
        doorbell: &impl RadioDoorbell,
    ) -> Poll<Result<(), WifiRxCause>> {
        use super::storage::ESP32S31_RX_WALKER_ENABLE_SETTLE_US;
        if self.frontier.phase() == super::frontier::RxFrontierPhase::Live {
            return Poll::Ready(Ok(()));
        }
        let deadline = match self.port_settle {
            Some(deadline) => deadline,
            None => {
                let (mut mac, dma) = registers.split_rx_dma();
                self.frontier
                    .prepare_with_storage(dma, self.rx)
                    .map_err(WifiRxCause::Start)?;
                activate_promiscuous_receive(&mut mac);
                self.requests.activate(&self.bound, doorbell);
                let deadline = timer
                    .now()
                    .checked_add(oer_time::Duration::from_micros(u64::from(
                        ESP32S31_RX_WALKER_ENABLE_SETTLE_US,
                    )))
                    .ok_or(WifiRxCause::Start(
                        super::frontier::RxFrontierError::OwnerUnavailable,
                    ))?;
                self.port_settle = Some(deadline);
                deadline
            }
        };
        if timer.now() < deadline {
            return Poll::Pending;
        }
        let (mut mac, dma) = registers.split_rx_dma();
        self.frontier
            .start_prepared(dma)
            .map_err(WifiRxCause::Start)?;
        mac.start_mac_runtime(self.frontier.live_mut().map_err(WifiRxCause::Start)?);
        self.port_settle = None;
        Poll::Ready(Ok(()))
    }

    pub(crate) fn port_service<P: Publisher<'static, B, Q>>(
        &mut self,
        registers: &mut RadioAccess,
        doorbell: &impl RadioDoorbell,
        timer: &impl Timer,
        publisher: &P,
    ) -> Result<crate::datapath::DatapathRxProgress, WifiRxCause> {
        let (mut mac, dma) = registers.split_rx_dma();
        let admission = CountingAdmission::new(self.counters);
        let mut receiver = Receiver {
            hardware: &mut mac,
            rx_dma: dma,
            rx: self.rx,
            stage: self.stage,
            irq: &mut self.irq,
            power: &mut self.power,
            requests: self.requests,
            counters: self.counters,
            control: &mut self.control,
            doorbell,
            timer,
            publisher,
            admission: &admission,
            totals: &mut self.totals,
            full_since: &mut self.full_since,
        };
        receiver.service_once(self.frontier.live_mut().map_err(WifiRxCause::Start)?)
    }

    pub(crate) fn port_unmask(&self, doorbell: &impl RadioDoorbell) {
        if !self.port_masked {
            self.requests.ring(WifiIrqRequest::UnmaskRx, doorbell);
        }
    }

    pub(crate) fn port_power(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Result<oer_esp32s31_hal::types::MacPowerInterruptObservation, WifiRxCause>> {
        self.power
            .poll(cx)
            .map(|r| r.map_err(|_| WifiRxCause::ForeignExecutor))
    }

    /// Freeze ingress at a finite hardware/ISR boundary. Outstanding upper
    /// leases are already ordered events and need not be returned for a GTK
    /// change. The live ring and every detached allocation remain owned here.
    pub(crate) fn port_freeze(
        &mut self,
        cx: &mut Context<'_>,
        registers: &mut RadioAccess,
        doorbell: &impl RadioDoorbell,
        timer: &impl Timer,
    ) -> Poll<Result<(), WifiRxCause>> {
        use oer_esp32s31_ieee80211_mac::init::{
            MacRuntimeStopHardware, deactivate_promiscuous_receive,
        };
        if !self.port_masked {
            let start = *self.port_mask_started.get_or_insert_with(|| {
                self.requests.ring(WifiIrqRequest::MaskAll, doorbell);
                timer.now()
            });
            for _ in 0..4 {
                match self.irq.poll_batch(cx) {
                    Poll::Ready(Ok(batch)) if batch.masked() => {
                        self.port_masked = true;
                        self.port_mask_started = None;
                        break;
                    }
                    Poll::Ready(Ok(_)) => {}
                    Poll::Ready(Err(_)) => return Poll::Ready(Err(WifiRxCause::ForeignExecutor)),
                    Poll::Pending => break,
                }
            }
            if !self.port_masked {
                if timer.now().saturating_duration_since(start)
                    >= super::receive::MASK_CONFIRM_TIMEOUT
                {
                    return Poll::Ready(Err(WifiRxCause::LineRefused));
                }
                return Poll::Pending;
            }
            let mut mac = registers.wifi_mac_hal();
            deactivate_promiscuous_receive(&mut mac);
            mac.request_mac_runtime_stop();
            self.port_stop_deadline =
                Some(timer.now().saturating_add(super::receive::DRAIN_TIMEOUT));
        }
        if registers.wifi_mac_hal().mac_runtime_active_state() != 0 {
            if self
                .port_stop_deadline
                .is_some_and(|deadline| timer.now() >= deadline)
            {
                return Poll::Ready(Err(WifiRxCause::Start(
                    super::frontier::RxFrontierError::OwnerUnavailable,
                )));
            }
            // This wait is timer-driven even if the stopped ingress raises no IRQ.
            self.port_settle = Some(timer.now().saturating_add(super::receive::DRAIN_POLL));
            return Poll::Pending;
        }
        self.port_settle = None;
        self.port_stop_deadline = None;
        self.port_frozen = true;
        Poll::Ready(Ok(()))
    }

    /// Resume a frozen ingress: the radio handler activates a fresh epoch
    /// (its stale events cleared), then receive reopens and the MAC runtime
    /// resumes.
    pub(crate) fn port_resume_cut(
        &mut self,
        registers: &mut RadioAccess,
        doorbell: &impl RadioDoorbell,
    ) -> Result<(), WifiRxCause> {
        if !self.port_frozen {
            return Ok(());
        }
        self.reattach();
        self.requests.activate(&self.bound, doorbell);
        activate_promiscuous_receive(&mut registers.wifi_mac_hal());
        registers.wifi_mac_hal().resume_mac_runtime();
        Ok(())
    }

    /// Freeze, drain and stop, then ask the radio handler to close the
    /// interrupt epoch and wait (timer-woken, bounded) until it took the
    /// request.
    pub(crate) fn port_pause(
        &mut self,
        cx: &mut Context<'_>,
        registers: &mut RadioAccess,
        doorbell: &impl RadioDoorbell,
        timer: &impl Timer,
    ) -> Poll<Result<(), WifiRxCause>> {
        core::task::ready!(self.port_freeze(cx, registers, doorbell, timer))?;
        if self.stage.claimed_slots() != 0 || self.rx.detached_buffer_count() != 0 {
            self.port_settle = Some(timer.now().saturating_add(super::receive::DRAIN_POLL));
            return Poll::Pending;
        }
        if self.frontier.phase() == super::frontier::RxFrontierPhase::Live {
            let (_, dma) = registers.split_rx_dma();
            self.frontier.stop(dma).map_err(WifiRxCause::Start)?;
        }
        self.port_settle = None;
        let start = *self.port_close_started.get_or_insert_with(|| {
            self.requests.ring(WifiIrqRequest::Close, doorbell);
            timer.now()
        });
        if self.requests.is_pending() {
            if timer.now().saturating_duration_since(start) >= super::receive::MASK_CONFIRM_TIMEOUT
            {
                return Poll::Ready(Err(WifiRxCause::LineRefused));
            }
            self.port_settle = Some(timer.now().saturating_add(super::receive::DRAIN_POLL));
            return Poll::Pending;
        }
        self.port_close_started = None;
        self.port_settle = None;
        Poll::Ready(Ok(()))
    }

    /// Ask the radio handler to mask both banks; latched events stay.
    pub(crate) fn port_mask_all(&self, doorbell: &impl RadioDoorbell) {
        self.requests.ring(WifiIrqRequest::MaskAll, doorbell);
    }

    pub(crate) fn port_abandon(&mut self, doorbell: &impl RadioDoorbell) {
        self.requests.ring(WifiIrqRequest::MaskAll, doorbell);
        self.frontier.require_reset();
    }
}
