//! The one handler of the Bluetooth interrupts and the gate through which
//! the task hands it each epoch's owners.
//!
//! One interrupt table entry binds [`BluetoothInterrupts`] once, at start:
//! its sources are the controller bank's primary (124) and NRT (133)
//! sources, the modem low-power timer (127) and the radio doorbell
//! (`FROM_CPU_INTR3`), all on the entry's one line. The part holds the
//! doorbell's interrupt part and two gates, one for the bank and one for the
//! timer; the line's handler calls [`BluetoothInterrupts::service`], which
//! looks at what fired by each peripheral's own status and touches a
//! register owner only while its gate holds it:
//!
//! 1. lower the doorbell line, apply the task's unarmed timer plan while
//!    retaining its register owner, then serve the task's gate request
//!    ([`BluetoothInterruptGate`]);
//! 2. serve one request of the scheduler channel;
//! 3. sample and acknowledge the bank's primary status, then its NRT
//!    snapshots, publishing the primary capture to the scheduler worker;
//! 4. step the timer.
//!
//! A clear status writes nothing. The task opens the gates with the epoch's
//! owners (the bank still unprepared, the timer initialized but not
//! started): the handler prepares the controller output and starts the
//! timer, so no source is armed while the task holds its owner. The bank
//! leaves only released ([`BluetoothInterruptGate::close`]) and the timer
//! only stopped and after the Controller reset
//! ([`BluetoothInterruptGate::take_timer`]). The route stays; nothing is
//! masked at the interrupt controller.
//!
//! The task and the handler share no lock: the gate's requests, replies and
//! value-only timer snapshots and unarmed plans cross through one-slot queues.
//! The timer owner stays in the handler while P2 processes its queue; only
//! the handler samples with rollover side effects and programs compare.
//! Repeated entries acknowledge and retain new timer facts with that owner,
//! without overwriting the task's in-flight snapshot.
//! The doorbell announces a request and replies raise the task's static signal.

use core::{
    future::poll_fn,
    sync::atomic::{AtomicBool, Ordering},
    task::Poll,
};

use oer_esp32s31_hal::bluetooth::{
    self,
    engine::{
        ControllerMemory, EngineResetTimerPending, scheduler::interrupt::SchedulerInterruptHalf,
    },
};
use oer_esp32s31_hal::platform::FromCpu3Irq;
use oer_interrupt_table::{
    Bound,
    signal::{IrqSignal, SignalExecutor},
};
use oer_memory::{AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, CompactSpscCursor};

use super::{PrimaryInterruptCapture, RadioDoorbell};
use crate::modem_lp_timer_queue::ModemLpTimerStableInterruptStep;
use crate::runtime_resources::ControllerEventCells;

#[cfg(test)]
mod tests;

type Queue<T> = AffineSpscQueue<T, 1, CompactSpscCursor>;
type Sender<T> = AffineSpscSender<'static, T, 1, CompactSpscCursor>;
type Receiver<T> = AffineSpscReceiver<'static, T, 1, CompactSpscCursor>;

/// The lowering half of the radio doorbell's software interrupt line, which
/// the image hands the handler.
pub trait DoorbellLine {
    /// Lower the line before taking the requests its raises announced: a
    /// ring after this raises the line again and is not lost.
    fn acknowledge(&mut self);
}

/// The owners the gates move and the register steps the handler runs on
/// them: [`ChipGate`]'s are the HAL's.
pub trait GateHardware: 'static {
    /// The doorbell's interrupt part, which the handler holds.
    type Doorbell: Send + 'static;
    /// The bank before its output is prepared.
    type Setup: Send + 'static;
    /// The bank as the handler holds it for the epoch.
    type Bank: Send + 'static;
    /// The released bank.
    type Released: Send + 'static;
    /// The proof that the bank's output is released.
    type ReleaseProof: Send + 'static;
    /// The handler's endpoint of the scheduler channel.
    type Scheduler: Send + 'static;
    /// The timer, initialized but not started.
    type TimerInitialized: Send + 'static;
    /// The timer ready for the handler's step.
    type Timer: Send + 'static;
    /// Register owner retained in the handler while the task plans.
    type TimerPending: Send + 'static;
    /// Value-only software work published to the task.
    type TimerWork: Send + 'static;
    /// Unarmed software plan returned by the task.
    type TimerPlan: Send + 'static;
    /// The proof that the stopped timer's compare is disabled.
    type CompareDisabled: Send + 'static;

    /// Prepare the controller output of `setup` for the epoch.
    fn open_bank(setup: Self::Setup) -> Self::Bank;
    /// Start the runtime timer of `timer` for the epoch.
    fn start_timer(timer: Self::TimerInitialized) -> Self::Timer;
    /// Serve at most one request of the scheduler channel; whether one was.
    fn serve_request<C: oer_time::Clock>(
        scheduler: &mut Self::Scheduler,
        bank: &mut Self::Bank,
        clock: &C,
    ) -> bool;
    /// Sample and acknowledge the primary status.
    fn capture_primary<C: oer_time::Clock>(
        bank: &mut Self::Bank,
        clock: &C,
    ) -> PrimaryInterruptCapture;
    /// Sample and acknowledge the NRT snapshots; whether any was set.
    fn capture_nrt(bank: &mut Self::Bank) -> bool;
    /// One finite register step of the timer.
    fn step_timer(
        timer: Self::Timer,
    ) -> TimerEntry<Self::Timer, Self::TimerPending, Self::TimerWork>;
    /// Apply a plan with the retained register owner; only the handler arms.
    fn apply_timer_plan(
        timer: Self::TimerPending,
        plan: Self::TimerPlan,
    ) -> TimerEntry<Self::Timer, Self::TimerPending, Self::TimerWork>;
    /// Acknowledge and retain new facts while the task processes its snapshot.
    fn observe_timer(timer: &mut Self::TimerPending);
    /// Disable the compare of the drained timer.
    fn stop_timer(timer: &mut Self::Timer) -> Self::CompareDisabled;
    /// The released bank, or the bank back when its output is not released.
    ///
    /// # Errors
    ///
    /// The output is not released.
    fn close_bank(
        bank: Self::Bank,
        released: Self::ReleaseProof,
    ) -> Result<Self::Released, Self::Bank>;
}

/// One register step of the timer.
pub enum TimerEntry<Timer, Pending, Work> {
    /// The status was clear; nothing was written.
    Spurious(Timer),
    /// The handler's registers were acknowledged without software work.
    Rearmed(Timer),
    /// Software work is pending in the task.
    SoftwarePending(Pending, Work),
}

/// Why the gate refused a request or could not exchange it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GateError {
    /// The gates are open already.
    AlreadyOpen,
    /// The bank's gate is closed.
    NotOpen,
    /// The bank's output is not released; the bank stays with its handler.
    OutputNotReleased,
    /// The timer awaits a software plan or is stopped already.
    TimerNotReady,
    /// The timer is not stopped.
    TimerNotStopped,
    /// An earlier request was abandoned before its reply: the gate stays
    /// closed to the task.
    Abandoned,
    /// The request slot is occupied.
    Occupied,
    /// The task runs outside the reply signal's executor.
    ForeignContext,
    /// The reply did not answer the request.
    ForeignReply,
}

/// What the task asks the handler for, one request at a time.
pub(crate) enum Request<H: GateHardware> {
    Open {
        setup: H::Setup,
        scheduler: H::Scheduler,
        timer: H::TimerInitialized,
    },
    StopTimer,
    Close(H::ReleaseProof),
    TakeTimer,
}

/// The handler's answer to one request.
pub(crate) enum Reply<H: GateHardware> {
    Opened,
    OpenRefused {
        setup: H::Setup,
        scheduler: H::Scheduler,
        timer: H::TimerInitialized,
    },
    TimerStopped(H::CompareDisabled),
    Closed {
        output: H::Released,
        scheduler: H::Scheduler,
    },
    Timer(H::Timer),
    Refused(GateError),
}

/// The bank's gate.
enum Bank<H: GateHardware> {
    Closed,
    Open {
        registers: H::Bank,
        scheduler: H::Scheduler,
    },
}

/// The timer's gate.
enum Timer<H: GateHardware> {
    /// No owner: closed, or returned after the Controller reset.
    Vacant,
    Ready(H::Timer),
    AwaitingPlan(H::TimerPending),
    /// Drained and its compare disabled, until the Controller reset.
    Stopped(H::Timer),
}

/// The static queues of the gate, one per image.
pub struct BluetoothInterruptStorage<H: GateHardware> {
    requests: Queue<Request<H>>,
    replies: Queue<Reply<H>>,
    plans: Queue<H::TimerPlan>,
    pending: Queue<H::TimerWork>,
}

impl<H: GateHardware> Default for BluetoothInterruptStorage<H> {
    fn default() -> Self {
        Self::new()
    }
}

/// The static signals and cells the handler raises: the reply signal of the
/// task's executor, the epoch's event cells and the image's fault flag.
pub struct BluetoothInterruptSignals<X: SignalExecutor + 'static> {
    /// The reply signal, shared with the scheduler channel's replies.
    pub reply: &'static IrqSignal<X>,
    /// The scheduler and source-127 worker wakes.
    pub cells: &'static ControllerEventCells<X>,
    /// Raised when the handler had to retain an owner it could not hand
    /// on.
    pub fault: &'static AtomicBool,
}

impl<H: GateHardware> BluetoothInterruptStorage<H> {
    pub const fn new() -> Self {
        Self {
            requests: Queue::new(),
            replies: Queue::new(),
            plans: Queue::new(),
            pending: Queue::new(),
        }
    }

    /// Split the storage into the handler's part, which the interrupt
    /// table's entry binds once, and the task's ends, which
    /// [`BluetoothInterruptEnds::attach`] completes with the doorbell's
    /// ringing half once the part is bound. `irq` is the doorbell's
    /// interrupt part and `line` its lowering half.
    pub fn split<X: SignalExecutor + 'static, B>(
        &'static mut self,
        irq: H::Doorbell,
        line: B,
        signals: BluetoothInterruptSignals<X>,
    ) -> (BluetoothInterruptEnds<H, X>, BluetoothInterruptsOf<H, X, B>) {
        let (requests, requested) = self.requests.split();
        let (replying, replies) = self.replies.split();
        let (plans, planned) = self.plans.split();
        let (pending_out, pending) = self.pending.split();
        (
            BluetoothInterruptEnds {
                requests,
                replies,
                plans,
                pending,
                reply: signals.reply,
            },
            BluetoothInterruptsOf {
                _irq: irq,
                line,
                bank: Bank::Closed,
                timer: Timer::Vacant,
                requests: requested,
                replies: replying,
                plans: planned,
                pending: pending_out,
                signals,
                retained: Retained {
                    reply: None,
                    plan: None,
                    pending: None,
                    work: None,
                },
            },
        )
    }
}

/// The task's ends of the gate before the doorbell rings.
pub struct BluetoothInterruptEnds<H: GateHardware, X: SignalExecutor + 'static> {
    requests: Sender<Request<H>>,
    replies: Receiver<Reply<H>>,
    plans: Sender<H::TimerPlan>,
    pending: Receiver<H::TimerWork>,
    reply: &'static IrqSignal<X>,
}

impl<H: GateHardware, X: SignalExecutor + 'static> BluetoothInterruptEnds<H, X> {
    fn with_ring<R: RadioDoorbell + Clone>(
        self,
        ring: R,
    ) -> (BluetoothInterruptGate<H, X, R>, ModemLpTimerPort<H, R>) {
        (
            BluetoothInterruptGate {
                requests: self.requests,
                replies: self.replies,
                ring: ring.clone(),
                reply: self.reply,
                awaiting: false,
            },
            ModemLpTimerPort {
                plans: self.plans,
                pending: self.pending,
                ring,
            },
        )
    }
}

/// Owners the handler could not hand on, kept until the chip resets; each
/// raises the fault flag.
struct Retained<H: GateHardware> {
    reply: Option<Reply<H>>,
    plan: Option<H::TimerPlan>,
    pending: Option<H::TimerPending>,
    work: Option<H::TimerWork>,
}

/// The handler's part: the doorbell's interrupt part and lowering half, the
/// bank's and the timer's gates and the handler's ends of the gate's queues.
pub struct BluetoothInterruptsOf<H: GateHardware, X: SignalExecutor + 'static, B> {
    _irq: H::Doorbell,
    line: B,
    bank: Bank<H>,
    timer: Timer<H>,
    requests: Receiver<Request<H>>,
    replies: Sender<Reply<H>>,
    plans: Receiver<H::TimerPlan>,
    pending: Sender<H::TimerWork>,
    signals: BluetoothInterruptSignals<X>,
    retained: Retained<H>,
}

impl<H: GateHardware, X: SignalExecutor + 'static, B: DoorbellLine> BluetoothInterruptsOf<H, X, B> {
    /// Serve one entry of the line: see the module documentation for the
    /// order. Finite: one request of each kind, one sample of each status,
    /// one timer step; diagnostic settles are bounded on `clock`.
    pub fn service<C: oer_time::Clock>(&mut self, clock: &C) {
        self.line.acknowledge();
        self.apply_timer_plan();
        self.serve_gate_request();
        if let Bank::Open {
            registers,
            scheduler,
        } = &mut self.bank
        {
            if H::serve_request(scheduler, registers, clock) {
                self.signals.reply.raise(1);
            }
            let capture = H::capture_primary(registers, clock);
            let _ = self
                .signals
                .cells
                .scheduler_wake()
                .publish_from_interrupt(capture);
            let _ = H::capture_nrt(registers);
        }
        self.step_timer();
    }

    /// Whether the bank's gate is open.
    pub const fn is_open(&self) -> bool {
        matches!(self.bank, Bank::Open { .. })
    }

    fn fault(&self) {
        self.signals.fault.store(true, Ordering::Release);
    }

    fn apply_timer_plan(&mut self) {
        let Ok(plan) = self.plans.try_receive() else {
            return;
        };
        match core::mem::replace(&mut self.timer, Timer::Vacant) {
            Timer::AwaitingPlan(timer) => {
                self.accept_timer_entry(H::apply_timer_plan(timer, plan));
            }
            other => {
                self.timer = other;
                self.retained.plan = Some(plan);
                self.fault();
            }
        }
    }

    fn serve_gate_request(&mut self) {
        let Ok(request) = self.requests.try_receive() else {
            return;
        };
        let reply = match request {
            Request::Open {
                setup,
                scheduler,
                timer,
            } => self.open(setup, scheduler, timer),
            Request::StopTimer => match core::mem::replace(&mut self.timer, Timer::Vacant) {
                Timer::Ready(mut timer) => {
                    let disabled = H::stop_timer(&mut timer);
                    self.timer = Timer::Stopped(timer);
                    Reply::TimerStopped(disabled)
                }
                other => {
                    self.timer = other;
                    Reply::Refused(GateError::TimerNotReady)
                }
            },
            Request::Close(released) => match core::mem::replace(&mut self.bank, Bank::Closed) {
                Bank::Open {
                    registers,
                    scheduler,
                } => match H::close_bank(registers, released) {
                    Ok(output) => Reply::Closed { output, scheduler },
                    Err(registers) => {
                        self.bank = Bank::Open {
                            registers,
                            scheduler,
                        };
                        Reply::Refused(GateError::OutputNotReleased)
                    }
                },
                Bank::Closed => Reply::Refused(GateError::NotOpen),
            },
            Request::TakeTimer => match core::mem::replace(&mut self.timer, Timer::Vacant) {
                Timer::Stopped(timer) => Reply::Timer(timer),
                other => {
                    self.timer = other;
                    Reply::Refused(GateError::TimerNotStopped)
                }
            },
        };
        if let Err(rejected) = self.replies.try_send(reply) {
            // The task sends one request per reply it took, so the slot is
            // free; a full one is a broken protocol.
            self.retained.reply = Some(rejected.0);
            self.fault();
        }
        self.signals.reply.raise(1);
    }

    fn open(
        &mut self,
        setup: H::Setup,
        scheduler: H::Scheduler,
        timer: H::TimerInitialized,
    ) -> Reply<H> {
        if !matches!(self.bank, Bank::Closed) || !matches!(self.timer, Timer::Vacant) {
            return Reply::OpenRefused {
                setup,
                scheduler,
                timer,
            };
        }
        // The vendor order: the controller output, then the runtime timer.
        self.bank = Bank::Open {
            registers: H::open_bank(setup),
            scheduler,
        };
        self.timer = Timer::Ready(H::start_timer(timer));
        Reply::Opened
    }

    fn accept_timer_entry(&mut self, entry: TimerEntry<H::Timer, H::TimerPending, H::TimerWork>) {
        let step = match entry {
            TimerEntry::Spurious(timer) => {
                self.timer = Timer::Ready(timer);
                ModemLpTimerStableInterruptStep::Spurious
            }
            TimerEntry::Rearmed(timer) => {
                self.timer = Timer::Ready(timer);
                ModemLpTimerStableInterruptStep::Rearmed
            }
            TimerEntry::SoftwarePending(owner, work) => {
                self.timer = Timer::AwaitingPlan(owner);
                if let Err(rejected) = self.pending.try_send(work) {
                    self.retained.work = Some(rejected.0);
                    self.fault();
                }
                ModemLpTimerStableInterruptStep::SoftwarePending
            }
        };
        let _ = step.publish(self.signals.cells.timer_wake());
    }

    fn step_timer(&mut self) {
        match core::mem::replace(&mut self.timer, Timer::Vacant) {
            Timer::Ready(timer) => {
                self.accept_timer_entry(H::step_timer(timer));
            }
            Timer::Stopped(timer) => match H::step_timer(timer) {
                TimerEntry::Spurious(timer) | TimerEntry::Rearmed(timer) => {
                    self.timer = Timer::Stopped(timer);
                }
                TimerEntry::SoftwarePending(pending, _work) => {
                    // The drained task takes no more work: keep the owner.
                    self.retained.pending = Some(pending);
                    self.fault();
                }
            },
            Timer::AwaitingPlan(mut timer) => {
                H::observe_timer(&mut timer);
                self.timer = Timer::AwaitingPlan(timer);
            }
            Timer::Vacant => {}
        }
    }
}

/// The task's end of the gate: one request at a time, each announced by the
/// doorbell, its reply awaited on the reply signal.
pub struct BluetoothInterruptGate<H: GateHardware, X: SignalExecutor + 'static, R> {
    requests: Sender<Request<H>>,
    replies: Receiver<Reply<H>>,
    ring: R,
    reply: &'static IrqSignal<X>,
    awaiting: bool,
}

impl<H: GateHardware, X: SignalExecutor + 'static, R: RadioDoorbell>
    BluetoothInterruptGate<H, X, R>
{
    /// Send `request`, ring the doorbell and await the handler's reply.
    ///
    /// # Errors
    ///
    /// An earlier request was abandoned, the slot is occupied or the task
    /// runs outside the reply signal's executor.
    pub(crate) async fn exchange(&mut self, request: Request<H>) -> Result<Reply<H>, GateError> {
        if self.awaiting {
            return Err(GateError::Abandoned);
        }
        self.requests
            .try_send(request)
            .map_err(|_| GateError::Occupied)?;
        self.awaiting = true;
        self.ring.ring();
        let replies = &self.replies;
        let signal = self.reply;
        let reply = poll_fn(|cx| {
            if let Ok(reply) = replies.try_receive() {
                return Poll::Ready(Ok(reply));
            }
            if let Poll::Ready(Err(_)) = signal.poll_take(cx) {
                return Poll::Ready(Err(GateError::ForeignContext));
            }
            // The handler may reply between the first look and the waker's
            // registration.
            match replies.try_receive() {
                Ok(reply) => Poll::Ready(Ok(reply)),
                Err(_) => Poll::Pending,
            }
        })
        .await?;
        self.awaiting = false;
        Ok(reply)
    }

    /// Open both gates with the epoch's owners: the handler prepares the
    /// bank's output and starts the timer.
    ///
    /// # Errors
    ///
    /// The gates are open already (the owners come back) or the exchange
    /// failed.
    #[allow(clippy::type_complexity, reason = "the refusal returns each owner")]
    pub(crate) async fn open_with(
        &mut self,
        setup: H::Setup,
        scheduler: H::Scheduler,
        timer: H::TimerInitialized,
    ) -> Result<
        (),
        (
            GateError,
            Option<(H::Setup, H::Scheduler, H::TimerInitialized)>,
        ),
    > {
        match self
            .exchange(Request::Open {
                setup,
                scheduler,
                timer,
            })
            .await
        {
            Ok(Reply::Opened) => Ok(()),
            Ok(Reply::OpenRefused {
                setup,
                scheduler,
                timer,
            }) => Err((GateError::AlreadyOpen, Some((setup, scheduler, timer)))),
            Ok(Reply::Refused(error)) => Err((error, None)),
            Ok(_) => Err((GateError::ForeignReply, None)),
            Err(error) => Err((error, None)),
        }
    }

    /// Disable the compare of the drained timer, which stays with its
    /// handler until the Controller reset.
    ///
    /// # Errors
    ///
    /// The timer is not with its handler, or the exchange failed.
    pub async fn stop_timer(&mut self) -> Result<H::CompareDisabled, GateError> {
        match self.exchange(Request::StopTimer).await? {
            Reply::TimerStopped(disabled) => Ok(disabled),
            Reply::Refused(error) => Err(error),
            _ => Err(GateError::ForeignReply),
        }
    }

    /// Close the bank's gate: the released bank and the handler's endpoint
    /// of the scheduler channel leave the handler.
    ///
    /// # Errors
    ///
    /// The gate is closed, the output is not released (the bank stays) or
    /// the exchange failed.
    pub async fn close(
        &mut self,
        released: H::ReleaseProof,
    ) -> Result<(H::Released, H::Scheduler), GateError> {
        match self.exchange(Request::Close(released)).await? {
            Reply::Closed { output, scheduler } => Ok((output, scheduler)),
            Reply::Refused(error) => Err(error),
            _ => Err(GateError::ForeignReply),
        }
    }

    pub(crate) async fn take_stopped_timer(&mut self) -> Result<H::Timer, GateError> {
        match self.exchange(Request::TakeTimer).await? {
            Reply::Timer(timer) => Ok(timer),
            Reply::Refused(error) => Err(error),
            _ => Err(GateError::ForeignReply),
        }
    }
}

/// The task's value-only timer endpoint: take a snapshot and return an
/// unarmed plan, ringing the doorbell. No register owner crosses this port.
pub struct ModemLpTimerPort<H: GateHardware, R> {
    plans: Sender<H::TimerPlan>,
    pending: Receiver<H::TimerWork>,
    ring: R,
}

impl<H: GateHardware, R: RadioDoorbell> ModemLpTimerPort<H, R> {
    /// The handler's software-work snapshot, if any.
    pub fn take_work(&self) -> Option<H::TimerWork> {
        self.pending.try_receive().ok()
    }

    /// Submit an unarmed plan for the handler to apply.
    ///
    /// # Errors
    ///
    /// The slot is occupied; the plan comes back.
    pub fn submit_plan(&self, plan: H::TimerPlan) -> Result<(), H::TimerPlan> {
        self.plans.try_send(plan).map_err(|rejected| rejected.0)?;
        self.ring.ring();
        Ok(())
    }
}

/// The chip's owners: the HAL's bank, timer and doorbell.
pub enum ChipGate {}

/// The handler's part on the chip, which the image's interrupt table binds
/// once; its handler calls [`BluetoothInterruptsOf::service`].
pub type BluetoothInterrupts<X, B> = BluetoothInterruptsOf<ChipGate, X, B>;

impl<X: SignalExecutor + 'static, B> oer_interrupt_table::InterruptSources
    for BluetoothInterrupts<X, B>
{
    const SOURCES: &'static [u16] = oer_interrupt_table::interrupt_sources![
        bluetooth::BluetoothInterruptBankSources,
        bluetooth::ModemLpTimerSource,
        FromCpu3Irq,
    ];
}

impl GateHardware for ChipGate {
    type Doorbell = FromCpu3Irq;
    type Setup = bluetooth::InterruptSetupOwner;
    type Bank = bluetooth::InterruptRegistersOwner;
    type Released = bluetooth::InterruptOutputReleasedOwner;
    type ReleaseProof = bluetooth::BluetoothOutputReleased;
    type Scheduler = SchedulerInterruptHalf;
    type TimerInitialized = bluetooth::ModemLpTimerLowPowerHardwareInitializedOwner;
    type Timer = bluetooth::ModemLpTimerInterruptReadyOwner;
    type TimerPending = bluetooth::ModemLpTimerSoftwarePendingOwner;
    type TimerWork = bluetooth::ModemLpTimerWork;
    type TimerPlan = bluetooth::ModemLpTimerPlan;
    type CompareDisabled = bluetooth::ModemLpTimerCompareDisabled;

    fn open_bank(setup: Self::Setup) -> Self::Bank {
        setup.prepare_controller_output().stage_for_handler()
    }

    fn start_timer(timer: Self::TimerInitialized) -> Self::Timer {
        timer.start_runtime_timer().stage_for_interrupt()
    }

    fn serve_request<C: oer_time::Clock>(
        scheduler: &mut Self::Scheduler,
        bank: &mut Self::Bank,
        clock: &C,
    ) -> bool {
        scheduler.service(bank, clock)
    }

    fn capture_primary<C: oer_time::Clock>(
        bank: &mut Self::Bank,
        clock: &C,
    ) -> PrimaryInterruptCapture {
        super::primary::step_primary_interrupt(bank, clock)
    }

    fn capture_nrt(bank: &mut Self::Bank) -> bool {
        super::nrt::step_nrt_default_interrupt(bank).is_some()
    }

    fn step_timer(
        timer: Self::Timer,
    ) -> TimerEntry<Self::Timer, Self::TimerPending, Self::TimerWork> {
        match timer.step() {
            bluetooth::ModemLpTimerInterruptStep::Spurious(timer) => TimerEntry::Spurious(timer),
            bluetooth::ModemLpTimerInterruptStep::HandlerPending(pending) => {
                match pending.step_registers() {
                    bluetooth::ModemLpTimerHandlerRegisterStep::Rearmed(timer) => {
                        TimerEntry::Rearmed(timer)
                    }
                    bluetooth::ModemLpTimerHandlerRegisterStep::SoftwarePending { owner, work } => {
                        TimerEntry::SoftwarePending(owner, work)
                    }
                }
            }
        }
    }

    fn apply_timer_plan(
        timer: Self::TimerPending,
        plan: Self::TimerPlan,
    ) -> TimerEntry<Self::Timer, Self::TimerPending, Self::TimerWork> {
        match timer.apply_plan(plan) {
            bluetooth::ModemLpTimerHandlerRegisterStep::Rearmed(timer) => {
                TimerEntry::Rearmed(timer)
            }
            bluetooth::ModemLpTimerHandlerRegisterStep::SoftwarePending { owner, work } => {
                TimerEntry::SoftwarePending(owner, work)
            }
        }
    }

    fn observe_timer(timer: &mut Self::TimerPending) {
        timer.observe_interrupt();
    }

    fn stop_timer(timer: &mut Self::Timer) -> Self::CompareDisabled {
        timer.disable_compare_for_shutdown()
    }

    fn close_bank(
        bank: Self::Bank,
        released: Self::ReleaseProof,
    ) -> Result<Self::Released, Self::Bank> {
        bank.into_released(released)
    }
}

impl<X: SignalExecutor + 'static> BluetoothInterruptEnds<ChipGate, X> {
    /// Complete the task's ends once `_bound` proves the interrupt table
    /// binds the handler's part, with the doorbell's ringing half `ring`,
    /// which the same binding makes
    /// (`Doorbell::new::<FromCpu3Irq, BluetoothInterrupts<X, B>>(line, &bound)`).
    pub fn attach<B, R: RadioDoorbell + Clone>(
        self,
        _bound: &Bound<BluetoothInterrupts<X, B>>,
        ring: R,
    ) -> (
        BluetoothInterruptGate<ChipGate, X, R>,
        ModemLpTimerPort<ChipGate, R>,
    ) {
        self.with_ring(ring)
    }
}

/// Why the gates did not open.
#[must_use = "a refused opening returns the epoch's owners"]
pub struct GateOpenRefused {
    pub error: GateError,
    /// The owners, when the handler returned them.
    pub owners: Option<(
        bluetooth::InterruptSetupOwner,
        SchedulerInterruptHalf,
        bluetooth::ModemLpTimerLowPowerHardwareInitializedOwner,
    )>,
}

impl core::fmt::Debug for GateOpenRefused {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter
            .debug_struct("GateOpenRefused")
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl<X: SignalExecutor + 'static, R: RadioDoorbell> BluetoothInterruptGate<ChipGate, X, R> {
    /// Open the gates with the epoch's owners: the handler prepares the
    /// bank's controller output, then starts the runtime timer. The gate
    /// exists only once its part is bound
    /// ([`BluetoothInterruptEnds::attach`]).
    ///
    /// # Errors
    ///
    /// The gates are open already (the owners come back) or the exchange
    /// failed.
    pub async fn open(
        &mut self,
        setup: bluetooth::InterruptSetupOwner,
        scheduler: SchedulerInterruptHalf,
        timer: bluetooth::ModemLpTimerLowPowerHardwareInitializedOwner,
    ) -> Result<(), GateOpenRefused> {
        self.open_with(setup, scheduler, timer)
            .await
            .map_err(|(error, owners)| GateOpenRefused { error, owners })
    }

    /// The stopped timer, which leaves its handler only after the
    /// Controller reset that `_reset` proves.
    ///
    /// # Errors
    ///
    /// The timer is not stopped, or the exchange failed.
    pub async fn take_timer<M: ControllerMemory>(
        &mut self,
        _reset: &EngineResetTimerPending<M>,
    ) -> Result<bluetooth::ModemLpTimerInterruptReadyOwner, GateError> {
        self.take_stopped_timer().await
    }
}
