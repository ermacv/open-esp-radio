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
//! 1. sample the read-only LP/NRT causes, lower the doorbell line, apply
//!    the task's unarmed timer plan while
//!    retaining its register owner, step the timer, then serve the gate request
//!    ([`BluetoothInterruptGate`]);
//! 2. serve one request of the scheduler channel;
//! 3. sample and acknowledge the bank's primary status, then its NRT
//!    snapshots, publishing the primary capture to the scheduler worker;
//!
//! A dispatch observed by the timer step refuses a stop request in the same entry.
//!
//! A clear status writes nothing. The task opens the gates with the epoch's
//! owners (the bank still unprepared, the timer initialized but not
//! started): the handler prepares the controller output and starts the
//! timer. Preparation already arms the independent rollover lane after
//! reporting and clearing stale pending, while the counter remains stopped.
//! The bank
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

use core::{future::poll_fn, task::Poll};

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
use oer_memory::{
    AffineSpscQueue, AffineSpscReceiver, AffineSpscSender, CompactSpscCursor, OnceValue,
};

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
    /// Permanent read-only LP cause capability.
    type Status: Send + 'static;
    /// Read the LP cause word without acknowledgement.
    fn lp_status(status: &Self::Status) -> u32;
    /// Read NRT causes without acknowledgement.
    fn nrt_status(bank: &Self::Bank) -> [u32; 2];
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
    /// Register owner after the counter and measured delivery lanes stopped.
    type StoppedTimer: Send + 'static;
    /// Stop proof retaining the pending-byte observation.
    type StopProof: Send + 'static;

    /// Prepare the controller output of `setup` for the epoch.
    fn open_bank(setup: Self::Setup) -> Self::Bank;
    /// Start the runtime timer of `timer` for the epoch.
    fn start_timer(timer: Self::TimerInitialized) -> Self::Timer;
    /// Serve at most one request of the scheduler channel; whether one was.
    fn serve_request<C: oer_time::Clock>(
        scheduler: &mut Self::Scheduler,
        bank: &mut Self::Bank,
        clock: &C,
    ) -> Result<bool, bluetooth::BluetoothInterruptPolicyFault>;
    /// Sample and acknowledge the primary status.
    fn capture_primary<C: oer_time::Clock>(
        bank: &mut Self::Bank,
        clock: &C,
    ) -> PrimaryInterruptCapture;
    /// Sample and acknowledge the NRT snapshots; whether any was set.
    /// An unexpected primary overflow clear lane faults before either clear.
    fn capture_nrt(bank: &mut Self::Bank) -> Result<bool, bluetooth::BluetoothNrtInterruptFault>;
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
    /// Quiet the drained timer, retaining sampled pending in the stop proof.
    fn stop_timer(timer: Self::Timer) -> (Self::StoppedTimer, Self::StopProof);
    /// Retire rollover-only planning; dispatch refuses with the owner intact.
    fn stop_pending_timer(
        timer: Self::TimerPending,
    ) -> Result<(Self::StoppedTimer, Self::StopProof), Self::TimerPending>;
    /// Acknowledge rollover while stopped without rearming delivery.
    fn step_stopped_timer(
        timer: &mut Self::StoppedTimer,
    ) -> Result<
        bluetooth::BluetoothModemLpTimerStoppedInterruptObservation,
        bluetooth::BluetoothModemLpTimerStoppedInterruptFault,
    >;
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

/// The timer phase at entry, before any register step or owner transfer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GateTimerState {
    /// The timer owner is outside the handler.
    Vacant,
    /// The handler can step the timer.
    Ready,
    /// The task is planning from a value-only snapshot.
    AwaitingPlan,
    /// Delivery and counter have been stopped.
    Stopped,
}

/// Causes sampled before servicing an interrupt. No status bits are discarded.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GateObservation {
    /// Complete modem LP cause word.
    pub lp_status: u32,
    /// Both NRT cause words, or no bank owner in this handler.
    pub nrt_status: Option<[u32; 2]>,
    /// Software timer ownership phase.
    pub timer: GateTimerState,
}

/// A quarantining fault retaining its gate entry or controller bank observations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GateFault {
    /// A gate protocol invariant, with the exact handler entry observations.
    Invariant {
        reason: GateFaultReason,
        observation: GateObservation,
    },
    /// Controller admission or release failed with all bank observations.
    InterruptPolicy(bluetooth::BluetoothInterruptPolicyFault),
}

/// The precise invariant that quarantined the gate's owners.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GateFaultReason {
    /// A plan arrived without its pending timer owner.
    PlanWithoutPendingTimer,
    /// The reply slot did not accept its one reply.
    ReplyOccupied,
    /// The work slot did not accept its one projection.
    WorkOccupied,
    /// A stopped timer reported invalid work or status.
    StoppedTimer(bluetooth::BluetoothModemLpTimerStoppedInterruptFault),
    /// LP is pending while the handler has no timer owner.
    VacantTimer,
    /// A captured BTLC wrap exhausted the software counter; retain its value.
    ControllerTimeOverflow(super::ControllerTimeOverflowExhausted),
    /// An exact NRT pair contained the primary overflow clear lane; neither
    /// bank was acknowledged and both observations remain retained.
    NrtControllerTimeOverflow(bluetooth::BluetoothNrtInterruptFault),
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
    /// The timer is absent or stopped already.
    TimerNotReady,
    /// The outstanding plan or a later interrupt still requires dispatch.
    TimerDispatchPending,
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
    TimerStopped(H::StopProof),
    Closed {
        output: H::Released,
        scheduler: H::Scheduler,
    },
    Timer(H::StoppedTimer),
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
    /// Counter and both measured delivery lanes stopped until Controller reset.
    Stopped(H::StoppedTimer),
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
/// task's executor, the epoch's event cells and the image's first typed fault.
pub struct BluetoothInterruptSignals<X: SignalExecutor + 'static> {
    /// The reply signal, shared with the scheduler channel's replies.
    pub reply: &'static IrqSignal<X>,
    /// The scheduler and source-127 worker wakes.
    pub cells: &'static ControllerEventCells<X>,
    /// First invariant failure, with its entry observation and exact payload.
    /// Publication quarantines the gate until the image records and resets.
    pub fault: &'static OnceValue<GateFault>,
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
        status: H::Status,
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
                fault: signals.fault,
            },
            BluetoothInterruptsOf {
                _irq: irq,
                status,
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
                    stopped: None,
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
    fault: &'static OnceValue<GateFault>,
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
                fault: self.fault,
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
/// records a named fault.
struct Retained<H: GateHardware> {
    reply: Option<Reply<H>>,
    plan: Option<H::TimerPlan>,
    stopped: Option<H::StoppedTimer>,
    work: Option<H::TimerWork>,
}

/// The handler's part: the doorbell's interrupt part and lowering half, the
/// bank's and the timer's gates and the handler's ends of the gate's queues.
pub struct BluetoothInterruptsOf<H: GateHardware, X: SignalExecutor + 'static, B> {
    _irq: H::Doorbell,
    status: H::Status,
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
    ///
    /// An invariant failure is absorbing: all owners stay quarantined, and
    /// subsequent calls return the first fault without touching hardware.
    /// The image must record the returned fault and reset without dropping owners.
    ///
    /// # Errors
    ///
    /// A broken gate invariant; the first failure is returned on every later call.
    pub fn service<C: oer_time::Clock>(&mut self, clock: &C) -> Result<(), GateFault> {
        self.check_fault()?;
        let observation = GateObservation {
            lp_status: H::lp_status(&self.status),
            nrt_status: match &self.bank {
                Bank::Open { registers, .. } => Some(H::nrt_status(registers)),
                Bank::Closed => None,
            },
            timer: match &self.timer {
                Timer::Vacant => GateTimerState::Vacant,
                Timer::Ready(_) => GateTimerState::Ready,
                Timer::AwaitingPlan(_) => GateTimerState::AwaitingPlan,
                Timer::Stopped(_) => GateTimerState::Stopped,
            },
        };
        self.line.acknowledge();
        self.apply_timer_plan(observation);
        self.check_fault()?;
        // A same-entry dispatch must become retained software work before
        // StopTimer can clear pending. Refusal then uses the AwaitingPlan rule.
        self.step_timer(observation);
        self.check_fault()?;
        self.serve_gate_request(observation);
        self.check_fault()?;
        if let Bank::Open {
            registers,
            scheduler,
        } = &mut self.bank
        {
            let replied = match H::serve_request(scheduler, registers, clock) {
                Ok(replied) => replied,
                Err(cause) => {
                    let _first = self.signals.fault.set(GateFault::InterruptPolicy(cause));
                    return self.check_fault();
                }
            };
            let capture = H::capture_primary(registers, clock);
            let published = self
                .signals
                .cells
                .scheduler_wake()
                .publish_from_interrupt(capture);
            if let Err(cause) = published {
                self.fault(GateFaultReason::ControllerTimeOverflow(cause), observation);
                return self.check_fault();
            }
            if let Err(cause) = H::capture_nrt(registers) {
                self.fault(
                    GateFaultReason::NrtControllerTimeOverflow(cause),
                    observation,
                );
                return self.check_fault();
            }
            // A service barrier completes only after the entry's pending
            // primary cause has reached the wrap counter. The table proves
            // that this handler is above its requesting P2 executor.
            if replied {
                self.signals.reply.raise(1);
            }
        }
        Ok(())
    }

    /// Whether the bank's gate is open.
    pub const fn is_open(&self) -> bool {
        matches!(self.bank, Bank::Open { .. })
    }

    fn fault(&self, reason: GateFaultReason, observation: GateObservation) {
        let _first = self.signals.fault.set(GateFault::Invariant {
            reason,
            observation,
        });
    }

    fn check_fault(&self) -> Result<(), GateFault> {
        match self.signals.fault.get() {
            Some(fault) => Err(fault),
            None => Ok(()),
        }
    }

    fn apply_timer_plan(&mut self, observation: GateObservation) {
        let Ok(plan) = self.plans.try_receive() else {
            return;
        };
        match core::mem::replace(&mut self.timer, Timer::Vacant) {
            Timer::AwaitingPlan(timer) => {
                self.accept_timer_entry(H::apply_timer_plan(timer, plan), observation);
            }
            other => {
                self.timer = other;
                self.retained.plan = Some(plan);
                self.fault(GateFaultReason::PlanWithoutPendingTimer, observation);
            }
        }
    }

    fn serve_gate_request(&mut self, observation: GateObservation) {
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
                Timer::Ready(timer) => {
                    let (timer, stopped) = H::stop_timer(timer);
                    self.timer = Timer::Stopped(timer);
                    Reply::TimerStopped(stopped)
                }
                Timer::AwaitingPlan(timer) => match H::stop_pending_timer(timer) {
                    Ok((timer, stopped)) => {
                        // Only the ending epoch's rollover projection is discarded.
                        self.timer = Timer::Stopped(timer);
                        Reply::TimerStopped(stopped)
                    }
                    Err(timer) => {
                        self.timer = Timer::AwaitingPlan(timer);
                        Reply::Refused(GateError::TimerDispatchPending)
                    }
                },
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
            self.fault(GateFaultReason::ReplyOccupied, observation);
            return;
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

    fn accept_timer_entry(
        &mut self,
        entry: TimerEntry<H::Timer, H::TimerPending, H::TimerWork>,
        observation: GateObservation,
    ) {
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
                    self.fault(GateFaultReason::WorkOccupied, observation);
                    return;
                }
                ModemLpTimerStableInterruptStep::SoftwarePending
            }
        };
        let _ = step.publish(self.signals.cells.timer_wake());
    }

    fn step_timer(&mut self, observation: GateObservation) {
        match core::mem::replace(&mut self.timer, Timer::Vacant) {
            Timer::Ready(timer) => {
                self.accept_timer_entry(H::step_timer(timer), observation);
            }
            Timer::Stopped(mut timer) => match H::step_stopped_timer(&mut timer) {
                Ok(_) => self.timer = Timer::Stopped(timer),
                Err(cause) => {
                    // Keep invalid stopped state through the existing fault path.
                    self.retained.stopped = Some(timer);
                    self.fault(GateFaultReason::StoppedTimer(cause), observation);
                }
            },
            Timer::AwaitingPlan(mut timer) => {
                H::observe_timer(&mut timer);
                self.timer = Timer::AwaitingPlan(timer);
            }
            Timer::Vacant => {
                if observation.lp_status != 0 {
                    self.fault(GateFaultReason::VacantTimer, observation);
                }
            }
        }
    }
}

/// The task's end of the gate: one request at a time, each announced by the
/// doorbell, its reply awaited on the reply signal.
pub struct BluetoothInterruptGate<H: GateHardware, X: SignalExecutor + 'static, R> {
    requests: Sender<Request<H>>,
    replies: Receiver<Reply<H>>,
    ring: R,
    fault: &'static OnceValue<GateFault>,
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
        const {
            assert!(
                oer_interrupt_table::request_direction(R::HANDLER, R::EXECUTOR),
                "a request handler must be above its executor on the same hart"
            )
        };
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

    /// Stop the counter and both measured delivery lanes of the drained
    /// timer. Rollover-only planning may retire its epoch projection; any
    /// outstanding dispatch refuses. The handler keeps the stopped owner
    /// until the Controller reset.
    ///
    /// # Errors
    ///
    /// The timer is not with its handler, or the exchange failed.
    pub async fn stop_timer(&mut self) -> Result<H::StopProof, GateError> {
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

    pub(crate) async fn take_stopped_timer(&mut self) -> Result<H::StoppedTimer, GateError> {
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
    type Status = bluetooth::ModemLpTimerStatus;

    fn lp_status(status: &Self::Status) -> u32 {
        status.read()
    }
    fn nrt_status(bank: &Self::Bank) -> [u32; 2] {
        bank.nrt_status()
    }

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
    type StoppedTimer = bluetooth::ModemLpTimerStoppedOwner;
    type StopProof = bluetooth::ModemLpTimerStopped;

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
    ) -> Result<bool, bluetooth::BluetoothInterruptPolicyFault> {
        scheduler.service(bank, clock)
    }

    fn capture_primary<C: oer_time::Clock>(
        bank: &mut Self::Bank,
        clock: &C,
    ) -> PrimaryInterruptCapture {
        super::primary::step_primary_interrupt(bank, clock)
    }

    fn capture_nrt(bank: &mut Self::Bank) -> Result<bool, bluetooth::BluetoothNrtInterruptFault> {
        super::nrt::step_nrt_default_interrupt(bank).map(|epoch| epoch.is_some())
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

    fn stop_timer(timer: Self::Timer) -> (Self::StoppedTimer, Self::StopProof) {
        timer.stop_for_shutdown()
    }

    fn stop_pending_timer(
        timer: Self::TimerPending,
    ) -> Result<(Self::StoppedTimer, Self::StopProof), Self::TimerPending> {
        timer.stop_for_shutdown()
    }

    fn step_stopped_timer(
        timer: &mut Self::StoppedTimer,
    ) -> Result<
        bluetooth::BluetoothModemLpTimerStoppedInterruptObservation,
        bluetooth::BluetoothModemLpTimerStoppedInterruptFault,
    > {
        timer.acknowledge_interrupt()
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
    /// (`Doorbell::<3, HandlerEntry, ExecutorEntry>::new::<FromCpu3Irq>(line, &bound)`).
    pub fn attach<E, B, R: RadioDoorbell + Clone>(
        self,
        _bound: &Bound<E>,
        ring: R,
    ) -> (
        BluetoothInterruptGate<ChipGate, X, R>,
        ModemLpTimerPort<ChipGate, R>,
    )
    where
        E: oer_interrupt_table::Entry<Part = BluetoothInterrupts<X, B>>,
    {
        const {
            assert!(
                oer_interrupt_table::request_direction(E::CONTEXT, R::EXECUTOR),
                "a request handler must be above its executor on the same hart"
            );
            assert!(
                E::LEVEL == R::HANDLER.level && E::HART == R::HANDLER.hart,
                "the doorbell must ring the bound handler"
            );
        }
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
    /// Publish the first admission fault before initialization, retaining the
    /// caller's owners. The handler returns it without any hardware access;
    /// the image records and resets without dropping the interrupted owner.
    pub(crate) fn quarantine_interrupt_policy(
        &self,
        cause: bluetooth::BluetoothInterruptPolicyFault,
    ) {
        let _first = self.fault.set(GateFault::InterruptPolicy(cause));
        self.ring.ring();
    }

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
    ) -> Result<bluetooth::ModemLpTimerStoppedOwner, GateError> {
        self.take_stopped_timer().await
    }
}

/// Complete the retirement's timer handoff. Stop refuses any dispatch;
/// only a late rollover snapshot/time projection can remain for removal.
#[cfg(any(target_arch = "riscv32", test))]
pub(crate) async fn stop_timer_for_retirement<
    H: GateHardware,
    X: SignalExecutor,
    R: RadioDoorbell,
>(
    gate: &mut BluetoothInterruptGate<H, X, R>,
    timer_port: &ModemLpTimerPort<H, R>,
) -> Result<H::StopProof, GateError> {
    let stopped = gate.stop_timer().await?;
    let _retired_rollover = timer_port.take_work();
    Ok(stopped)
}
