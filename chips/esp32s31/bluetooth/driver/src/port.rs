//! DTM port ownership. P1 only holds queue endpoints; device progress is P2.
//!
//! The device adapter talks to the separately placed controller task. Pool and
//! epoch brands remain in that task and never cross a static queue. One command
//! credit reserves one command and one terminal place through terminal dequeue.

use core::{
    cell::{Cell, RefCell},
    future::{Future, poll_fn},
    marker::PhantomData,
    ops::ControlFlow,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll},
};
use oer_bluetooth_le_port::{
    backend::{OwnedCommand, OwnedRequest, OwnedSession},
    *,
};
use oer_bluetooth_le_radio::{LePhys, LeRadioCapabilities};
use oer_memory::{
    AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, CompactSpscCursor, LocalSpscReceiver,
    LocalSpscSender, OnceValue,
};
use oer_radio_port::*;
use oer_time::{Clock, Duration, Instant, Timer};

mod capacity;
#[cfg(any(target_arch = "riscv32", test))]
mod event;
#[cfg(any(target_arch = "riscv32", test))]
mod guard;
mod lifecycle;
/// Durable command and clock endpoints for the separately retained P2 worker.
pub mod mailbox;
mod owned;
/// Physical DTM session worker, kept in the image's permanently placed P2 future.
#[cfg(target_arch = "riscv32")]
pub mod runner;
#[cfg(test)]
mod tests;
use capacity::Pool;
use owned::OwnedTerminal;

/// Command credit count; the B0 HCI service admits one command at a time.
pub const COMMANDS: usize = 1;
/// Independent FIFO waiter seats, including a cancelled and resumed waiter.
pub const WAITERS: usize = 3;
/// S31's implemented physical test profile; other roles are not advertised.
pub enum DtmCaps {}
impl LeCapabilities for DtmCaps {
    const CAPS: LeRadioCapabilities = LeRadioCapabilities {
        direct_test_mode: true,
        test_phys: LePhys::LE_1M,
        min_tx_power_dbm: oer_esp32s31_hal::bluetooth::engine::LeTxPower::LOWEST_DBM,
        ..LeRadioCapabilities::NONE
    };
}
/// Backend fault. An unknown physical outcome never returns a reusable lease.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Fault {
    Disabled,
    Hardware,
    Accounting,
    Abandoned,
    RejectedTerminal,
    ClockBusy,
    ClockBegin,
    ClockTimeout,
    ClockRead,
    Deadline,
    SchedulerStop,
    SchedulerResume,
    SchedulerSubmit,
    SchedulerWake,
    SchedulerComplete,
    DtmFinish,
    DtmStatus,
    TimerInterrupt,
    TimerBegin,
    TimerStep,
    TimerRearm,
    StopTimeout,
    ResumeTimeout,
    SubmitTimeout,
    WakeTimeout,
}
/// Notification from P1 to P2. The image uses its priority-safe static signal.
pub trait Notify {
    const NEW: Self;
    fn notify(&self);
    fn poll_wait(&self, cx: &mut Context<'_>) -> Poll<()>;
}
/// Independent, generation-keyed controller-clock request endpoint.
#[allow(async_fn_in_trait)]
pub trait ClockClient {
    /// P1 opens bookkeeping before enabling the physical installation.
    fn activate(&mut self) -> Result<(), Fault>;
    /// P1 drains an abandoned clock response before uninstall can join.
    async fn retire(&mut self) -> Result<(), Fault>;
    async fn now(&mut self) -> PortResult<oer_time::RadioInstant<LeRadio>, ClockError, Fault>;
}
/// A complete start description copied only after the request is retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Start {
    Transmit(Transmit),
    Receive(Receive),
}
/// Definite pre-publication refusal or an admitted physical test session.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Started {
    Executed,
    NotExecuted(Cause),
    Aborted,
}
/// P2-only adapter to the permanently placed controller task.
///
/// Each asynchronous operation retains progress in the device/task. Cancellation
/// does not free hardware storage. `quarantine` closes publication immediately;
/// `shutdown` succeeds only after actual stop, actor leave/reset and detachment.
#[allow(async_fn_in_trait)]
pub trait Device {
    type Timer: Timer + Clone;
    type Notify: Notify;
    type Clock: ClockClient;
    fn epoch(&self) -> u64;
    fn timer(&self) -> Self::Timer;
    fn clock(&mut self) -> Self::Clock;
    fn shutdown_bound(&self) -> Duration;
    fn is_running(&self) -> bool;
    /// Complete this installation's actor/Controller/IRQ preparation before
    /// the frontend receives Enabled authority.
    async fn enable(&mut self) -> Result<(), Fault>;
    async fn start(&mut self, config: Start) -> Result<Started, Fault>;
    async fn end(&mut self) -> Result<PacketCount, Fault>;
    async fn progress(&mut self) -> Result<(), Fault>;
    async fn shutdown(&mut self) -> Result<(), Fault>;
    fn quarantine(&mut self);
}

type Path<Id> = Root<Id>;
type Credit<Id> = Permit<'static, CommandCapacity, Child<Path<Id>, 0>>;
type Work<Id, X> = (OwnedRequest<Path<Id>, X, DtmCaps>, Instant);
type Send<'a, T> = AffineSpscSender<'a, T, COMMANDS, CompactSpscCursor>;
type ReceiveQueue<'a, T> = AffineSpscReceiver<'a, T, COMMANDS, CompactSpscCursor>;
type Queue<T> = AffineSpscQueue<T, COMMANDS, CompactSpscCursor>;

/// Registered P2 owner. Current command and failed terminals stay here or in
/// permanent queue storage, never solely in a cancellable driver future.
pub struct Backend<Id: SlotIdentity, D: Device, X = ()> {
    device: Option<D>,
    current: Option<Work<Id, X>>,
    terminal: Option<OwnedTerminal<Id, X>>,
    quarantine: Option<(OwnedCommand<Path<Id>, DtmCaps>, Credit<Id>)>,
    identity: PhantomData<fn(Id) -> Id>,
}
impl<Id: SlotIdentity, D: Device, X> Backend<Id, D, X> {
    pub fn new(device: D) -> Self {
        Self {
            device: Some(device),
            current: None,
            terminal: None,
            quarantine: None,
            identity: PhantomData,
        }
    }
}
/// Normal retirement's actual stopped device, retained by the slot on P2.
pub struct Vacant<D> {
    device: D,
}
impl<D> Vacant<D> {
    /// Reinstall only after reinitializing this returned device and consuming
    /// the same slot's Joined authority. No quarantined path issues this value.
    pub fn into_device(self) -> D {
        self.device
    }
}
struct Shared<N: Notify> {
    notify: N,
    enabled: AtomicBool,
    ready: AtomicBool,
    draining: AtomicBool,
    drained: AtomicBool,
    fault: OnceValue<Fault>,
}
impl<N: Notify> Shared<N> {
    const fn new() -> Self {
        Self {
            notify: N::NEW,
            enabled: AtomicBool::new(false),
            ready: AtomicBool::new(false),
            draining: AtomicBool::new(false),
            drained: AtomicBool::new(true),
            fault: OnceValue::new(),
        }
    }
    // Keep the notification's typed Waker scope out of generated poll frames.
    #[inline(never)]
    fn poison(&self, fault: Fault) {
        let _ = self.fault.set(fault);
        self.enabled.store(false, Ordering::Release);
        self.notify.notify();
    }
}
/// Permanent cross-context queues. No device or HAL register owner is shared.
pub struct Queues<Id: SlotIdentity, D: Device, X> {
    shared: Shared<D::Notify>,
    commands: Queue<Work<Id, X>>,
    events: Queue<OwnedTerminal<Id, X>>,
    failed_request: OnceValue<Work<Id, X>>,
    failed_terminal: OnceValue<OwnedTerminal<Id, X>>,
    resources: Option<(
        [Seat<'static, CommandCapacity, Child<Path<Id>, 0>>; WAITERS],
        OwnedSession<Path<Id>, Idle>,
    )>,
    credits: Option<Pool<Id>>,
    outstanding: Cell<Option<LeOperationId>>,
    timer: Option<D::Timer>,
    clock: Option<D::Clock>,
    epoch: u64,
    bound: Duration,
}
/// P1 endpoint, independent of the P2 device borrow.
pub struct Frontend<'s, Id: SlotIdentity, D: Device, X> {
    shared: &'s Shared<D::Notify>,
    commands: Send<'s, Work<Id, X>>,
    events: ReceiveQueue<'s, OwnedTerminal<Id, X>>,
    failed_request: &'s OnceValue<Work<Id, X>>,
    failed_terminal: &'s OnceValue<OwnedTerminal<Id, X>>,
    resources: &'s mut Option<(
        [Seat<'static, CommandCapacity, Child<Path<Id>, 0>>; WAITERS],
        OwnedSession<Path<Id>, Idle>,
    )>,
    credits: &'s mut Pool<Id>,
    outstanding: &'s mut Cell<Option<LeOperationId>>,
    wakes: SlotWakes<'s, Id>,
    timer: D::Timer,
    clock: &'s mut D::Clock,
    epoch: u64,
    bound: Duration,
}
/// P2 queue endpoints; admitted owners stay durable if run is cancelled.
pub struct Invocation<'s, Id: SlotIdentity, D: Device, X> {
    shared: &'s Shared<D::Notify>,
    commands: ReceiveQueue<'s, Work<Id, X>>,
    events: Send<'s, OwnedTerminal<Id, X>>,
    wakes: SlotWakes<'s, Id>,
}
/// Public body resources, minted once per installation.
pub struct Resources<'p, Id: SlotIdentity> {
    pub seats: [Seat<'p, CommandCapacity, Child<Path<Id>, 0>>; WAITERS],
    pub session: Session<'p, Path<Id>, Idle>,
}
/// Body-local P1 views. Only this private loan can convert owned resources.
pub struct View<'p, Id: SlotIdentity, D: Device, X> {
    shared: &'p Shared<D::Notify>,
    commands: LocalSpscSender<'p, Work<Id, X>, COMMANDS, CompactSpscCursor>,
    events: LocalSpscReceiver<'p, OwnedTerminal<Id, X>, COMMANDS, CompactSpscCursor>,
    failed_request: &'p OnceValue<Work<Id, X>>,
    failed_terminal: &'p OnceValue<OwnedTerminal<Id, X>>,
    credits: &'p Pool<Id>,
    outstanding: &'p Cell<Option<LeOperationId>>,
    wakes: SlotWakes<'p, Id>,
    timer: D::Timer,
    clock: RefCell<&'p mut D::Clock>,
    epoch: u64,
    loan: BodyLoan<'p, Id, Backend<Id, D, X>>,
}
/// Exclusive event consumer with shared synchronous admission views.
pub struct Phase<'a, 'p, Id: SlotIdentity, D: Device, X> {
    view: &'a mut View<'p, Id, D, X>,
}

impl<Id: SlotIdentity, D: Device, X> View<'_, Id, D, X> {
    fn delivered(&self, event: &mut OwnedTerminal<Id, X>) -> Result<(), Fault> {
        if self.outstanding.get() != Some(event.admitted.id) {
            self.shared.poison(Fault::Accounting);
            return Err(Fault::Accounting);
        }
        self.outstanding.set(None);
        if let Some(credit) = event.credit.take() {
            self.credits.release(credit);
        }
        Ok(())
    }
    fn fault(&self) -> Option<Fault> {
        self.shared.fault.get()
    }
}
impl<'p, Id: SlotIdentity, D: Device, X> PortPhase for Phase<'_, 'p, Id, D, X> {
    type Event = Event<'p, Path<Id>, X>;
    type Fault = Fault;
    async fn next_event(&mut self) -> Result<Result<Self::Event, EventsLost>, EventEnd<Fault>> {
        poll_fn(|cx| {
            let v = &self.view;
            v.wakes.register_frontend(cx);
            if let Ok(mut event) = v.events.try_receive() {
                if let Err(cause) = v.delivered(&mut event) {
                    assert!(
                        v.failed_terminal.set(event).is_ok(),
                        "one failed terminal reservation"
                    );
                    return Poll::Ready(Err(EventEnd::Quarantined { cause }));
                }
                return Poll::Ready(Ok(Ok(Event::Terminal(event.lend(&v.loan)))));
            }
            if let Some(cause) = v.fault() {
                return Poll::Ready(Err(EventEnd::Quarantined { cause }));
            }
            if v.shared.drained.load(Ordering::Acquire) {
                return Poll::Ready(Err(EventEnd::Drained {
                    cause: Fault::Disabled,
                }));
            }
            Poll::Pending
        })
        .await
    }
}
impl<'p, Id: SlotIdentity, D: Device, X> LePhase<'p, Path<Id>, X> for Phase<'_, 'p, Id, D, X> {
    type Caps = DtmCaps;
    async fn take<'a>(
        &'a self,
        seat: &'a mut Seat<'p, CommandCapacity, Child<Path<Id>, 0>>,
    ) -> Permit<'p, CommandCapacity, Child<Path<Id>, 0>>
    where
        'p: 'a,
    {
        self.view
            .loan
            .lend_permit(self.view.credits.take(seat.index()).await)
    }
    fn release(&self, credit: Permit<'p, CommandCapacity, Child<Path<Id>, 0>>) {
        self.view
            .credits
            .release(self.view.loan.reclaim_permit(credit));
    }
    fn submit(
        &self,
        request: Request<'p, Path<Id>, X, DtmCaps>,
    ) -> Submission<'p, Path<Id>, X, DtmCaps, Fault> {
        let v = &self.view;
        if let Some(cause) = v.fault() {
            let retained = (request.retain(&v.loan), v.timer.now());
            assert!(
                v.failed_request.set(retained).is_ok(),
                "one unreturned command credit"
            );
            return Err(Poisoned { cause });
        }
        let reason = if request.id.is_backend_reserved() {
            Some(Refusal::ReservedId)
        } else if v.outstanding.get() == Some(request.id) {
            Some(Refusal::DuplicateId)
        } else if request.epoch() != v.epoch {
            Some(Refusal::StaleEpoch)
        } else if !v.shared.enabled.load(Ordering::Acquire)
            || v.shared.draining.load(Ordering::Acquire)
        {
            Some(Refusal::Unavailable)
        } else {
            None
        };
        if let Some(reason) = reason {
            return Ok(Err(Refused { reason, request }));
        }
        let admitted = Admitted {
            id: request.id,
            epoch: request.epoch(),
        };
        let owned = (request.retain(&v.loan), v.timer.now());
        v.outstanding.set(Some(admitted.id));
        if let Err(error) = v.commands.try_send(owned) {
            assert!(
                v.failed_request.set(error.0).is_ok(),
                "failed request owns sole credit"
            );
            v.shared.poison(Fault::Accounting);
            return Err(Poisoned {
                cause: Fault::Accounting,
            });
        }
        v.shared.notify.notify();
        Ok(Ok(admitted))
    }
}
