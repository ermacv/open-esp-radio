//! One affine request/reply slot between the scheduler task and P3.
//!
//! The slot owns each request until P3 consumes it, then owns the reply until
//! P2 takes it. Only the readiness notification coalesces. Neither context
//! calls the other or borrows its registers, and no waker crosses into P3.

use core::{
    future::poll_fn,
    sync::atomic::{AtomicU32, Ordering},
    task::{Context, Poll},
};

use oer_espressif_bluetooth_engine::port::OutputInterruptPort;
use oer_memory::{InPlace, InPlaceClient, InPlaceServer, RequestSlot};

use crate::bluetooth::{
    BluetoothControllerOutputReleaseError, BluetoothDynamicSourcesMasked,
    BluetoothOutputIdleSampled, BluetoothOutputReleaseAdmitted, BluetoothOutputReleased,
    BluetoothSchedulerBusyObservation, BluetoothSchedulerCancellationIndexed,
    BluetoothSchedulerCancellationSourceAcknowledged, BluetoothSchedulerHeadsAbsent,
    BluetoothSchedulerLockModifyInterruptObservation, BluetoothSchedulerRunDisabled,
    BluetoothSchedulerRunInterruptsPrepared, InterruptRegistersOwner,
};

/// The image's shared P3 radio doorbell and P2 reply signal.
///
/// `ring` only pends the radio doorbell. `poll_reply` registers and uses a
/// waker only in P2, through its executor's static typed signal. P3 publishes
/// that signal after [`SchedulerInterruptHalf::service`] returns `true`.
pub trait SchedulerDoorbell {
    fn ring(&self);
    fn poll_reply(&self, cx: &mut Context<'_>) -> Poll<Result<(), SchedulerInterruptError>>;
}

/// Typed, out-of-line clock read for the bounded P3 diagnostic settles.
/// It reads the exact supplied clock; the boundary prevents each command's
/// generic settle loop from inlining another copy of its conversion code.
pub struct SchedulerInterruptClock<'a, C: oer_time::Clock>(&'a C);
impl<'a, C: oer_time::Clock> SchedulerInterruptClock<'a, C> {
    pub const fn new(clock: &'a C) -> Self {
        Self(clock)
    }
}
impl<C: oer_time::Clock> oer_time::Clock for SchedulerInterruptClock<'_, C> {
    #[inline(never)]
    fn now(&self) -> oer_time::Instant {
        self.0.now()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SchedulerInterruptError {
    NotInstalled,
    Occupied,
    Abandoned,
    SequenceExhausted,
    ForeignReply,
    ForeignContext,
    DiagnosticUnsettled,
    Quarantined,
    NotStopped,
    /// The epoch's output is released: no scheduler-run group is enabled
    /// again.
    OutputReleased,
}

enum Command {
    ServiceBarrier,
    Busy,
    LockModify,
    PrepareRun,
    AcknowledgeCancellation(BluetoothSchedulerCancellationIndexed),
    MaskRun,
    SelectProgress,
    Progress,
    SelectSettle,
    Settled,
    SampleIdleOutput(BluetoothOutputReleaseAdmitted),
    MaskOutput(BluetoothSchedulerHeadsAbsent),
    ReleaseOutput(BluetoothSchedulerRunDisabled),
}

enum Reply {
    Busy(BluetoothSchedulerBusyObservation),
    LockModify(BluetoothSchedulerLockModifyInterruptObservation),
    Prepared(BluetoothSchedulerRunInterruptsPrepared),
    Cancellation(BluetoothSchedulerCancellationSourceAcknowledged),
    Done,
    Flag(bool),
    IdleSampled(Result<BluetoothOutputIdleSampled, BluetoothControllerOutputReleaseError>),
    OutputMasked(Result<BluetoothDynamicSourcesMasked, BluetoothControllerOutputReleaseError>),
    OutputReleased(Result<BluetoothOutputReleased, BluetoothControllerOutputReleaseError>),
}

// Initialized by P2 before POSTED. P3 borrows this one message in place,
// writes only the reply, then publishes ANSWERED. In particular the indexed
// cancellation owner never needs an enum/envelope move through P3.
struct Exchange {
    epoch: u32,
    sequence: u32,
    command: Command,
    reply: Result<Reply, SchedulerInterruptError>,
}

/// Static, allocation-free storage for one controller installation.
///
/// Both endpoints must reunite after scheduler stop before another epoch
/// can install. A pending or abandoned request keeps the slot reserved.
pub struct SchedulerInterruptChannel {
    slot: RequestSlot<Exchange, (), InPlace>,
    sequence: AtomicU32,
}

impl Default for SchedulerInterruptChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl SchedulerInterruptChannel {
    pub const fn new() -> Self {
        Self {
            slot: RequestSlot::new_in_place(),
            sequence: AtomicU32::new(0),
        }
    }

    pub(crate) fn install(
        &'static self,
        epoch: u32,
    ) -> Result<(SchedulerTaskHalf, SchedulerInterruptHalf), SchedulerInterruptError> {
        let (client, server) = self
            .slot
            .claim_in_place()
            .ok_or(SchedulerInterruptError::Occupied)?;
        let sequence = self.sequence.load(Ordering::Relaxed);
        Ok((
            SchedulerTaskHalf {
                channel: self,
                epoch,
                sequence,
                pending: false,
                client,
            },
            SchedulerInterruptHalf {
                channel: self,
                epoch,
                sequence,
                server,
            },
        ))
    }
}

/// P3's command endpoint. The image stores it beside its unique IRQ owner.
/// It contains no task register or arena authority.
#[must_use]
pub struct SchedulerInterruptHalf {
    channel: &'static SchedulerInterruptChannel,
    epoch: u32,
    sequence: u32,
    server: InPlaceServer<Exchange>,
}

pub(crate) struct SchedulerTaskHalf {
    channel: &'static SchedulerInterruptChannel,
    epoch: u32,
    sequence: u32,
    pending: bool,
    client: InPlaceClient<Exchange>,
}

impl SchedulerInterruptHalf {
    /// Execute at most one request in the P3 doorbell handler, after that
    /// handler acknowledges the shared software interrupt. The primary,
    /// NRT and doorbell entries use the same stable IRQ owner at P3/core 0.
    /// A true result requires publishing the P2 reply signal.
    pub fn service(
        &mut self,
        interrupts: &mut InterruptRegistersOwner,
        clock: &impl oer_time::Clock,
    ) -> Result<bool, crate::bluetooth::BluetoothInterruptPolicyFault> {
        let clock = SchedulerInterruptClock::new(clock);
        let mut fault = None;
        let replied = self.service_with(|command| {
            let settle = oer_time::settle::Settle::new(super::DIAGNOSTIC_SETTLE, &clock);
            let unsettled = |_| SchedulerInterruptError::DiagnosticUnsettled;
            let reply = match command {
                Command::ServiceBarrier => Reply::Done,
                Command::Busy => Reply::Busy(
                    interrupts
                        .capture_scheduler_busy(settle)
                        .map_err(unsettled)?,
                ),
                Command::LockModify => Reply::LockModify(
                    interrupts
                        .capture_scheduler_lock_modify_interrupt(settle)
                        .map_err(unsettled)?,
                ),
                Command::PrepareRun => Reply::Prepared(
                    interrupts
                        .prepare_scheduler_run_interrupts()
                        .ok_or(SchedulerInterruptError::OutputReleased)?,
                ),
                Command::AcknowledgeCancellation(indexed) => {
                    let acknowledged =
                        interrupts.acknowledge_scheduler_cancellation_source(indexed);
                    Reply::Cancellation(acknowledged)
                }
                Command::MaskRun => {
                    interrupts.mask_scheduler_run_interrupts();
                    Reply::Done
                }
                Command::SelectProgress => {
                    interrupts.select_scheduler_execution_modify_progress();
                    Reply::Done
                }
                Command::Progress => Reply::Flag(
                    interrupts
                        .scheduler_execution_modify_repeats(settle)
                        .map_err(unsettled)?,
                ),
                Command::SelectSettle => {
                    interrupts.select_scheduler_execution_modify_settle();
                    Reply::Done
                }
                Command::Settled => Reply::Flag(
                    interrupts
                        .scheduler_execution_modify_settled(settle)
                        .map_err(unsettled)?,
                ),
                Command::SampleIdleOutput(admitted) => {
                    Reply::IdleSampled(interrupts.sample_idle(admitted, settle))
                }
                Command::MaskOutput(heads) => Reply::OutputMasked(interrupts.mask_dynamic(heads)),
                Command::ReleaseOutput(run) => {
                    Reply::OutputReleased(interrupts.release_idle(run, settle))
                }
            };
            if let Reply::IdleSampled(Err(
                BluetoothControllerOutputReleaseError::InterruptPolicy(cause),
            ))
            | Reply::OutputMasked(Err(
                BluetoothControllerOutputReleaseError::InterruptPolicy(cause),
            ))
            | Reply::OutputReleased(Err(
                BluetoothControllerOutputReleaseError::InterruptPolicy(cause),
            )) = &reply
            {
                fault = Some(*cause);
            }
            Ok(reply)
        });
        match fault {
            Some(fault) => Err(fault),
            None => Ok(replied),
        }
    }

    fn service_with(
        &mut self,
        execute: impl FnOnce(&Command) -> Result<Reply, SchedulerInterruptError>,
    ) -> bool {
        let epoch = self.epoch;
        let sequence = &mut self.sequence;
        self.server.service(|request| {
            let next = sequence.checked_add(1);
            request.reply = if request.epoch != epoch || next != Some(request.sequence) {
                Err(SchedulerInterruptError::ForeignReply)
            } else {
                *sequence = request.sequence;
                execute(&request.command)
            };
        })
    }
}

impl SchedulerTaskHalf {
    pub(crate) fn retire(
        self,
        interrupts: SchedulerInterruptHalf,
    ) -> Result<(), (SchedulerInterruptError, Self, SchedulerInterruptHalf)> {
        if !core::ptr::eq(self.channel, interrupts.channel) || self.epoch != interrupts.epoch {
            return Err((SchedulerInterruptError::ForeignReply, self, interrupts));
        }
        if !self.is_quiet() || !self.client.is_idle() {
            return Err((SchedulerInterruptError::Abandoned, self, interrupts));
        }
        match self.client.reunite(interrupts.server) {
            Ok(()) => Ok(()),
            Err((client, server)) => Err((
                SchedulerInterruptError::Abandoned,
                Self { client, ..self },
                SchedulerInterruptHalf {
                    server,
                    ..interrupts
                },
            )),
        }
    }

    pub(crate) const fn is_quiet(&self) -> bool {
        !self.pending
    }

    async fn exchange(
        &mut self,
        bell: &impl SchedulerDoorbell,
        command: Command,
    ) -> Result<(Command, Reply), SchedulerInterruptError> {
        self.begin(bell, command)?;
        poll_fn(|cx| self.poll_exchange(bell, cx)).await
    }

    fn begin(
        &mut self,
        bell: &impl SchedulerDoorbell,
        command: Command,
    ) -> Result<(), SchedulerInterruptError> {
        if self.pending {
            return Err(SchedulerInterruptError::Abandoned);
        }
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(SchedulerInterruptError::SequenceExhausted)?;
        if !self.client.is_idle() {
            return Err(SchedulerInterruptError::Occupied);
        }
        self.client
            .post(Exchange {
                epoch: self.epoch,
                sequence,
                command,
                // P3 replaces this before ANSWERED; P2 never interprets the
                // request's initial reply as a hardware observation.
                reply: Err(SchedulerInterruptError::Abandoned),
            })
            .map_err(|_| SchedulerInterruptError::Occupied)?;
        self.sequence = sequence;
        self.channel.sequence.store(sequence, Ordering::Relaxed);
        self.pending = true;
        bell.ring();
        Ok(())
    }

    fn poll_exchange(
        &mut self,
        bell: &impl SchedulerDoorbell,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(Command, Reply), SchedulerInterruptError>> {
        if let Some(reply) = self.take_reply() {
            return Poll::Ready(reply);
        }
        if let Poll::Ready(Err(error)) = bell.poll_reply(cx) {
            return Poll::Ready(Err(error));
        }
        // P3 may reply between the first load and P2's waker registration.
        match self.take_reply() {
            Some(reply) => Poll::Ready(reply),
            None => Poll::Pending,
        }
    }

    pub(crate) fn begin_busy(
        &mut self,
        bell: &impl SchedulerDoorbell,
    ) -> Result<(), SchedulerInterruptError> {
        self.begin(bell, Command::Busy)
    }
    pub(crate) fn poll_busy(
        &mut self,
        bell: &impl SchedulerDoorbell,
        cx: &mut Context<'_>,
    ) -> Poll<Result<BluetoothSchedulerBusyObservation, SchedulerInterruptError>> {
        self.poll_exchange(bell, cx).map(|result| match result? {
            (Command::Busy, Reply::Busy(busy)) => Ok(busy),
            _ => Err(SchedulerInterruptError::ForeignReply),
        })
    }
    pub(crate) fn begin_mask_run(
        &mut self,
        bell: &impl SchedulerDoorbell,
    ) -> Result<(), SchedulerInterruptError> {
        self.begin(bell, Command::MaskRun)
    }
    pub(crate) fn poll_mask_run(
        &mut self,
        bell: &impl SchedulerDoorbell,
        cx: &mut Context<'_>,
    ) -> Poll<Result<(), SchedulerInterruptError>> {
        self.poll_exchange(bell, cx).map(|result| match result? {
            (Command::MaskRun, Reply::Done) => Ok(()),
            _ => Err(SchedulerInterruptError::ForeignReply),
        })
    }

    fn take_reply(&mut self) -> Option<Result<(Command, Reply), SchedulerInterruptError>> {
        let reply = self.client.try_take_if(|reply| {
            if reply.epoch != self.epoch || reply.sequence != self.sequence {
                // Keep the actual message in place and the endpoint poisoned.
                return Err(SchedulerInterruptError::ForeignReply);
            }
            Ok(())
        })?;
        Some(reply.and_then(|reply| {
            self.pending = false;
            reply.reply.map(|value| (reply.command, value))
        }))
    }

    pub(crate) async fn busy(
        &mut self,
        bell: &impl SchedulerDoorbell,
    ) -> Result<BluetoothSchedulerBusyObservation, SchedulerInterruptError> {
        match self.exchange(bell, Command::Busy).await? {
            (_, Reply::Busy(value)) => Ok(value),
            _ => Err(SchedulerInterruptError::ForeignReply),
        }
    }

    pub(crate) async fn service_barrier(
        &mut self,
        bell: &impl SchedulerDoorbell,
    ) -> Result<(), SchedulerInterruptError> {
        match self.exchange(bell, Command::ServiceBarrier).await? {
            (_, Reply::Done) => Ok(()),
            _ => Err(SchedulerInterruptError::ForeignReply),
        }
    }
    pub(crate) async fn lock_modify(
        &mut self,
        bell: &impl SchedulerDoorbell,
    ) -> Result<BluetoothSchedulerLockModifyInterruptObservation, SchedulerInterruptError> {
        match self.exchange(bell, Command::LockModify).await? {
            (_, Reply::LockModify(value)) => Ok(value),
            _ => Err(SchedulerInterruptError::ForeignReply),
        }
    }
    pub(crate) async fn prepare_run(
        &mut self,
        bell: &impl SchedulerDoorbell,
    ) -> Result<BluetoothSchedulerRunInterruptsPrepared, SchedulerInterruptError> {
        match self.exchange(bell, Command::PrepareRun).await? {
            (_, Reply::Prepared(value)) => Ok(value),
            _ => Err(SchedulerInterruptError::ForeignReply),
        }
    }
    pub(crate) async fn acknowledge_cancellation(
        &mut self,
        bell: &impl SchedulerDoorbell,
        indexed: BluetoothSchedulerCancellationIndexed,
    ) -> Result<
        (
            BluetoothSchedulerCancellationIndexed,
            BluetoothSchedulerCancellationSourceAcknowledged,
        ),
        SchedulerInterruptError,
    > {
        match self
            .exchange(bell, Command::AcknowledgeCancellation(indexed))
            .await?
        {
            (Command::AcknowledgeCancellation(indexed), Reply::Cancellation(acknowledged)) => {
                Ok((indexed, acknowledged))
            }
            _ => Err(SchedulerInterruptError::ForeignReply),
        }
    }
    pub(crate) async fn mask_run(
        &mut self,
        bell: &impl SchedulerDoorbell,
    ) -> Result<(), SchedulerInterruptError> {
        self.done(bell, Command::MaskRun).await
    }
    pub(crate) async fn select_progress(
        &mut self,
        bell: &impl SchedulerDoorbell,
    ) -> Result<(), SchedulerInterruptError> {
        self.done(bell, Command::SelectProgress).await
    }
    pub(crate) async fn select_settle(
        &mut self,
        bell: &impl SchedulerDoorbell,
    ) -> Result<(), SchedulerInterruptError> {
        self.done(bell, Command::SelectSettle).await
    }
    async fn done(
        &mut self,
        bell: &impl SchedulerDoorbell,
        command: Command,
    ) -> Result<(), SchedulerInterruptError> {
        match self.exchange(bell, command).await? {
            (_, Reply::Done) => Ok(()),
            _ => Err(SchedulerInterruptError::ForeignReply),
        }
    }
    pub(crate) async fn progress(
        &mut self,
        bell: &impl SchedulerDoorbell,
    ) -> Result<bool, SchedulerInterruptError> {
        self.flag(bell, Command::Progress).await
    }
    pub(crate) async fn settled(
        &mut self,
        bell: &impl SchedulerDoorbell,
    ) -> Result<bool, SchedulerInterruptError> {
        self.flag(bell, Command::Settled).await
    }
    /// The handler's idle sample of the output release.
    pub(crate) async fn sample_idle_output(
        &mut self,
        bell: &impl SchedulerDoorbell,
        admitted: BluetoothOutputReleaseAdmitted,
    ) -> Result<
        Result<BluetoothOutputIdleSampled, BluetoothControllerOutputReleaseError>,
        SchedulerInterruptError,
    > {
        match self
            .exchange(bell, Command::SampleIdleOutput(admitted))
            .await?
        {
            (_, Reply::IdleSampled(sampled)) => Ok(sampled),
            _ => Err(SchedulerInterruptError::ForeignReply),
        }
    }
    /// The handler's mask of the dynamic scheduler sources.
    pub(crate) async fn mask_output(
        &mut self,
        bell: &impl SchedulerDoorbell,
        heads: BluetoothSchedulerHeadsAbsent,
    ) -> Result<
        Result<BluetoothDynamicSourcesMasked, BluetoothControllerOutputReleaseError>,
        SchedulerInterruptError,
    > {
        match self.exchange(bell, Command::MaskOutput(heads)).await? {
            (_, Reply::OutputMasked(masked)) => Ok(masked),
            _ => Err(SchedulerInterruptError::ForeignReply),
        }
    }
    /// The handler's release of the controller output.
    pub(crate) async fn release_output(
        &mut self,
        bell: &impl SchedulerDoorbell,
        run: BluetoothSchedulerRunDisabled,
    ) -> Result<
        Result<BluetoothOutputReleased, BluetoothControllerOutputReleaseError>,
        SchedulerInterruptError,
    > {
        match self.exchange(bell, Command::ReleaseOutput(run)).await? {
            (_, Reply::OutputReleased(released)) => Ok(released),
            _ => Err(SchedulerInterruptError::ForeignReply),
        }
    }
    async fn flag(
        &mut self,
        bell: &impl SchedulerDoorbell,
        command: Command,
    ) -> Result<bool, SchedulerInterruptError> {
        match self.exchange(bell, command).await? {
            (_, Reply::Flag(value)) => Ok(value),
            _ => Err(SchedulerInterruptError::ForeignReply),
        }
    }
}

#[cfg(test)]
mod tests;
