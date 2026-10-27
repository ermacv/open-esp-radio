//! The P2 DTM executor. Its complete branded engine stays inside this owner.
use super::event::{Finish, successful};
use super::guard::{guard, step};
use super::{Cause, Fault, PacketCount, Start, Started};
use crate::{
    ControllerSchedulerEpoch,
    ble_phy::BlePhyTimingAuthority,
    controller_time::{ControllerTimeEventStep, ControllerTimeSample},
    interrupt::SchedulerWakeCell,
    roles::{DtmEventPhase, DtmImageBuilder, DtmSchedulerReceiverPhy, DtmSchedulerTransmitterPhy},
    runtime_resources::ControllerPoweredTaskRuntime,
    scheduler::{ControllerTimeAccess, SchedulerFinishedListWorkerStep},
};
use core::{
    future::poll_fn,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll},
};
use embassy_futures::select::{Either, select};

use oer_bluetooth_le_ll::dtm::{DtmPayloadPattern, DtmTest, next_window};
use oer_bluetooth_le_radio::{LeInstant, LeWindow, RadioDuration, RadioTiming, TestPhy};
use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerStop,
    engine::{
        ArenaMemory, DtmSchedulerFrequency, LeTxPower, SchedulerRoleInstance,
        scheduler::{SchedulerNext, SchedulerRawWindow, interrupt::SchedulerDoorbell},
    },
};
use oer_esp32s31_radio_client::bluetooth::{Operation, Permission};
use oer_espressif_interrupt_table::signal::SignalExecutor;
use oer_time::{Duration, Instant, Timer};

/// An independent clock request channel serviced even while a physical DTM
/// event is waiting for its IRQ. Its answer never waits for terminal dequeue.
pub trait ClockRequests {
    type Request;
    fn try_request(&self) -> Option<Self::Request>;
    fn poll_request(&self, cx: &mut Context<'_>) -> Poll<Self::Request>;
    fn answer(request: Self::Request, sample: Result<LeInstant, Fault>);
}

pub type Memory<'pool, 'epoch> = ArenaMemory<'pool, 'epoch, 0, 0, 0, 0, 0, 0, 1>;
/// Explicit image containment bounds, separate from physical DTM planning.
#[derive(Clone, Copy)]
pub struct Limits {
    recheck: Duration,
    command_turn: Duration,
    operation: Duration,
}
#[derive(Clone, Copy)]
struct PendingEvent {
    window: LeWindow,
    receiver: bool,
    deadline: Instant,
}
impl Limits {
    /// No zero-delay loops or unbounded waits.
    pub const fn new(
        recheck: Duration,
        command_turn: Duration,
        operation: Duration,
    ) -> Option<Self> {
        if recheck.as_micros() == 0
            || command_turn.as_micros() < recheck.as_micros()
            || operation.as_micros() <= command_turn.as_micros()
        {
            None
        } else {
            Some(Self {
                recheck,
                command_turn,
                operation,
            })
        }
    }
}
/// Actual P2 session owner. A failure leaves its instance, task and arena here;
/// neither cancellation nor an error path extracts a reusable graph.
pub struct Runner<'pool, 'epoch, X: SignalExecutor, D, T, C> {
    task: ControllerPoweredTaskRuntime<'static, X, Memory<'pool, 'epoch>>,
    timing: BlePhyTimingAuthority,
    doorbell: D,
    timer: T,
    wake: &'static SchedulerWakeCell<X>,
    clock: C,
    permission: Permission,
    limits: Limits,
    cancel: &'static AtomicBool,
    etm: Option<oer_esp32s31_hal::bluetooth::BlePhyEtmRouteDisabled>,
    instance: Option<SchedulerRoleInstance<'pool, 'epoch>>,
    config: Option<Start>,
    phase: DtmEventPhase,
    previous: Option<LeWindow>,
    pending: Option<PendingEvent>,
    payload: [u8; 255],
    count: PacketCount,
    faulted: bool,
    stop: BluetoothSchedulerStop,
}
impl<'pool, 'epoch, X: SignalExecutor, D: SchedulerDoorbell, T: Timer, C: ClockRequests>
    Runner<'pool, 'epoch, X, D, T, C>
{
    pub fn new(
        task: ControllerPoweredTaskRuntime<'static, X, Memory<'pool, 'epoch>>,
        timing: BlePhyTimingAuthority,
        doorbell: D,
        timer: T,
        wake: &'static SchedulerWakeCell<X>,
        permission: Permission,
        clock: C,
        limits: Limits,
        cancel: &'static AtomicBool,
    ) -> Self {
        Self {
            task,
            timing,
            doorbell,
            timer,
            wake,
            permission,
            clock,
            limits,
            cancel,
            instance: None,
            etm: None,
            config: None,
            phase: DtmEventPhase::Initial,
            previous: None,
            pending: None,
            payload: [0; 255],
            count: PacketCount::default(),
            faulted: false,
            stop: BluetoothSchedulerStop::default(),
        }
    }
    pub fn installation(&mut self) -> u32 {
        self.task.task_owner().installation()
    }
    pub fn is_running(&self) -> bool {
        self.config.is_some()
    }
    fn check(&self) -> Result<(), Fault> {
        if self.faulted || self.cancel.load(Ordering::Acquire) {
            Err(Fault::Abandoned)
        } else {
            Ok(())
        }
    }
    fn deadline(&self) -> Result<Instant, Fault> {
        self.timer
            .now()
            .checked_add(self.limits.operation)
            .ok_or(Fault::Hardware)
    }
    async fn service_clock(&mut self) -> Result<(), Fault> {
        if let Some(request) = self.clock.try_request() {
            let sample = self.now().await;
            C::answer(request, sample);
            sample.map(|_| ())?;
        }
        Ok(())
    }
    async fn stop(&mut self, deadline: Instant) -> Result<(), Fault> {
        if self.task.task_owner().engine().scheduler_is_stopped() {
            return Ok(());
        }
        loop {
            self.service_clock().await?;
            let step = guard(
                self.cancel,
                &self.timer,
                deadline,
                poll_fn(|cx| {
                    self.task
                        .task_owner()
                        .engine()
                        .poll_stop(&self.doorbell, &mut self.stop, cx)
                }),
            )
            .await
            .map_err(|fault| timeout_at(fault, Fault::StopTimeout))?
            .map_err(|_| Fault::SchedulerStop)?;
            if step {
                self.stop = BluetoothSchedulerStop::default();
                return Ok(());
            }
            let at = self
                .timer
                .now()
                .checked_add(self.limits.recheck)
                .ok_or(Fault::Hardware)?;
            guard(
                self.cancel,
                &self.timer,
                deadline,
                self.timer.wait_until(at.min(deadline)),
            )
            .await
            .map_err(|fault| timeout_at(fault, Fault::StopTimeout))?;
        }
    }
    async fn sample(
        &mut self,
        deadline: Instant,
    ) -> Result<(ControllerSchedulerEpoch, ControllerTimeSample, u64), Fault> {
        self.check()?;
        let mut overflows = self.wake.controller_time_overflows();
        let mut request = match self.task.controller_time_access() {
            ControllerTimeAccess::Idle(i) => i.request().map_err(|_| Fault::ClockBegin)?,
            _ => return Err(Fault::ClockBusy),
        };
        loop {
            if self.cancel.load(Ordering::Acquire) || self.timer.now() >= deadline {
                let _ = self.task.cancel_owned_controller_time(request);
                return Err(Fault::ClockTimeout);
            }
            match self
                .task
                .recheck_owned_controller_time(request)
                .map_err(|_| Fault::ClockRead)?
            {
                ControllerTimeEventStep::Sample { sample, .. } => {
                    // Pending hardware overflow must reach the software count
                    // even if source 124 has not caused a CPU entry yet. The
                    // doorbell entry services primary before resuming this P2 task.
                    guard(
                        self.cancel,
                        &self.timer,
                        deadline,
                        self.task
                            .task_owner()
                            .engine()
                            .interrupt_service_barrier(&self.doorbell),
                    )
                    .await?
                    .map_err(|_| Fault::ClockRead)?;
                    let projected = ControllerSchedulerEpoch::from_counted_live_update(
                        &sample,
                        overflows,
                        self.wake.controller_time_overflows(),
                        self.task.controller_time_scale(),
                    )
                    .map_err(Fault::ControllerTimeRange)?;
                    let Some((epoch, extended)) = projected else {
                        // A wrap may precede or follow the hardware latch. Retry
                        // under the original deadline instead of guessing its epoch.
                        overflows = self.wake.controller_time_overflows();
                        request = match self.task.controller_time_access() {
                            ControllerTimeAccess::Idle(i) => {
                                i.request().map_err(|_| Fault::ClockBegin)?
                            }
                            _ => return Err(Fault::ClockBusy),
                        };
                        continue;
                    };
                    return Ok((epoch, sample, extended));
                }
                ControllerTimeEventStep::Waiting => {}
                _ => return Err(Fault::Hardware),
            }
            let at = self
                .timer
                .now()
                .checked_add(self.limits.recheck)
                .ok_or(Fault::Hardware)?;
            self.timer.wait_until(at.min(deadline)).await;
        }
    }
    /// Fresh controller latch, independent of command/terminal dequeue.
    pub async fn now(&mut self) -> Result<LeInstant, Fault> {
        let deadline = self.deadline()?;
        let result = self
            .sample(deadline)
            .await
            .map(|(_, _, now)| LeInstant::from_micros(now));
        if result.is_err() {
            self.faulted = true;
        }
        result
    }
    pub async fn start(&mut self, config: Start) -> Result<Started, Fault> {
        self.check()?;
        if self.config.is_some() {
            return Ok(Started::NotExecuted(Cause::AirDenied));
        }
        match config {
            Start::Transmit(c) => {
                if c.phy != TestPhy::Le1M
                    || self.permission.tx_power(Operation::DtmTx, c.power).is_err()
                {
                    return Ok(Started::NotExecuted(Cause::AirDenied));
                }
            }
            Start::Receive(c) => {
                if c.phy != TestPhy::Le1M || self.permission.authorize(Operation::DtmRx).is_err() {
                    return Ok(Started::NotExecuted(Cause::AirDenied));
                }
            }
        }
        self.instance = self
            .task
            .task_owner()
            .engine()
            .dtm()
            .map_err(|_| Fault::Hardware)?
            .acquire();
        if self.instance.is_none() {
            return Err(Fault::Accounting);
        }
        self.etm = Some(self.task.task_owner().engine().disable_dtm_etm_route());
        self.config = Some(config);
        self.phase = DtmEventPhase::Initial;
        self.previous = None;
        self.count = PacketCount::default();
        let outcome = self.publish_event().await;
        match outcome {
            Ok(()) => Ok(Started::Executed),
            Err(fault) => {
                self.faulted = true;
                Err(fault)
            }
        }
    }
    pub async fn progress(&mut self) -> Result<(), Fault> {
        self.check()?;
        if self.config.is_none() {
            return Ok(());
        }
        let outcome = async {
            if self.pending.is_none() {
                self.publish_event().await?;
            }
            self.poll_event().await
        }
        .await;
        if outcome.is_err() {
            self.faulted = true;
        }
        outcome.map(|_| ())
    }
    async fn publish_event(&mut self) -> Result<(), Fault> {
        self.check()?;
        let deadline = self.deadline()?;
        self.stop(deadline).await?;
        let (epoch, sample, now) = self.sample(deadline).await?;
        let ready = self.timing.complete_always_awake(epoch, sample);
        let _fresh = ready.into_scheduler_instant();
        let config = self.config.ok_or(Fault::Accounting)?;
        let (test, channel, power, receiver) = match config {
            Start::Transmit(c) => (
                DtmTest::Transmit {
                    channel: c.channel,
                    phy: c.phy,
                    pattern: DtmPayloadPattern::from_hci_selector(c.payload.value())
                        .ok_or(Fault::Accounting)?,
                    length: c.length,
                },
                c.channel,
                self.permission
                    .tx_power(Operation::DtmTx, c.power)
                    .map_err(|_| Fault::Hardware)?,
                false,
            ),
            Start::Receive(c) => (
                DtmTest::Receive {
                    channel: c.channel,
                    phy: c.phy,
                },
                c.channel,
                // This inert descriptor value grants no permission to transmit.
                LeTxPower::from_dbm(LeTxPower::LOWEST_DBM).expect("provider minimum exists"),
                true,
            ),
        };
        let cfg = self.task.scheduler_config();
        let timing = RadioTiming {
            preparation_lead: RadioDuration::from_micros(u64::from(cfg.preparation_lead_micros())),
            admission_guard: RadioDuration::from_micros(u64::from(cfg.late_start_guard_micros())),
            ..RadioTiming::MINIMAL
        };
        let window = next_window(test, LeInstant::from_micros(now), timing, self.previous)
            .map_err(|_| Fault::Hardware)?;
        let reservation = timing.reservation(window).map_err(|_| Fault::Hardware)?;
        let start_delta = reservation
            .start()
            .checked_duration_since(LeInstant::from_micros(now))
            .ok_or(Fault::Hardware)?
            .as_micros();
        let end_delta = reservation
            .end()
            .checked_duration_since(LeInstant::from_micros(now))
            .ok_or(Fault::Hardware)?
            .as_micros();
        let start = epoch.raw_ticks_for_micros(
            (now as u32).wrapping_add(u32::try_from(start_delta).map_err(|_| Fault::Hardware)?),
        );
        let end = epoch.raw_ticks_for_micros(
            (now as u32).wrapping_add(u32::try_from(end_delta).map_err(|_| Fault::Hardware)?),
        );
        let raw = SchedulerRawWindow::from_projected_scheduler_window(start, end)
            .ok_or(Fault::Hardware)?;
        let sequence = epoch.raw_ticks_for_micros(epoch.project_capture(start));
        let frequency = DtmSchedulerFrequency::from_controller_image(channel.rf_channel() * 2)
            .ok_or(Fault::Hardware)?;
        let engine = self.task.task_owner().engine();
        let instance = self.instance.as_ref().ok_or(Fault::Accounting)?;
        let pool = engine.dtm().map_err(|_| Fault::Hardware)?;
        if let DtmTest::Transmit {
            pattern, length, ..
        } = test
        {
            pattern.fill(&mut self.payload[..usize::from(length)]);
            pool.prepare_tx_packet(instance, pattern.payload_type(), length, &self.payload)
                .map_err(|_| Fault::Hardware)?;
        }
        let builder = if receiver {
            DtmImageBuilder::receiver(
                self.phase,
                DtmSchedulerReceiverPhy::Le1M,
                power,
                crate::roles::STANDALONE_DTM_CONFIGURATION,
                frequency,
                raw,
                sequence,
            )
        } else {
            DtmImageBuilder::transmitter(
                self.phase,
                DtmSchedulerTransmitterPhy::Le1M,
                power,
                crate::roles::STANDALONE_DTM_CONFIGURATION,
                frequency,
                raw,
                sequence,
            )
        };
        builder
            .prepare(pool, instance)
            .map_err(|_| Fault::Hardware)?;
        guard(
            self.cancel,
            &self.timer,
            deadline,
            engine.resume(&self.doorbell),
        )
        .await
        .map_err(|fault| timeout_at(fault, Fault::ResumeTimeout))?
        .map_err(|_| Fault::SchedulerResume)?;
        let (_, next) = guard(
            self.cancel,
            &self.timer,
            deadline,
            engine.submit_dtm(instance, &self.doorbell),
        )
        .await
        .map_err(|fault| timeout_at(fault, Fault::SubmitTimeout))?
        .map_err(|_| Fault::SchedulerSubmit)?;
        if !matches!(next, SchedulerNext::Finished) {
            return Err(Fault::SchedulerSubmit);
        }
        self.pending = Some(PendingEvent {
            window,
            receiver,
            deadline,
        });
        Ok(())
    }
    /// Keep a listening receiver durable while returning to the command queue
    /// after one finite wait. Test End must not wait for a packet to arrive.
    async fn poll_event(&mut self) -> Result<(), Fault> {
        let event = self.pending.ok_or(Fault::Accounting)?;
        let deadline = if event.receiver {
            self.deadline()?
        } else {
            event.deadline
        };
        while self.task.task_owner().engine().scheduled_count() != 0 {
            let recheck = self
                .timer
                .now()
                .checked_add(self.limits.command_turn)
                .ok_or(Fault::Hardware)?;
            let input = step(
                self.cancel,
                &self.timer,
                deadline,
                recheck,
                select(self.wake.wait(), poll_fn(|cx| self.clock.poll_request(cx))),
            )
            .await
            .map_err(|fault| timeout_at(fault, Fault::WakeTimeout))?;
            let captured = match input {
                None => return Ok(()),
                Some(Either::First(captured)) => captured.map_err(|_| Fault::SchedulerWake)?,
                Some(Either::Second(request)) => {
                    let sample = self.now().await;
                    C::answer(request, sample);
                    sample?;
                    continue;
                }
            };
            let batch = captured.classify().map_err(|_| Fault::SchedulerWake)?;
            self.task
                .capture_scheduler_finished_lists(batch)
                .map_err(|_| Fault::SchedulerWake)?;
            loop {
                match self.task.scheduler_finished_lists().step() {
                    SchedulerFinishedListWorkerStep::List { observed, .. } => {
                        self.task
                            .task_owner()
                            .engine()
                            .complete(observed)
                            .map_err(|_| Fault::SchedulerComplete)?;
                    }
                    SchedulerFinishedListWorkerStep::Complete
                    | SchedulerFinishedListWorkerStep::Idle => break,
                }
            }
        }
        self.stop(self.deadline()?).await?;
        self.finish_event(Finish::Natural)
    }
    fn finish_event(&mut self, finish: Finish) -> Result<(), Fault> {
        let pending = self.pending.ok_or(Fault::Accounting)?;
        let event = self
            .task
            .task_owner()
            .engine()
            .dtm()
            .map_err(|_| Fault::Hardware)?
            .finish_event(
                self.instance.as_ref().ok_or(Fault::Accounting)?,
                pending.receiver,
            )
            .map_err(|_| Fault::DtmFinish)?;
        self.previous = Some(pending.window);
        self.pending = None;
        self.phase = DtmEventPhase::Recurring;
        if successful(event.status, finish)? {
            if pending.receiver && matches!(event.received, Some(Ok(_))) {
                if let Some(n) = self.count.packets.checked_add(1) {
                    self.count.packets = n;
                } else {
                    self.count.saturated = true;
                }
            }
        }
        Ok(())
    }
    /// Normal End reclaims exactly this session's graph after confirmed stop.
    pub async fn end(&mut self) -> Result<PacketCount, Fault> {
        let result = self.end_inner().await;
        if result.is_err() {
            self.faulted = true;
        }
        result
    }
    async fn end_inner(&mut self) -> Result<PacketCount, Fault> {
        self.check()?;
        let deadline = self.deadline()?;
        if let Some(captured) = self.wake.take().map_err(|_| Fault::SchedulerWake)? {
            // Captured P3 faults still preempt the ordinary End path.
            let _ = captured.classify().map_err(|_| Fault::SchedulerWake)?;
        }
        self.stop(deadline).await?;
        if let Some(captured) = self.wake.take().map_err(|_| Fault::SchedulerWake)? {
            let _ = captured.classify().map_err(|_| Fault::SchedulerWake)?;
        }
        if self.task.task_owner().engine().scheduled_count() != 0 {
            let next = guard(
                self.cancel,
                &self.timer,
                deadline,
                self.task.task_owner().engine().flush(&self.doorbell),
            )
            .await?
            .map_err(|_| Fault::SchedulerComplete)?;
            if !matches!(next, SchedulerNext::Finished) {
                return Err(Fault::SchedulerComplete);
            }
        }
        if self.pending.is_some() {
            self.finish_event(Finish::Stopped)?;
        }
        let pool = self
            .task
            .task_owner()
            .engine()
            .dtm()
            .map_err(|_| Fault::Hardware)?;
        if let Some(instance) = self.instance.take() {
            if let Err(failed) = pool.release(instance) {
                self.instance = Some(failed.instance);
                self.faulted = true;
                return Err(Fault::Hardware);
            }
        }
        if let Some(route) = self.etm.take() {
            self.task.task_owner().engine().restore_dtm_etm_route(route);
        }
        self.config = None;
        Ok(self.count)
    }
    /// Only a normal stopped session yields its actual task endpoint to route
    /// removal and the actor's retirement/reset sequence.
    pub fn into_stopped(
        self,
    ) -> Result<ControllerPoweredTaskRuntime<'static, X, Memory<'pool, 'epoch>>, Self> {
        if self.stop.is_pending()
            || self.instance.is_some()
            || self.etm.is_some()
            || self.config.is_some()
            || self.pending.is_some()
            || self.faulted
            || self.cancel.load(Ordering::Acquire)
        {
            Err(self)
        } else {
            Ok(self.task)
        }
    }
}
fn timeout_at(fault: Fault, stage: Fault) -> Fault {
    if fault == Fault::Deadline {
        stage
    } else {
        fault
    }
}
