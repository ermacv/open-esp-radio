//! The port supervisor owns these endpoints; the worker retains the Engine.
//! Cancellation of a device call leaves its request and answer token here.
use super::{ClockClient, Device, Fault, Notify, PacketCount, Start, Started};
use core::{
    future::poll_fn,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll},
};
use oer_bluetooth_le_port::LeInstant;
use oer_memory::{Awaiting, HandlerCell, Poster, RequestSlot};
use oer_radio_port::{ClockError, Poisoned, PortResult};
use oer_sync::PriorityWaker;
use oer_time::{Duration, Timer};

/// One operation on the retained physical installation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Enable,
    Start(Start),
    End,
    Progress,
    Shutdown,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Command {
    pub epoch: u64,
    pub operation: Operation,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Completion {
    Done,
    Started(Started),
    Ended(PacketCount),
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Answer {
    pub epoch: u64,
    pub result: Result<Completion, Fault>,
}
pub type Commands = RequestSlot<Command, Answer>;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockRequest {
    pub epoch: u64,
    pub sequence: u64,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ClockAnswer {
    pub request: ClockRequest,
    pub result: Result<LeInstant, Fault>,
}
pub type ClockRequests = RequestSlot<ClockRequest, ClockAnswer>;

/// A priority-safe wake between P1 and the port supervisor.
pub struct Notification(PriorityWaker);
impl Notify for Notification {
    const NEW: Self = Self(PriorityWaker::new());
    fn notify(&self) {
        self.0.wake();
    }
    fn poll_wait(&self, cx: &mut Context<'_>) -> Poll<()> {
        self.0.register(cx.waker());
        Poll::Pending
    }
}

struct ClockPending {
    request: ClockRequest,
    awaiting: Awaiting<ClockRequest, ClockAnswer>,
    abandoned: bool,
}
/// The independent clock lane remains owned across frontend cancellation.
pub struct ClockState {
    poster: Option<Poster<ClockRequest, ClockAnswer>>,
    pending: Option<ClockPending>,
    epoch: u64,
    sequence: u64,
    retired: bool,
    fault: Option<Fault>,
    cancel: &'static AtomicBool,
}
impl ClockState {
    pub fn new(poster: Poster<ClockRequest, ClockAnswer>, cancel: &'static AtomicBool) -> Self {
        Self {
            poster: Some(poster),
            pending: None,
            epoch: 0,
            sequence: 0,
            retired: true,
            fault: None,
            cancel,
        }
    }
    fn fail(&mut self, fault: Fault) -> Fault {
        self.fault = Some(fault);
        self.cancel.store(true, Ordering::Release);
        fault
    }
    fn take_answer(&mut self, cx: &mut Context<'_>) -> Poll<ClockAnswer> {
        let pending = self.pending.as_mut().expect("one retained clock call");
        let (poster, answer) = core::task::ready!(pending.awaiting.poll_answer(cx));
        let request = pending.request;
        self.poster = Some(poster);
        self.pending = None;
        if answer.request != request {
            let fault = self.fail(Fault::Accounting);
            return Poll::Ready(ClockAnswer {
                request,
                result: Err(fault),
            });
        }
        if let Err(fault) = answer.result {
            self.fail(fault);
        }
        Poll::Ready(answer)
    }
    fn discard_abandoned(&mut self, cx: &mut Context<'_>) -> bool {
        match self.pending.as_ref() {
            None => true,
            Some(pending) if pending.abandoned => self.take_answer(cx).is_ready(),
            Some(_) => false,
        }
    }
    fn activate(&mut self, epoch: u64) -> Result<(), Fault> {
        if self.fault.is_some()
            || self.cancel.load(Ordering::Acquire)
            || !self.retired
            || self.epoch.checked_add(1) != Some(epoch)
            || self.pending.is_some()
            || self.poster.is_none()
        {
            return Err(self.fail(Fault::Accounting));
        }
        self.epoch = epoch;
        self.retired = false;
        Ok(())
    }
    fn poll_retire(&mut self, epoch: u64, cx: &mut Context<'_>) -> Poll<Result<(), Fault>> {
        if self.epoch != epoch || self.pending.as_ref().is_some_and(|p| !p.abandoned) {
            return Poll::Ready(Err(self.fail(Fault::Accounting)));
        }
        if !self.discard_abandoned(cx) {
            return Poll::Pending;
        }
        if let Some(fault) = self.fault {
            return Poll::Ready(Err(fault));
        }
        if self.cancel.load(Ordering::Acquire) {
            return Poll::Ready(Err(Fault::Abandoned));
        }
        self.retired = true;
        Poll::Ready(Ok(()))
    }
    fn begin(&mut self, epoch: u64, cx: &mut Context<'_>) -> Result<ClockRequest, ClockError> {
        if epoch != self.epoch || self.retired {
            return Err(ClockError::Unavailable);
        }
        if !self.discard_abandoned(cx) {
            return Err(ClockError::Unavailable);
        }
        if self.fault.is_some() || self.cancel.load(Ordering::Acquire) {
            return Err(ClockError::Unavailable);
        }
        let sequence = self
            .sequence
            .checked_add(1)
            .ok_or(ClockError::EpochExhausted)?;
        self.sequence = sequence;
        let request = ClockRequest { epoch, sequence };
        let Some(poster) = self.poster.take() else {
            self.fail(Fault::Accounting);
            return Err(ClockError::Unavailable);
        };
        self.pending = Some(ClockPending {
            request,
            awaiting: poster.post(request),
            abandoned: false,
        });
        Ok(request)
    }
}

/// P1-only bookkeeping: initialization, activate, now, cancellation and retire.
/// P2 only constructs an epoch-keyed endpoint and answers the request mailbox;
/// it never enters this cell. No P3 handler reaches it.
pub type ClockStorage = HandlerCell<ClockState>;

pub struct ClockEndpoint {
    state: &'static ClockStorage,
    epoch: u64,
}
struct ClockCall {
    state: &'static ClockStorage,
    request: ClockRequest,
}
impl Drop for ClockCall {
    fn drop(&mut self) {
        let _ = self.state.with(|state| {
            if let Some(pending) = state.pending.as_mut()
                && pending.request == self.request
            {
                pending.abandoned = true;
            }
        });
    }
}
impl ClockClient for ClockEndpoint {
    fn activate(&mut self) -> Result<(), Fault> {
        self.state
            .with(|state| state.activate(self.epoch))
            .ok_or(Fault::Accounting)?
    }
    async fn retire(&mut self) -> Result<(), Fault> {
        poll_fn(|cx| {
            self.state
                .with(|state| state.poll_retire(self.epoch, cx))
                .unwrap_or(Poll::Ready(Err(Fault::Accounting)))
        })
        .await
    }
    async fn now(&mut self) -> PortResult<LeInstant, ClockError, Fault> {
        let Some(begin) = poll_fn(|cx| {
            Poll::Ready(self.state.with(|state| {
                if let Some(cause) = state.fault {
                    return Err(Poisoned { cause });
                }
                let begin = state.begin(self.epoch, cx);
                match state.fault {
                    Some(cause) => Err(Poisoned { cause }),
                    None if state.cancel.load(Ordering::Acquire) => Err(Poisoned {
                        cause: Fault::Abandoned,
                    }),
                    None => Ok(begin),
                }
            }))
        })
        .await
        else {
            return Ok(Err(ClockError::Unavailable));
        };
        let request = match begin? {
            Ok(request) => request,
            Err(error) => return Ok(Err(error)),
        };
        let call = ClockCall {
            state: self.state,
            request,
        };
        let result = poll_fn(|cx| {
            self.state
                .with(|state| {
                    if let Some(cause) = state.fault {
                        return Poll::Ready(Err(Poisoned { cause }));
                    }
                    if state.epoch != request.epoch {
                        return Poll::Ready(Ok(Err(ClockError::Unavailable)));
                    }
                    match state.take_answer(cx) {
                        Poll::Ready(answer) => Poll::Ready(match answer.result {
                            Err(cause) => Err(Poisoned { cause }),
                            Ok(_) if state.cancel.load(Ordering::Acquire) => Err(Poisoned {
                                cause: Fault::Abandoned,
                            }),
                            Ok(sample) => Ok(Ok(sample)),
                        }),
                        Poll::Pending if state.cancel.load(Ordering::Acquire) => {
                            Poll::Ready(Err(Poisoned {
                                cause: Fault::Abandoned,
                            }))
                        }
                        Poll::Pending => Poll::Pending,
                    }
                })
                .unwrap_or(Poll::Ready(Ok(Err(ClockError::Unavailable))))
        })
        .await;
        drop(call);
        result
    }
}

struct Pending {
    command: Command,
    awaiting: Awaiting<Command, Answer>,
}
/// Its durable transaction fields outlive every borrowed async device call.
pub struct MailboxDevice<T> {
    timer: T,
    poster: Option<Poster<Command, Answer>>,
    pending: Option<Pending>,
    clock: &'static ClockStorage,
    cancel: &'static AtomicBool,
    epoch: u64,
    bound: Duration,
    running: bool,
    stopped: bool,
    fault: Option<Fault>,
}
impl<T> MailboxDevice<T> {
    pub fn new(
        timer: T,
        poster: Poster<Command, Answer>,
        clock: &'static ClockStorage,
        cancel: &'static AtomicBool,
        bound: Duration,
    ) -> Self {
        Self {
            timer,
            poster: Some(poster),
            pending: None,
            clock,
            cancel,
            epoch: 1,
            bound,
            running: false,
            stopped: false,
            fault: None,
        }
    }
    /// Called only on the device returned by the slot's normal Joined path.
    /// A fault or an unfinished request cannot advance the physical epoch.
    pub fn reinitialize(mut self) -> Result<Self, Self> {
        if !self.stopped
            || self.pending.is_some()
            || self.fault.is_some()
            || self.cancel.load(Ordering::Acquire)
        {
            return Err(self);
        }
        let Some(epoch) = self.epoch.checked_add(1) else {
            self.fault = Some(Fault::Accounting);
            return Err(self);
        };
        self.epoch = epoch;
        self.stopped = false;
        Ok(self)
    }
    fn fail(&mut self, fault: Fault) -> Fault {
        self.fault = Some(fault);
        self.cancel.store(true, Ordering::Release);
        fault
    }
    async fn exchange(&mut self, operation: Operation) -> Result<Completion, Fault> {
        if let Some(fault) = self.fault {
            return Err(fault);
        }
        if self.cancel.load(Ordering::Acquire) {
            return Err(self.fail(Fault::Abandoned));
        }
        let command = Command {
            epoch: self.epoch,
            operation,
        };
        if let Some(pending) = &self.pending {
            if pending.command != command {
                return Err(self.fail(Fault::Accounting));
            }
        } else {
            let Some(poster) = self.poster.take() else {
                return Err(self.fail(Fault::Accounting));
            };
            self.pending = Some(Pending {
                command,
                awaiting: poster.post(command),
            });
        }
        let answer = poll_fn(|cx| {
            let pending = self.pending.as_mut().expect("retained device transaction");
            pending.awaiting.poll_answer(cx)
        })
        .await;
        self.poster = Some(answer.0);
        self.pending = None;
        if answer.1.epoch != self.epoch {
            return Err(self.fail(Fault::Accounting));
        }
        answer.1.result.map_err(|fault| self.fail(fault))
    }
    async fn done(&mut self, operation: Operation) -> Result<(), Fault> {
        match self.exchange(operation).await? {
            Completion::Done => Ok(()),
            _ => Err(self.fail(Fault::Accounting)),
        }
    }
}
impl<T: Timer + Clone> Device for MailboxDevice<T> {
    type Timer = T;
    type Notify = Notification;
    type Clock = ClockEndpoint;
    fn epoch(&self) -> u64 {
        self.epoch
    }
    fn timer(&self) -> T {
        self.timer.clone()
    }
    fn clock(&mut self) -> ClockEndpoint {
        ClockEndpoint {
            state: self.clock,
            epoch: self.epoch,
        }
    }
    fn shutdown_bound(&self) -> Duration {
        self.bound
    }
    fn is_running(&self) -> bool {
        self.running
    }
    async fn enable(&mut self) -> Result<(), Fault> {
        self.done(Operation::Enable).await
    }
    async fn start(&mut self, config: Start) -> Result<Started, Fault> {
        match self.exchange(Operation::Start(config)).await? {
            Completion::Started(started) => {
                if started == Started::Executed {
                    self.running = true;
                }
                Ok(started)
            }
            _ => Err(self.fail(Fault::Accounting)),
        }
    }
    async fn end(&mut self) -> Result<PacketCount, Fault> {
        match self.exchange(Operation::End).await? {
            Completion::Ended(count) => {
                self.running = false;
                Ok(count)
            }
            _ => Err(self.fail(Fault::Accounting)),
        }
    }
    async fn progress(&mut self) -> Result<(), Fault> {
        self.done(Operation::Progress).await
    }
    async fn shutdown(&mut self) -> Result<(), Fault> {
        self.done(Operation::Shutdown).await?;
        self.stopped = true;
        self.running = false;
        Ok(())
    }
    fn quarantine(&mut self) {
        self.fail(Fault::Abandoned);
    }
}

#[cfg(test)]
mod tests;
