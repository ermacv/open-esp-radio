//! One affine request/reply slot between the scheduler task and P3.
//!
//! The slot owns each request until P3 consumes it, then owns the reply until
//! P2 takes it. Only the readiness notification coalesces. Neither context
//! calls the other or borrows its registers, and no waker crosses into P3.

use core::{
    cell::UnsafeCell,
    future::poll_fn,
    mem::MaybeUninit,
    sync::atomic::{AtomicU32, Ordering},
    task::{Context, Poll},
};

use crate::bluetooth::{
    BluetoothSchedulerBusyObservation, BluetoothSchedulerCancellationIndexed,
    BluetoothSchedulerCancellationSourceAcknowledged,
    BluetoothSchedulerLockModifyInterruptObservation, BluetoothSchedulerRunInterruptsPrepared,
    InterruptRegistersOwner,
};

const FREE: u32 = 0;
const EMPTY: u32 = 1;
const REQUEST: u32 = 2;
const EXECUTING: u32 = 3;
const RESPONSE: u32 = 4;

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
}

enum Command {
    Busy,
    LockModify,
    PrepareRun,
    AcknowledgeCancellation(BluetoothSchedulerCancellationIndexed),
    MaskRun,
    SelectProgress,
    Progress,
    SelectSettle,
    Settled,
}

enum Reply {
    Busy(BluetoothSchedulerBusyObservation),
    LockModify(BluetoothSchedulerLockModifyInterruptObservation),
    Prepared(BluetoothSchedulerRunInterruptsPrepared),
    Cancellation(BluetoothSchedulerCancellationSourceAcknowledged),
    Done,
    Flag(bool),
}

// Initialized by P2 before REQUEST. P3 borrows this one message in place,
// writes only the reply, then publishes RESPONSE. In particular the indexed
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
    state: AtomicU32,
    sequence: AtomicU32,
    payload: UnsafeCell<MaybeUninit<Exchange>>,
}

#[allow(
    unsafe_code,
    reason = "the atomic state transfers exclusive payload ownership"
)]
// SAFETY: EMPTY is exclusively P2's; REQUEST transfers the initialized
// payload to the unique P3 endpoint; RESPONSE transfers it back. Release
// stores and Acquire loads cover every payload access. Endpoints cannot be
// cloned. Reinstallation consumes both quiet endpoints of the old epoch.
// No endpoint reads EXECUTING.
unsafe impl Sync for SchedulerInterruptChannel {}

impl Default for SchedulerInterruptChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl SchedulerInterruptChannel {
    pub const fn new() -> Self {
        Self {
            state: AtomicU32::new(FREE),
            sequence: AtomicU32::new(0),
            payload: UnsafeCell::new(MaybeUninit::uninit()),
        }
    }

    pub(crate) fn install(
        &'static self,
        epoch: u32,
    ) -> Result<(SchedulerTaskHalf, SchedulerInterruptHalf), SchedulerInterruptError> {
        self.state
            .compare_exchange(FREE, EMPTY, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| SchedulerInterruptError::Occupied)?;
        let sequence = self.sequence.load(Ordering::Relaxed);
        Ok((
            SchedulerTaskHalf {
                channel: self,
                epoch,
                sequence,
                pending: false,
            },
            SchedulerInterruptHalf {
                channel: self,
                epoch,
                sequence,
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
}

pub(crate) struct SchedulerTaskHalf {
    channel: &'static SchedulerInterruptChannel,
    epoch: u32,
    sequence: u32,
    pending: bool,
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
    ) -> bool {
        let clock = SchedulerInterruptClock::new(clock);
        self.service_with(|command| {
            let settle = crate::settle::Settle::new(super::DIAGNOSTIC_SETTLE, &clock);
            let unsettled = |_| SchedulerInterruptError::DiagnosticUnsettled;
            Ok(match command {
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
                Command::PrepareRun => {
                    Reply::Prepared(interrupts.prepare_scheduler_run_interrupts())
                }
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
            })
        })
    }

    #[allow(
        unsafe_code,
        reason = "the unique P3 endpoint consumes only a Release-published request"
    )]
    fn service_with(
        &mut self,
        execute: impl FnOnce(&Command) -> Result<Reply, SchedulerInterruptError>,
    ) -> bool {
        if self.channel.state.load(Ordering::Acquire) != REQUEST {
            return false;
        }
        self.channel.state.store(EXECUTING, Ordering::Relaxed);
        // SAFETY: REQUEST transferred the slot to this unique endpoint. P2
        // cannot touch it until RESPONSE, and no other P3 endpoint exists.
        let request = unsafe { (&mut *self.channel.payload.get()).assume_init_mut() };
        let next = self.sequence.checked_add(1);
        request.reply = if request.epoch != self.epoch || next != Some(request.sequence) {
            Err(SchedulerInterruptError::ForeignReply)
        } else {
            self.sequence = request.sequence;
            execute(&request.command)
        };
        self.channel.state.store(RESPONSE, Ordering::Release);
        true
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
        if !self.is_quiet() || self.channel.state.load(Ordering::Acquire) != EMPTY {
            return Err((SchedulerInterruptError::Abandoned, self, interrupts));
        }
        self.channel.state.store(FREE, Ordering::Release);
        Ok(())
    }

    pub(crate) const fn is_quiet(&self) -> bool {
        !self.pending
    }

    #[allow(
        unsafe_code,
        reason = "the unique P2 endpoint owns EMPTY and consumes only RESPONSE"
    )]
    async fn exchange(
        &mut self,
        bell: &impl SchedulerDoorbell,
        command: Command,
    ) -> Result<(Command, Reply), SchedulerInterruptError> {
        if self.pending {
            return Err(SchedulerInterruptError::Abandoned);
        }
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(SchedulerInterruptError::SequenceExhausted)?;
        if self.channel.state.load(Ordering::Acquire) != EMPTY {
            return Err(SchedulerInterruptError::Occupied);
        }
        self.sequence = sequence;
        self.channel.sequence.store(sequence, Ordering::Relaxed);
        self.pending = true;
        // SAFETY: this unique endpoint owns EMPTY; P3 accesses only REQUEST.
        unsafe {
            (*self.channel.payload.get()).write(Exchange {
                epoch: self.epoch,
                sequence,
                command,
                // P3 replaces this before RESPONSE; P2 never interprets the
                // request's initial reply as a hardware observation.
                reply: Err(SchedulerInterruptError::Abandoned),
            });
        }
        self.channel.state.store(REQUEST, Ordering::Release);
        bell.ring();
        poll_fn(|cx| {
            if let Some(reply) = self.take_reply() {
                return Poll::Ready(reply);
            }
            if let Poll::Ready(Err(error)) = bell.poll_reply(cx) {
                return Poll::Ready(Err(error));
            }
            // Check after registering: P3 can reply between the first load
            // and registration. The static signal also retains that wake.
            match self.take_reply() {
                Some(reply) => Poll::Ready(reply),
                None => Poll::Pending,
            }
        })
        .await
    }

    #[allow(
        unsafe_code,
        reason = "Acquire of RESPONSE grants the unique P2 endpoint the payload"
    )]
    fn take_reply(&mut self) -> Option<Result<(Command, Reply), SchedulerInterruptError>> {
        if self.channel.state.load(Ordering::Acquire) != RESPONSE {
            return None;
        }
        // SAFETY: P3 Release-published RESPONSE after writing its reply. It
        // cannot touch the initialized message before P2 opens a new REQUEST.
        let reply = unsafe { (&*self.channel.payload.get()).assume_init_ref() };
        if reply.epoch != self.epoch || reply.sequence != self.sequence {
            // Keep the actual message in place and the endpoint poisoned.
            // Repeated observation cannot duplicate/drop its affine command.
            return Some(Err(SchedulerInterruptError::ForeignReply));
        }
        // SAFETY: this unique P2 endpoint validated the message above and
        // consumes it once; EMPTY grants neither P3 nor another reader access.
        let reply = unsafe { (&*self.channel.payload.get()).assume_init_read() };
        self.pending = false;
        self.channel.state.store(EMPTY, Ordering::Release);
        Some(reply.reply.map(|value| (reply.command, value)))
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
