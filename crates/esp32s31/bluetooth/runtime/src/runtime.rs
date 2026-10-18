//! The shared part of the radio, its driver, which owns the radio role and
//! its hardware, and the driver's requests.

use core::{
    cell::{Cell, RefCell},
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    task::Poll,
};

use embassy_futures::select::{Either, Either3, select, select3};
use embassy_sync::{
    blocking_mutex::{Mutex as BlockingMutex, raw::RawMutex},
    channel::Channel,
    signal::Signal,
    waitqueue::WakerRegistration,
};
use oer_bluetooth_port::EnableResult;
use oer_bluetooth_radio::{
    CancelError, ClockError, EventId, EventsLost, LeCapabilities, LeInstant, OutsideEpoch,
    OwnedRadioRequest, Poisoned, PortResult, RadioActivity, RadioOutcome, RadioRequest,
    RequestError,
};
use oer_esp32s31_bluetooth::{
    ControllerTimeSample,
    controller_time::{
        ControllerTimeEventError, ControllerTimeEventStep, ControllerTimeRequestError,
    },
    scheduler::{
        SchedulerFinishedListWorkerStep, SchedulerHardwareError, SchedulerNext,
        SchedulerStartError, SchedulerStopError, SchedulerWait,
    },
};
use oer_esp32s31_bluetooth_radio::{
    BluetoothCapabilities, BluetoothRadio, BluetoothRadioConfig, BluetoothRadioMemory,
    BluetoothRadioSink, CoexistenceProfile, RadioFault, RadioFaulted, RadioStep,
};
use oer_esp32s31_hal::bluetooth::{
    BluetoothSchedulerHardwareListIndex, BluetoothSchedulerStop, BluetoothSchedulerStopStep,
};
use oer_time::{Duration, Timer};

use crate::port::BluetoothBackend;
use crate::{
    BluetoothOutcome,
    hardware::{BluetoothRadioHardware, RxChainPublicationError},
};

/// Delay before a hardware wait is observed again.
///
/// Scheduler commands, the stop sequence and the controller-time latch
/// settle within microseconds; a scheduler interrupt also ends the delay.
pub const HARDWARE_RECHECK: Duration = Duration::from_micros(20);

/// The bound of each transition of the port's owner, `Enable` and
/// `Disable`; past it the runtime is poisoned
/// ([`BluetoothRuntimeFault::TransitionTimedOut`]).
///
/// The bound turns a hung hardware stop into a poisoned port; it does not
/// time a healthy one. `Disable` (the scheduler stop, the withdrawal of
/// every admitted event and its settling) took 121 us under a secure GATT
/// Host restart with advertising and one connection (HIL run
/// `-001db8ba`). The withdrawal after the stop is synchronous
/// and barely grows with the number of events, so a margin of about 80
/// times also covers what that run did not measure: several connections,
/// a scan window or a Direct Test Mode event in flight. The repository
/// owner chose the value:
/// <https://github.com/the predecessor repository/issues/581#issuecomment->.
/// `Enable` (one controller-time sample and the scheduler's resume) does a
/// subset of that work and shares the bound, as the owner chose:
/// <https://github.com/the predecessor repository/issues/581#issuecomment->.
/// The trace's lifecycle records time each transition.
pub const TRANSITION_BOUND: Duration = Duration::from_millis(10);

/// Longest time without a controller-time sample while the runtime is idle.
///
/// The radio extends the 32-bit controller clock from successive samples,
/// which needs a sample at least once per half clock range.
pub const TIME_REFRESH: Duration = Duration::from_secs(60);

/// Why the controller-time latch produced no sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothTimeError {
    /// The latch refused a request.
    Request(ControllerTimeRequestError),
    /// The latch lost or refused the request in flight.
    Event(ControllerTimeEventError),
    /// The latch reported no request in flight.
    Lost,
    /// The runtime's timer cannot express a wait's deadline.
    Deadline,
}

/// Why the runtime is poisoned: the cause its port reports.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothFault<E> {
    /// The radio faulted.
    Radio(RadioFault),
    /// The runner stopped on a hardware fault, or a transition did not end
    /// within its bound.
    Runner(BluetoothRuntimeFault<E>),
    /// The platform beside the runtime failed: an interrupt service or the
    /// timer task ([`BluetoothRuntime::platform_fault`]).
    Platform,
}

/// Why the runtime stopped driving the radio.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothRuntimeFault<E> {
    /// A list-zero step or observation failed.
    Scheduler(SchedulerHardwareError),
    /// The idle scheduler did not start.
    Start(SchedulerStartError<E>),
    /// The stop sequence ended before the scheduler stopped.
    StopSequence(SchedulerStopError),
    /// A finished-list observation could not be captured losslessly.
    FinishedLists(oer_esp32s31_bluetooth::scheduler::SchedulerFinishedListCaptureError),
    /// The radio refused the stopped receipt.
    Stop,
    /// No controller-time sample could be taken to resume.
    Time(BluetoothTimeError),
    /// A transition of the port's owner did not end within
    /// [`TRANSITION_BOUND`].
    TransitionTimedOut,
    /// The port's owner was abandoned ([`BluetoothRuntime::abandon`]).
    Abandoned,
    /// After `Disable`, the outcome queue still owed the end of an admitted
    /// event: its accounting and the radio disagree.
    Unsettled,
}

/// Why a radio was not taken out of the runtime.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothUninstallError<E> {
    /// The port was poisoned while enabled: admitted events may still owe
    /// their ends. Nothing changed.
    NotDisabled,
    /// The driver stopped on a fault, or its stop or controller-time drain
    /// failed.
    Fault(BluetoothRuntimeFault<E>),
    /// The driver was never asked to retire.
    NotRetired,
}

/// A radio its driver did not leave for the uninstall, with why: the
/// composition keeps it fail-stop.
#[allow(
    clippy::type_complexity,
    reason = "the role's pool capacities stay visible in the retained radio"
)]
pub struct BluetoothRetained<
    H: BluetoothRadioHardware,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
> {
    /// Why the radio was not left.
    pub error: BluetoothUninstallError<H::StartError>,
    /// The radio role, in the state the driver left it.
    pub radio: Radio<LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS, ITEMS>,
    /// The hardware.
    pub hardware: H,
}

/// Why a radio was not installed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BluetoothInstallError {
    /// A radio is already installed.
    AlreadyInstalled,
    /// The receive chains were not published.
    RxChains(RxChainPublicationError),
    /// No controller-time sample could be taken.
    Time(BluetoothTimeError),
    /// The hardware's scheduler policy or sleep-clock accuracy is not the
    /// one the runtime states its timing for.
    TimingMismatch,
    /// A port owner was abandoned: the radio it left is unreachable until a
    /// reset.
    Abandoned,
}

pub(crate) type Radio<
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
> = BluetoothRadio<LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS, ITEMS>;

struct Installed<
    H: BluetoothRadioHardware,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
> {
    radio: Radio<LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS, ITEMS>,
    hardware: H,
    awaiting: Option<SchedulerWait>,
    /// The BLE PHY ETM route a test session disabled; restored when the
    /// session ends, when the runtime faults and when the radio leaves.
    phy_route: Option<H::DisabledPhyRoute>,
    faulted: bool,
}

impl<
    H: BluetoothRadioHardware,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
> Installed<H, LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS, ITEMS>
{
    /// Restore the BLE PHY ETM route if a test session still holds it.
    fn restore_phy_route(&mut self) {
        if let Some(route) = self.phy_route.take() {
            self.hardware.restore_phy_etm_route(route);
        }
    }

    /// Stop driving the radio after a hardware fault.
    fn fault(&mut self) {
        self.faulted = true;
        self.restore_phy_route();
    }
}

/// What one pass of the driver left to do.
enum Pass {
    /// The radio asked for a scheduler stop.
    Stop,
    /// More work is ready now.
    Continue,
    /// A hardware wait is pending.
    Recheck,
    /// Nothing is ready until a wake, a request or the refresh deadline.
    Idle,
}

/// The port's admission of events: the state of its owner.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Admission {
    Disabled,
    Enabled,
}

/// What the outcome queue owes the consumer; `CONNECTIONS` bounds the
/// connection events admitted at once.
struct Owed<const CONNECTIONS: usize> {
    /// A received PDU was dropped: the next outcome reports the loss first.
    lost: bool,
    /// An outcome taken after the loss it reported.
    held: Option<BluetoothOutcome>,
    /// Admitted events whose end is not queued yet.
    events: usize,
    /// Slots reserved for outcomes that are never dropped: two for each
    /// admitted event (its end, and the acknowledgement or test report
    /// before it) and the received PDUs an admitted connection event may
    /// still report.
    reserved: usize,
    /// Each admitted connection event and the slots it still holds for the
    /// data PDUs the hardware acknowledges.
    data: [Option<(EventId, usize)>; CONNECTIONS],
    /// The acknowledgement or test report of the event about to end took
    /// one of its reserved slots.
    companion: bool,
    admission: Admission,
}

/// One queued outcome, with whether a loss precedes it.
struct Entry {
    lost_before: bool,
    outcome: BluetoothOutcome,
}

/// The bounded outcome queue. An event's admission reserves the slots of
/// its terminal outcomes and, for a connection event, of the data PDUs the
/// hardware acknowledges, so none of them is lost: the Link Layer promises
/// its peer reliable delivery. A full queue drops only the received PDUs
/// nothing acknowledges (advertising and scan reports) and reports the loss
/// in place of the first dropped one.
struct OutcomeQueue<M: RawMutex, const EVENTS: usize, const CONNECTIONS: usize> {
    entries: Channel<M, Entry, EVENTS>,
    owed: BlockingMutex<M, RefCell<Owed<CONNECTIONS>>>,
}

impl<M: RawMutex, const EVENTS: usize, const CONNECTIONS: usize>
    OutcomeQueue<M, EVENTS, CONNECTIONS>
{
    const fn new() -> Self {
        Self {
            entries: Channel::new(),
            owed: BlockingMutex::new(RefCell::new(Owed {
                lost: false,
                held: None,
                events: 0,
                reserved: 0,
                data: [None; CONNECTIONS],
                companion: false,
                admission: Admission::Disabled,
            })),
        }
    }

    fn owed<R>(&self, entry: impl FnOnce(&mut Owed<CONNECTIONS>) -> R) -> R {
        self.owed.lock(|owed| entry(&mut owed.borrow_mut()))
    }

    fn admission(&self) -> Admission {
        self.owed(|owed| owed.admission)
    }

    /// Whether `slots` more terminal outcomes fit beside the queued and
    /// reserved ones.
    fn has_room(&self, owed: &Owed<CONNECTIONS>, slots: usize) -> bool {
        self.entries.len() + owed.reserved + slots <= EVENTS
    }

    /// Reserve the terminal slots of event `id` before its admission, and
    /// `data` more for the data PDUs a connection event may receive;
    /// `false` when they do not fit.
    fn admit_event(&self, id: EventId, data: usize) -> bool {
        self.owed(|owed| {
            let free = owed.data.iter().position(Option::is_none);
            if !self.has_room(owed, 2 + data) || (data > 0 && free.is_none()) {
                return false;
            }
            if data > 0 {
                owed.data[free.expect("checked above")] = Some((id, data));
            }
            owed.reserved += 2 + data;
            owed.events += 1;
            true
        })
    }

    /// Return the slots of event `id`, which the radio refused.
    fn withdraw_event(&self, id: EventId) {
        self.owed(|owed| {
            owed.reserved -= 2 + Self::release_data(owed, id);
            owed.events -= 1;
        });
    }

    /// Forget the data slots event `id` still holds; how many.
    fn release_data(owed: &mut Owed<CONNECTIONS>, id: EventId) -> usize {
        owed.data
            .iter_mut()
            .find(|slot| slot.is_some_and(|(event, _)| event == id))
            .and_then(Option::take)
            .map_or(0, |(_, left)| left)
    }

    /// Open admission: the owner enabled the port.
    fn open(&self) {
        self.owed(|owed| owed.admission = Admission::Enabled);
    }

    /// Close admission once the withdrawn events all ended: `false` when
    /// an admitted event still owes its end.
    fn close(&self) -> bool {
        self.owed(|owed| {
            owed.admission = Admission::Disabled;
            owed.events == 0
        })
    }

    /// Queue `outcome`: a terminal one in the slot reserved for it, a
    /// received PDU when a slot is free.
    fn push(&self, outcome: BluetoothOutcome) {
        self.owed(|owed| {
            match &outcome {
                BluetoothOutcome::Received { id, .. } => {
                    let data = owed
                        .data
                        .iter_mut()
                        .flatten()
                        .find(|(event, left)| *event == *id && *left > 0);
                    if let Some((_, left)) = data {
                        // Acknowledged connection data: its reserved slot.
                        *left -= 1;
                        owed.reserved -= 1;
                    } else if !self.has_room(owed, 1) {
                        owed.lost = true;
                        crate::trace::emit(|| oer_bluetooth_trace::QueueLoss {
                            op: oer_bluetooth_trace::QueueOp::ReportDropped,
                            id: id.get(),
                        });
                        return;
                    }
                }
                BluetoothOutcome::TransmitAcknowledged(_) | BluetoothOutcome::TestReport { .. } => {
                    owed.reserved -= 1;
                    owed.companion = true;
                }
                BluetoothOutcome::EventEnded { id, .. } => {
                    let unused_data = Self::release_data(owed, *id);
                    owed.reserved -= unused_data
                        + if core::mem::take(&mut owed.companion) {
                            1
                        } else {
                            2
                        };
                    owed.events -= 1;
                }
            }
            self.send(owed, outcome);
        });
    }

    /// Queue `outcome` in a slot known to be free.
    fn send(&self, owed: &mut Owed<CONNECTIONS>, outcome: BluetoothOutcome) {
        let lost_before = core::mem::take(&mut owed.lost);
        let sent = self.entries.try_send(Entry {
            lost_before,
            outcome,
        });
        assert!(sent.is_ok(), "a terminal outcome holds a reserved slot");
    }

    fn take(&self) -> Option<Result<BluetoothOutcome, EventsLost>> {
        self.owed(|owed| {
            if let Some(outcome) = owed.held.take() {
                return Some(Ok(outcome));
            }
            match self.entries.try_receive() {
                Ok(Entry {
                    lost_before: true,
                    outcome,
                }) => {
                    owed.held = Some(outcome);
                    trace_loss_reported();
                    Some(Err(EventsLost))
                }
                Ok(Entry { outcome, .. }) => Some(Ok(outcome)),
                Err(_) if core::mem::take(&mut owed.lost) => {
                    trace_loss_reported();
                    Some(Err(EventsLost))
                }
                Err(_) => None,
            }
        })
    }

    /// Forget the events of a radio that left with its port: their ends
    /// never come, the outcomes it did not deliver and a loss it owed end
    /// with the port's stream, and the next radio starts disabled with an
    /// empty queue.
    fn reset(&self) {
        self.owed(|owed| {
            while self.entries.try_receive().is_ok() {}
            owed.held = None;
            owed.lost = false;
            owed.events = 0;
            owed.reserved = 0;
            owed.data = [None; CONNECTIONS];
            owed.companion = false;
            owed.admission = Admission::Disabled;
        });
    }
}

/// Record that the consumer took a loss in place of dropped reports.
fn trace_loss_reported() {
    crate::trace::emit(|| oer_bluetooth_trace::QueueLoss {
        op: oer_bluetooth_trace::QueueOp::LossReported,
        id: 0,
    });
}

/// The sink of one driver step: outcomes go to the queue, and a fault
/// poisons the runtime.
struct QueueSink<'a, M: RawMutex, E, const EVENTS: usize, const CONNECTIONS: usize> {
    outcomes: &'a OutcomeQueue<M, EVENTS, CONNECTIONS>,
    fault: &'a BlockingMutex<M, Cell<Option<BluetoothFault<E>>>>,
    changed: &'a Signal<M, ()>,
}

impl<M: RawMutex, E: Copy, const EVENTS: usize, const CONNECTIONS: usize> BluetoothRadioSink
    for QueueSink<'_, M, E, EVENTS, CONNECTIONS>
{
    fn outcome(&mut self, outcome: RadioOutcome<'_>) {
        match BluetoothOutcome::copy(outcome) {
            Some(copied) => self.outcomes.push(copied),
            // A copy that could not hold its PDU is a fault as well.
            None => self.fault(RadioFault::MemoryInconsistency),
        }
    }

    fn fault(&mut self, fault: RadioFault) {
        self.fault.lock(|cause| {
            if cause.get().is_none() {
                cause.set(Some(BluetoothFault::Radio(fault)));
            }
        });
        self.changed.signal(());
    }
}

/// A request of the port to its driver; the parts and the owner send one at
/// a time, and it owns its PDUs ([`OwnedRadioRequest`]).
#[allow(
    clippy::large_enum_variant,
    reason = "the one request in flight owns its PDU inline, in the runtime's one slot"
)]
pub(crate) enum Request<K: LeCapabilities> {
    Submit(OwnedRadioRequest<K>),
    Cancel(EventId),
    Enable,
    Disable,
}

/// A call's request the driver has not taken yet: a call dropped before
/// its reply withdraws it, so nothing happens. A request the driver took is
/// no longer there to withdraw; the driver serves it to the end.
struct Withdraw<'a, M: RawMutex, T> {
    request: &'a Signal<M, (u32, T)>,
    ticket: u32,
}

impl<M: RawMutex, T> Drop for Withdraw<'_, M, T> {
    fn drop(&mut self) {
        if let Some((ticket, request)) = self.request.try_take()
            && ticket != self.ticket
        {
            // Not this call's: callers never overlap, so this cannot
            // happen; put it back rather than lose it.
            self.request.signal((ticket, request));
        }
    }
}

/// The driver's reply to one [`Request`].
pub(crate) enum Reply<F> {
    Submit(PortResult<(), RequestError, F>),
    Cancel(PortResult<(), CancelError, F>),
    Transition(Result<(), Poisoned<F>>),
    /// The driver stopped on a fault before it served the request.
    Poisoned(Poisoned<F>),
}

/// The part of the Bluetooth LE radio its port and its driver share: the
/// outcome queue, the request exchange, the poison and the wakes.
///
/// [`Self::install`] installs a radio and returns its port's backend
/// ([`BluetoothBackend`], an [`oer_bluetooth_radio::LeRadioBackend`]) and
/// its driver ([`BluetoothDriver`]), which owns the radio role and its
/// hardware by value. The driver drives the scheduler: it reports
/// completions, inserts admitted events, carries list transactions through
/// their hardware waits, samples the controller clock at least every
/// [`TIME_REFRESH`] and serves the port's requests in between, each as a
/// message that owns its PDUs. Nothing else reaches the radio: the backend
/// sends requests, the interrupt entry signals a wake
/// ([`Self::on_scheduler_wake`]), and the composition's tasks read the
/// active roles ([`Self::next_activity`]) and set the coexistence profile
/// ([`Self::set_coexistence`]) through signals and atomics.
///
/// `EVENTS` bounds the outcomes waiting for the port's event consumer: each
/// admitted event reserves the slots of its end and of the acknowledgement
/// or test report before it, a connection event also `RX_PACKETS` slots for
/// the data PDUs the hardware acknowledges, and an event that finds its
/// slots taken is refused as `Busy`. A full queue drops the newest
/// advertising or scan report and reports [`EventsLost`] once in its place.
///
/// `C` is the scheduler policy and sleep-clock accuracy the runtime is built
/// with: its port's capabilities ([`BluetoothCapabilities`]) and their
/// timing come from it, and an install refuses hardware of another.
///
/// A hardware fault of the radio or the driver, a transition past
/// [`TRANSITION_BOUND`] or a platform fault poisons the runtime: the queued
/// outcomes are still reported, then [`Poisoned`] with its
/// [`BluetoothFault`] at every call. An abandoned owner
/// ([`Self::abandon`]) leaves it poisoned and refusing any install until a
/// reset; otherwise a new install clears the poison.
#[allow(
    clippy::type_complexity,
    reason = "the role's pool capacities stay visible in the runtime type"
)]
pub struct BluetoothRuntime<
    M: RawMutex,
    H: BluetoothRadioHardware,
    T,
    C: BluetoothRadioConfig,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
    const EVENTS: usize,
> {
    /// A radio is installed: its driver and backend exist.
    occupied: AtomicBool,
    outcomes: OutcomeQueue<M, EVENTS, CONNECTIONS>,
    /// Why the radio or the driver faulted; a new install clears it.
    fault: BlockingMutex<M, Cell<Option<BluetoothFault<H::StartError>>>>,
    /// Why the driver must stop at its next pass: a transition past its
    /// bound or an abandoned owner.
    halt: BlockingMutex<M, Cell<Option<BluetoothRuntimeFault<H::StartError>>>>,
    /// A port owner was abandoned: no install until a reset.
    abandoned: AtomicBool,
    /// Clock readings asked, each reader's ticket: the owner's `now` may
    /// run beside itself, and one sample answers every ticket asked before
    /// it.
    now_asked: AtomicU32,
    /// The last ticket the driver answered, and that sample's answer.
    now_answered: BlockingMutex<
        M,
        Cell<(
            u32,
            Option<PortResult<LeInstant, ClockError, BluetoothFault<H::StartError>>>,
        )>,
    >,
    /// The waiting readers' waker: they share the port's task with the
    /// driver.
    now_waker: BlockingMutex<M, RefCell<WakerRegistration>>,
    /// The last call's ticket: each call numbers its request, and a reply
    /// reaches only the call of its ticket.
    tickets: AtomicU32,
    /// The one request in flight, with its call's ticket: the phase client
    /// takes its calls by `&mut` and the owner its transitions by value, and
    /// the two never run together, so no second request meets it.
    request: Signal<
        M,
        (
            u32,
            Request<
                BluetoothCapabilities<
                    C,
                    LEGACY,
                    CONNECTABLE,
                    SCANNERS,
                    CONNECTIONS,
                    SCAN_PACKETS,
                    RX_PACKETS,
                    ITEMS,
                >,
            >,
        ),
    >,
    /// The driver's reply, with the ticket of the call it answers.
    reply: Signal<M, (u32, Reply<BluetoothFault<H::StartError>>)>,
    /// Raised when the consumer must look again without a new outcome.
    changed: Signal<M, ()>,
    /// The scheduler policy and sleep-clock accuracy the timing is for.
    config: core::marker::PhantomData<fn() -> C>,
    work: Signal<M, ()>,
    activity: Signal<M, RadioActivity>,
    /// Whether another radio shares the antenna.
    shared: AtomicBool,
    /// Whether a controller-time sample passed the last instant of the radio
    /// epoch; a new install clears it.
    exhausted: AtomicBool,
    /// The image's monotonic time the driver's rechecks and the transition
    /// bounds wait on.
    timer: T,
}

/// The driver of an installed radio: it owns the radio role and its
/// hardware by value and is their one user ([`BluetoothRuntime`]).
///
/// The port's joined driver (`PortBackend::drive`, which `run_port` polls
/// in the task of the port's body) runs it for the port's whole life; it
/// returns once the radio is left for the uninstall, or on a fault, which
/// poisoned the port first.
#[must_use = "a driver owns the installed radio"]
#[allow(
    clippy::type_complexity,
    reason = "the role's pool capacities stay visible in the driver type"
)]
pub struct BluetoothDriver<
    'r,
    M: RawMutex,
    H: BluetoothRadioHardware,
    T,
    C: BluetoothRadioConfig,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
    const EVENTS: usize,
> {
    runtime: &'r BluetoothRuntime<
        M,
        H,
        T,
        C,
        LEGACY,
        CONNECTABLE,
        SCANNERS,
        CONNECTIONS,
        SCAN_PACKETS,
        RX_PACKETS,
        ITEMS,
        EVENTS,
    >,
    installed:
        Installed<H, LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS, ITEMS>,
    /// The fault that stopped the driver; it stops again at once.
    fault: Option<BluetoothRuntimeFault<H::StartError>>,
    /// How the retirement ended: the radio left for the uninstall, or why
    /// it was not.
    retired: Option<Result<(), BluetoothUninstallError<H::StartError>>>,
}

impl<
    M: RawMutex,
    H: BluetoothRadioHardware,
    T: Timer,
    C: BluetoothRadioConfig,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
    const EVENTS: usize,
>
    BluetoothRuntime<
        M,
        H,
        T,
        C,
        LEGACY,
        CONNECTABLE,
        SCANNERS,
        CONNECTIONS,
        SCAN_PACKETS,
        RX_PACKETS,
        ITEMS,
        EVENTS,
    >
{
    /// An empty runtime whose waits are on `timer`, for hardware of `C`'s
    /// scheduler policy and sleep clock, suitable for a `static`.
    pub const fn new(timer: T) -> Self {
        Self {
            timer,
            occupied: AtomicBool::new(false),
            outcomes: OutcomeQueue::new(),
            fault: BlockingMutex::new(Cell::new(None)),
            halt: BlockingMutex::new(Cell::new(None)),
            abandoned: AtomicBool::new(false),
            now_asked: AtomicU32::new(0),
            now_answered: BlockingMutex::new(Cell::new((0, None))),
            now_waker: BlockingMutex::new(RefCell::new(WakerRegistration::new())),
            tickets: AtomicU32::new(0),
            request: Signal::new(),
            reply: Signal::new(),
            changed: Signal::new(),
            config: core::marker::PhantomData,
            work: Signal::new(),
            activity: Signal::new(),
            shared: AtomicBool::new(false),
            exhausted: AtomicBool::new(false),
        }
    }

    /// Publish the receive chains of `memory`, take the first controller-time
    /// sample and install the radio: its port's backend and its driver,
    /// which owns the radio and hardware until its retirement.
    ///
    /// This runs once per powered epoch, after the interrupt owner is
    /// published and before the first scheduler RUN.
    ///
    /// # Errors
    ///
    /// Returns the memory and the hardware unchanged apart from the chain
    /// publication.
    #[allow(
        clippy::type_complexity,
        clippy::result_large_err,
        reason = "the no-alloc runtime returns the unconsumed owners by value"
    )]
    pub async fn install(
        &self,
        memory: BluetoothRadioMemory<
            LEGACY,
            CONNECTABLE,
            SCANNERS,
            CONNECTIONS,
            SCAN_PACKETS,
            RX_PACKETS,
        >,
        mut hardware: H,
    ) -> Result<
        BluetoothBackend<
            '_,
            Self,
            BluetoothDriver<
                '_,
                M,
                H,
                T,
                C,
                LEGACY,
                CONNECTABLE,
                SCANNERS,
                CONNECTIONS,
                SCAN_PACKETS,
                RX_PACKETS,
                ITEMS,
                EVENTS,
            >,
        >,
        (
            BluetoothInstallError,
            BluetoothRadioMemory<
                LEGACY,
                CONNECTABLE,
                SCANNERS,
                CONNECTIONS,
                SCAN_PACKETS,
                RX_PACKETS,
            >,
            H,
        ),
    > {
        if self.abandoned.load(Ordering::Acquire) {
            return Err((BluetoothInstallError::Abandoned, memory, hardware));
        }
        if self.occupied.load(Ordering::Acquire) {
            return Err((BluetoothInstallError::AlreadyInstalled, memory, hardware));
        }
        if hardware.scheduler_config() != C::SCHEDULER
            || hardware.local_sleep_clock_ppm() != C::LOCAL_SLEEP_CLOCK_PPM
        {
            return Err((BluetoothInstallError::TimingMismatch, memory, hardware));
        }
        if let Err(error) = hardware.publish_rx_chains(&memory.scanning, &memory.non_scanning) {
            return Err((BluetoothInstallError::RxChains(error), memory, hardware));
        }
        hardware.publish_device_table(memory.device_table.publication());
        let sample = match sample_time(&self.timer, &mut hardware).await {
            Ok(sample) => sample,
            Err(error) => return Err((BluetoothInstallError::Time(error), memory, hardware)),
        };
        let mut radio = BluetoothRadio::new(
            memory,
            hardware.scheduler_config(),
            hardware.controller_time_scale(),
            &sample,
            hardware.local_sleep_clock_ppm(),
        );
        radio.set_coexistence(self.coexistence());
        self.occupied.store(true, Ordering::Release);
        self.outcomes.reset();
        self.exhausted.store(false, Ordering::Release);
        self.fault.lock(|fault| fault.set(None));
        self.halt.lock(|halt| halt.set(None));
        self.request.reset();
        self.reply.reset();
        self.now_asked.store(0, Ordering::Release);
        self.now_answered.lock(|answered| answered.set((0, None)));
        self.work.signal(());
        Ok(BluetoothBackend::new(
            self,
            BluetoothDriver {
                runtime: self,
                installed: Installed {
                    radio,
                    hardware,
                    awaiting: None,
                    phy_route: None,
                    faulted: false,
                },
                fault: None,
                retired: None,
            },
        ))
    }

    /// Advance the radio time with `sample`, remembering an exhausted radio
    /// epoch: from then on the clock reports it and nothing is admitted.
    fn observe(
        &self,
        radio: &mut Radio<
            LEGACY,
            CONNECTABLE,
            SCANNERS,
            CONNECTIONS,
            SCAN_PACKETS,
            RX_PACKETS,
            ITEMS,
        >,
        sample: &ControllerTimeSample,
    ) -> Result<(), OutsideEpoch> {
        if self.exhausted.load(Ordering::Acquire) {
            return Err(OutsideEpoch);
        }
        let observed = radio.observe_time(sample);
        if observed.is_err() {
            self.exhausted.store(true, Ordering::Release);
        }
        observed
    }

    /// The image's monotonic time the transition bounds wait on.
    pub(crate) const fn timer(&self) -> &T {
        &self.timer
    }

    /// The port's [`Poisoned`] once the radio or the driver faulted.
    fn poisoned(&self) -> Option<Poisoned<BluetoothFault<H::StartError>>> {
        self.fault.lock(Cell::get).map(|cause| Poisoned { cause })
    }

    /// Send `request` to the driver and wait for its reply; a poisoned
    /// runtime answers at once.
    ///
    /// Cancel-safe: dropped before the driver took the request, the call
    /// withdraws it and nothing happens; dropped after, the driver serves
    /// it to the end, its effects stand (an admitted event's end reaches the
    /// event consumer), and its reply, numbered with the call's ticket, is
    /// discarded by the next call.
    async fn call(
        &self,
        request: Request<
            BluetoothCapabilities<
                C,
                LEGACY,
                CONNECTABLE,
                SCANNERS,
                CONNECTIONS,
                SCAN_PACKETS,
                RX_PACKETS,
                ITEMS,
            >,
        >,
    ) -> Result<Reply<BluetoothFault<H::StartError>>, Poisoned<BluetoothFault<H::StartError>>> {
        if let Some(poisoned) = self.poisoned() {
            return Err(poisoned);
        }
        debug_assert!(
            !self.request.signaled(),
            "the client and the owner never send two requests at once"
        );
        let ticket = self.tickets.fetch_add(1, Ordering::AcqRel).wrapping_add(1);
        self.request.signal((ticket, request));
        self.work.signal(());
        // Dropped before the driver takes the request, this withdraws it;
        // once taken, nothing is left to withdraw.
        let _withdraw = Withdraw {
            request: &self.request,
            ticket,
        };
        let reply = loop {
            let (answered, reply) = self.reply.wait().await;
            // The reply of an earlier, dropped call: its effects stand.
            if answered == ticket {
                break reply;
            }
        };
        match reply {
            Reply::Poisoned(poisoned) => Err(poisoned),
            reply => Ok(reply),
        }
    }

    /// Admit one request against a fresh controller-time sample: the phase
    /// client's submission.
    pub(crate) async fn submit_request(
        &self,
        request: RadioRequest<
            '_,
            BluetoothCapabilities<
                C,
                LEGACY,
                CONNECTABLE,
                SCANNERS,
                CONNECTIONS,
                SCAN_PACKETS,
                RX_PACKETS,
                ITEMS,
            >,
        >,
    ) -> PortResult<(), RequestError, BluetoothFault<H::StartError>> {
        match self.call(Request::Submit(request.to_owned())).await? {
            Reply::Submit(answer) => answer,
            _ => unreachable!("the driver answers a submission"),
        }
    }

    /// A fresh radio time: the port's clock, from a sample the driver took
    /// after this reading was asked. Readers beside each other share it.
    pub(crate) async fn read_now(
        &self,
    ) -> PortResult<LeInstant, ClockError, BluetoothFault<H::StartError>> {
        if let Some(poisoned) = self.poisoned() {
            return Err(poisoned);
        }
        let ticket = self
            .now_asked
            .fetch_add(1, Ordering::AcqRel)
            .wrapping_add(1);
        self.work.signal(());
        core::future::poll_fn(|cx| {
            let (answered, answer) = self.now_answered.lock(Cell::get);
            // Tickets wrap; one answered at or after this one serves it.
            if let Some(answer) = answer
                && answered.wrapping_sub(ticket) < u32::MAX / 2
            {
                return Poll::Ready(answer);
            }
            if let Some(poisoned) = self.poisoned() {
                return Poll::Ready(Err(poisoned));
            }
            self.now_waker
                .lock(|waker| waker.borrow_mut().register(cx.waker()));
            Poll::Pending
        })
        .await
    }

    /// Wake the readers waiting for a clock sample.
    fn wake_readers(&self) {
        self.now_waker.lock(|waker| waker.borrow_mut().wake());
    }

    /// Withdraw one scheduled event: the phase client's cancellation.
    pub(crate) async fn cancel_event(
        &self,
        id: EventId,
    ) -> PortResult<(), CancelError, BluetoothFault<H::StartError>> {
        match self.call(Request::Cancel(id)).await? {
            Reply::Cancel(answer) => answer,
            _ => unreachable!("the driver answers a cancellation"),
        }
    }

    /// Enable the port within [`TRANSITION_BOUND`]: the driver resumes the
    /// scheduler a `Disable` stopped and opens admission. A failed resume,
    /// or one past the bound, poisons the runtime.
    pub(crate) async fn enable(
        &self,
    ) -> EnableResult<core::convert::Infallible, BluetoothFault<H::StartError>> {
        self.bounded(oer_bluetooth_trace::LifecycleCommand::Enable, async {
            match self.call(Request::Enable).await? {
                Reply::Transition(answer) => answer,
                _ => unreachable!("the driver answers a transition"),
            }
        })
        .await
        .map(Ok)
    }

    /// Disable the port within [`TRANSITION_BOUND`]: the driver stops the
    /// scheduler, withdraws every admitted event
    /// ([`BluetoothRadio::cancel_all_stopped`]) against a sample taken after
    /// the stop and settles the withdrawals; every outcome the phase left,
    /// each admitted event's end among them, then goes to `sink`. A failed
    /// stop, an event left without its end or a disable past the bound
    /// poisons the runtime.
    pub(crate) async fn disable(
        &self,
        sink: &mut dyn FnMut(BluetoothOutcome),
    ) -> Result<(), Poisoned<BluetoothFault<H::StartError>>> {
        self.bounded(oer_bluetooth_trace::LifecycleCommand::Disable, async {
            match self.call(Request::Disable).await? {
                Reply::Transition(answer) => answer?,
                _ => unreachable!("the driver answers a transition"),
            }
            // A loss stands only for reports the phase's consumer did not
            // take; the sink takes what is left.
            while let Some(outcome) = self.outcomes.take() {
                if let Ok(outcome) = outcome {
                    sink(outcome);
                }
            }
            Ok(())
        })
        .await
    }

    /// Run `transition` within [`TRANSITION_BOUND`], tracing its start and
    /// how it ended; past the bound the runtime is poisoned and its driver
    /// halts.
    async fn bounded(
        &self,
        command: oer_bluetooth_trace::LifecycleCommand,
        transition: impl Future<Output = Result<(), Poisoned<BluetoothFault<H::StartError>>>>,
    ) -> Result<(), Poisoned<BluetoothFault<H::StartError>>> {
        use oer_bluetooth_trace::{Lifecycle, LifecycleStep};
        crate::trace::emit(|| Lifecycle {
            command,
            step: LifecycleStep::Started,
        });
        let (ended, step) = match select(transition, wait_for(&self.timer, TRANSITION_BOUND)).await
        {
            Either::First(Ok(())) => (Ok(()), LifecycleStep::Done),
            Either::First(Err(poisoned)) => (Err(poisoned), LifecycleStep::Poisoned),
            Either::Second(waited) => {
                self.halt(match waited {
                    Ok(()) => BluetoothRuntimeFault::TransitionTimedOut,
                    Err(error) => BluetoothRuntimeFault::Time(error),
                });
                (
                    Err(self.poisoned().expect("the runtime is poisoned")),
                    LifecycleStep::TimedOut,
                )
            }
        };
        crate::trace::emit(|| Lifecycle { command, step });
        ended
    }

    /// Stop the runtime at once, synchronously: poison it, halt its driver
    /// at its next pass and refuse any install until a reset. A dropped or
    /// fail-stopped port owner calls it ([`oer_bluetooth_port::PortBackend::abandon`]);
    /// the radio and its hardware stay unreachable, and the composition
    /// keeps the interrupt routes and power owners fail-stop.
    pub fn abandon(&self) {
        self.abandoned.store(true, Ordering::Release);
        self.halt(BluetoothRuntimeFault::Abandoned);
    }

    /// Poison the runtime after the platform beside it failed (an interrupt
    /// service or the timer task), so the port's body sees the fault at its
    /// next call and stops.
    pub fn platform_fault(&self) {
        self.fault.lock(|cause| {
            if cause.get().is_none() {
                cause.set(Some(BluetoothFault::Platform));
            }
        });
        self.changed.signal(());
    }

    /// Poison the runtime with `fault` and halt its driver at its next pass.
    fn halt(&self, fault: BluetoothRuntimeFault<H::StartError>) {
        self.halt.lock(|halt| {
            if halt.get().is_none() {
                halt.set(Some(fault));
            }
        });
        self.poison(fault);
        self.work.signal(());
    }

    /// The platform's scheduler interrupt published a wake for the worker.
    pub fn on_scheduler_wake(&self) {
        self.work.signal(());
    }

    /// Take the next outcome: the queued ones in their order, then the
    /// [`Poisoned`] of a poisoned runtime.
    pub(crate) async fn wait_event(
        &self,
    ) -> PortResult<BluetoothOutcome, EventsLost, BluetoothFault<H::StartError>> {
        loop {
            if let Some(outcome) = self.outcomes.take() {
                return Ok(outcome);
            }
            if let Some(poisoned) = self.poisoned() {
                return Err(poisoned);
            }
            select(
                self.outcomes.entries.ready_to_receive(),
                self.changed.wait(),
            )
            .await;
        }
    }

    /// Poison the runtime after a fault of the driver, unless the radio
    /// faulted first.
    fn poison(&self, fault: BluetoothRuntimeFault<H::StartError>) {
        self.fault.lock(|cause| {
            if cause.get().is_none() {
                cause.set(Some(BluetoothFault::Runner(fault)));
            }
        });
        self.changed.signal(());
    }

    /// Follow whether another radio shares the antenna. The driver applies
    /// the matching coexistence profile at its next pass, and every later
    /// installation starts with it.
    pub fn set_coexistence(&self, shared: bool) {
        self.shared.store(shared, Ordering::Release);
        self.work.signal(());
    }

    fn coexistence(&self) -> CoexistenceProfile {
        if self.shared.load(Ordering::Acquire) {
            CoexistenceProfile::Shared
        } else {
            CoexistenceProfile::Standalone
        }
    }

    /// Publish the roles the Controller has active. Only the latest summary
    /// is kept for [`Self::next_activity`].
    pub fn publish_activity(&self, activity: RadioActivity) {
        self.activity.signal(activity);
    }

    /// Forget an unread summary; the next epoch starts with no role active.
    pub fn clear_activity(&self) {
        self.activity.reset();
    }

    /// Wait for the next change of the active roles; a burst of changes
    /// yields only the latest.
    pub async fn next_activity(&self) -> RadioActivity {
        self.activity.wait().await
    }
}

impl<
    M: RawMutex,
    H: BluetoothRadioHardware,
    T: Timer,
    C: BluetoothRadioConfig,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
    const EVENTS: usize,
>
    BluetoothDriver<
        '_,
        M,
        H,
        T,
        C,
        LEGACY,
        CONNECTABLE,
        SCANNERS,
        CONNECTIONS,
        SCAN_PACKETS,
        RX_PACKETS,
        ITEMS,
        EVENTS,
    >
{
    /// Drive the radio and serve the port's requests until `retiring`
    /// asks for the uninstall, which it then prepares ([`Self::into_parts`]).
    ///
    /// Between passes it serves the one request in flight, and yields after
    /// each reply so the requester goes on first; a pass that continues
    /// yields after a fixed budget, so the body beside it makes progress.
    /// It returns early on a hardware fault or a halt (a transition past its
    /// bound, an abandoned owner), which poisoned the runtime first; it is
    /// cancel-safe, since every step's progress lives in the radio and the
    /// hardware, and the next pass picks it up.
    ///
    /// # Errors
    ///
    /// The fault that stopped it.
    pub(crate) async fn run(
        &mut self,
        retiring: impl Fn() -> bool,
    ) -> Result<(), BluetoothRuntimeFault<H::StartError>> {
        let runtime = self.runtime;
        let mut budget = ContinueBudget::new();
        loop {
            if let Some(fault) = runtime.halt.lock(Cell::get).or(self.fault) {
                return Err(self.stop(fault));
            }
            if retiring() {
                let retired = self.retire().await;
                let stopped = match retired {
                    Err(BluetoothUninstallError::Fault(fault)) => Err(fault),
                    _ => Ok(()),
                };
                self.retired = Some(retired);
                return stopped;
            }
            let asked = runtime.now_asked.load(Ordering::Acquire);
            if runtime.now_answered.lock(Cell::get).0 != asked {
                let answer = self.now().await;
                runtime
                    .now_answered
                    .lock(|answered| answered.set((asked, Some(answer))));
                runtime.wake_readers();
                budget.refill();
                embassy_futures::yield_now().await;
                continue;
            }
            if let Some((ticket, request)) = runtime.request.try_take() {
                let reply = self.serve(request).await;
                runtime.reply.signal((ticket, reply));
                // The requester goes on first.
                budget.refill();
                embassy_futures::yield_now().await;
                continue;
            }
            self.installed.radio.set_coexistence(runtime.coexistence());
            let installed = &mut self.installed;
            if !installed.faulted {
                match sample_time(&runtime.timer, &mut installed.hardware).await {
                    // Exhaustion admits nothing new; the driver still
                    // settles the admitted work.
                    Ok(sample) => {
                        let _exhausted = runtime.observe(&mut installed.radio, &sample);
                    }
                    Err(BluetoothTimeError::Deadline) => {
                        let fault = BluetoothRuntimeFault::Time(BluetoothTimeError::Deadline);
                        runtime.poison(fault);
                        return Err(self.stop(fault));
                    }
                    Err(_) => {}
                }
            }
            let pass = match runtime.pass(installed, &mut runtime.sink()) {
                // A test event is published only on an idle scheduler, and a
                // listed one leaves its list only after a stop; resuming
                // restarts at the remaining events.
                Ok(Pass::Stop) => match runtime.restart_scheduler(installed).await {
                    Ok(()) => Pass::Continue,
                    Err(fault) => {
                        runtime.poison(fault);
                        return Err(self.stop(fault));
                    }
                },
                Ok(pass) => pass,
                Err(fault) => {
                    runtime.poison(fault);
                    return Err(self.stop(fault));
                }
            };
            let wait = match pass {
                Pass::Stop | Pass::Continue => {
                    budget.spend().await;
                    continue;
                }
                Pass::Recheck => HARDWARE_RECHECK,
                Pass::Idle => TIME_REFRESH,
            };
            budget.refill();
            // A retirement, a request, a wake or the deadline ends the wait;
            // the retire request registers no waker, but its owner polls the
            // driver right after it asks.
            let retire = core::future::poll_fn(|_| {
                if retiring() {
                    Poll::Ready(())
                } else {
                    Poll::Pending
                }
            });
            if let Either3::Second(Err(error)) =
                select3(runtime.work.wait(), wait_for(&runtime.timer, wait), retire).await
            {
                let fault = BluetoothRuntimeFault::Time(error);
                runtime.poison(fault);
                return Err(self.stop(fault));
            }
        }
    }

    /// Stop driving after `fault`, which poisoned the runtime: answer the
    /// request in flight and keep the radio faulted.
    fn stop(
        &mut self,
        fault: BluetoothRuntimeFault<H::StartError>,
    ) -> BluetoothRuntimeFault<H::StartError> {
        self.fault = Some(fault);
        self.installed.fault();
        if let Some((ticket, _)) = self.runtime.request.try_take() {
            let poisoned = self.runtime.poisoned().expect("the runtime is poisoned");
            self.runtime
                .reply
                .signal((ticket, Reply::Poisoned(poisoned)));
        }
        self.runtime.wake_readers();
        fault
    }

    /// Serve one request of the port.
    async fn serve(
        &mut self,
        request: Request<
            BluetoothCapabilities<
                C,
                LEGACY,
                CONNECTABLE,
                SCANNERS,
                CONNECTIONS,
                SCAN_PACKETS,
                RX_PACKETS,
                ITEMS,
            >,
        >,
    ) -> Reply<BluetoothFault<H::StartError>> {
        match request {
            Request::Submit(request) => Reply::Submit(self.submit(&request).await),
            Request::Cancel(id) => Reply::Cancel(self.cancel(id)),
            Request::Enable => Reply::Transition(self.enable().await),
            Request::Disable => Reply::Transition(self.disable().await),
        }
    }

    /// Admit one request against a fresh controller-time sample. An event
    /// reserves the slots of its terminal outcomes first.
    async fn submit(
        &mut self,
        request: &OwnedRadioRequest<
            BluetoothCapabilities<
                C,
                LEGACY,
                CONNECTABLE,
                SCANNERS,
                CONNECTIONS,
                SCAN_PACKETS,
                RX_PACKETS,
                ITEMS,
            >,
        >,
    ) -> PortResult<(), RequestError, BluetoothFault<H::StartError>> {
        let runtime = self.runtime;
        if let Some(poisoned) = runtime.poisoned() {
            return Err(poisoned);
        }
        debug_assert_eq!(
            runtime.outcomes.admission(),
            Admission::Enabled,
            "a client exists only while the owner is enabled"
        );
        let installed = &mut self.installed;
        let Ok(sample) = sample_time(&runtime.timer, &mut installed.hardware).await else {
            return Ok(Err(RequestError::ClockUnavailable));
        };
        if runtime.observe(&mut installed.radio, &sample).is_err() {
            return Ok(Err(RequestError::EpochExhausted));
        }
        let request = request.as_request();
        // An event reserves its terminal outcomes; a connection event also
        // every data PDU its receive chain holds, which the hardware
        // acknowledges to the peer.
        let event = match &request {
            RadioRequest::Advertise(event) => Some((event.get().id, 0)),
            RadioRequest::Scan(window) => Some((window.get().id, 0)),
            RadioRequest::ConnectionEvent(event) => Some((event.get().id, RX_PACKETS)),
            RadioRequest::TestTransmit(test) => Some((test.get().id, 0)),
            RadioRequest::TestReceive(test) => Some((test.get().id, 0)),
            _ => None,
        };
        if let Some((id, data)) = event
            && !runtime.outcomes.admit_event(id, data)
        {
            return Ok(Err(RequestError::Busy));
        }
        let starts_scanner = matches!(request, RadioRequest::ConfigureScanner(_));
        let changes_list = matches!(request, RadioRequest::FilterAcceptList(_));
        match installed.radio.request(request, &mut runtime.sink()) {
            Ok(Ok(())) => {}
            Ok(Err(error)) => {
                if let Some((id, _)) = event {
                    runtime.outcomes.withdraw_event(id);
                }
                return Ok(Err(error));
            }
            // The radio reported its fault to the sink, which poisoned the
            // runtime.
            Err(RadioFaulted) => {
                if let Some((id, _)) = event {
                    runtime.outcomes.withdraw_event(id);
                }
                installed.fault();
                return Err(runtime
                    .poisoned()
                    .expect("a radio fault poisons the runtime"));
            }
        }
        if starts_scanner {
            installed.hardware.publish_scan_start();
        }
        if changes_list {
            let publication = installed.radio.device_table_publication();
            installed.hardware.publish_device_table(publication);
        }
        Ok(Ok(()))
    }

    /// A fresh radio time.
    async fn now(&mut self) -> PortResult<LeInstant, ClockError, BluetoothFault<H::StartError>> {
        let runtime = self.runtime;
        if let Some(poisoned) = runtime.poisoned() {
            return Err(poisoned);
        }
        let installed = &mut self.installed;
        let Ok(sample) = sample_time(&runtime.timer, &mut installed.hardware).await else {
            return Ok(Err(ClockError::Unavailable));
        };
        if runtime.observe(&mut installed.radio, &sample).is_err() {
            return Ok(Err(ClockError::EpochExhausted));
        }
        Ok(Ok(installed.radio.now()))
    }

    /// Withdraw one scheduled event.
    fn cancel(
        &mut self,
        id: EventId,
    ) -> PortResult<(), CancelError, BluetoothFault<H::StartError>> {
        let runtime = self.runtime;
        if let Some(poisoned) = runtime.poisoned() {
            return Err(poisoned);
        }
        Ok(self.installed.radio.cancel(id, &mut runtime.sink()))
    }

    /// Resume the scheduler a `Disable` stopped and open admission.
    async fn enable(&mut self) -> Result<(), Poisoned<BluetoothFault<H::StartError>>> {
        let runtime = self.runtime;
        if let Some(poisoned) = runtime.poisoned() {
            return Err(poisoned);
        }
        let installed = &mut self.installed;
        if installed.radio.stopped().is_some()
            && let Err(fault) = runtime.resume_installed(installed).await
        {
            installed.fault();
            runtime.poison(fault);
            return Err(runtime.poisoned().expect("the runtime is poisoned"));
        }
        runtime.outcomes.open();
        Ok(())
    }

    /// Stop the scheduler, withdraw every admitted event and settle the
    /// withdrawals: every admitted event ends into the queue.
    async fn disable(&mut self) -> Result<(), Poisoned<BluetoothFault<H::StartError>>> {
        let runtime = self.runtime;
        if let Some(poisoned) = runtime.poisoned() {
            return Err(poisoned);
        }
        let installed = &mut self.installed;
        if let Err(fault) = runtime.withdraw_stopped(installed).await {
            installed.fault();
            runtime.poison(fault);
            return Err(runtime.poisoned().expect("the runtime is poisoned"));
        }
        if !runtime.outcomes.close() {
            installed.fault();
            runtime.poison(BluetoothRuntimeFault::Unsettled);
            return Err(runtime.poisoned().expect("the runtime is poisoned"));
        }
        Ok(())
    }

    /// Leave the radio for the uninstall.
    ///
    /// The owner's `disable` already ended every admitted event and stopped
    /// the scheduler; a radio whose owner never enabled it has its scheduler
    /// stopped here, after a list transaction in progress finishes. A
    /// controller-time request in flight is drained, and the BLE PHY route
    /// leaves as initialization left it.
    async fn retire(&mut self) -> Result<(), BluetoothUninstallError<H::StartError>> {
        let runtime = self.runtime;
        if runtime.outcomes.admission() != Admission::Disabled {
            return Err(BluetoothUninstallError::NotDisabled);
        }
        let installed = &mut self.installed;
        if installed.radio.stopped().is_none()
            && let Err(fault) = runtime.stop_scheduler(installed, &mut runtime.sink()).await
        {
            runtime.poison(fault);
            return Err(BluetoothUninstallError::Fault(self.stop(fault)));
        }
        loop {
            match installed.hardware.drain_time() {
                Ok(ControllerTimeEventStep::Waiting) => {
                    if let Err(error) = wait_for(&runtime.timer, HARDWARE_RECHECK).await {
                        let fault = BluetoothRuntimeFault::Time(error);
                        runtime.poison(fault);
                        return Err(BluetoothUninstallError::Fault(self.stop(fault)));
                    }
                }
                Ok(_) => break,
                Err(error) => {
                    let fault = BluetoothRuntimeFault::Time(BluetoothTimeError::Event(error));
                    runtime.poison(fault);
                    return Err(BluetoothUninstallError::Fault(self.stop(fault)));
                }
            }
        }
        installed.restore_phy_route();
        Ok(())
    }

    /// Take the radio and its hardware out of the driver; the runtime is
    /// free for the next install, which starts disabled with an empty queue.
    ///
    /// # Errors
    ///
    /// The driver did not leave the radio for the uninstall: it stopped on
    /// a fault, or its retirement found the port enabled
    /// ([`BluetoothUninstallError::NotDisabled`]: the owner was poisoned
    /// while enabled, and admitted events may still owe their ends). The
    /// radio and hardware come back with the error, for the composition's
    /// fail-stop; the runtime stays occupied.
    #[allow(
        clippy::type_complexity,
        clippy::result_large_err,
        reason = "the role's pool capacities stay visible in the returned radio"
    )]
    pub fn into_parts(
        self,
    ) -> Result<
        (
            Radio<LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS, ITEMS>,
            H,
        ),
        BluetoothRetained<
            H,
            LEGACY,
            CONNECTABLE,
            SCANNERS,
            CONNECTIONS,
            SCAN_PACKETS,
            RX_PACKETS,
            ITEMS,
        >,
    > {
        let retired = match (self.retired, self.fault) {
            (Some(retired), _) => retired,
            (None, Some(fault)) => Err(BluetoothUninstallError::Fault(fault)),
            (None, None) => Err(BluetoothUninstallError::NotRetired),
        };
        let Installed {
            radio, hardware, ..
        } = self.installed;
        match retired {
            Ok(()) => {
                self.runtime.outcomes.reset();
                self.runtime.occupied.store(false, Ordering::Release);
                Ok((radio, hardware))
            }
            Err(error) => Err(BluetoothRetained {
                error,
                radio,
                hardware,
            }),
        }
    }

    /// Run `entry` on the installed radio; for host tests.
    #[cfg(test)]
    pub(crate) fn with_radio<R>(
        &mut self,
        entry: impl FnOnce(
            &mut Radio<LEGACY, CONNECTABLE, SCANNERS, CONNECTIONS, SCAN_PACKETS, RX_PACKETS, ITEMS>,
        ) -> R,
    ) -> R {
        entry(&mut self.installed.radio)
    }
}

impl<
    M: RawMutex,
    H: BluetoothRadioHardware,
    T: Timer,
    C: BluetoothRadioConfig,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
    const EVENTS: usize,
>
    BluetoothRuntime<
        M,
        H,
        T,
        C,
        LEGACY,
        CONNECTABLE,
        SCANNERS,
        CONNECTIONS,
        SCAN_PACKETS,
        RX_PACKETS,
        ITEMS,
        EVENTS,
    >
{
    /// Stop the scheduler unless it is stopped, withdraw every scheduled
    /// event against a sample taken after the stop, and settle the
    /// withdrawals: every admitted event ends.
    async fn withdraw_stopped(
        &self,
        installed: &mut Installed<
            H,
            LEGACY,
            CONNECTABLE,
            SCANNERS,
            CONNECTIONS,
            SCAN_PACKETS,
            RX_PACKETS,
            ITEMS,
        >,
    ) -> Result<(), BluetoothRuntimeFault<H::StartError>> {
        if installed.radio.stopped().is_none() {
            self.stop_scheduler(installed, &mut self.sink()).await?;
        }
        let sample = sample_time(&self.timer, &mut installed.hardware)
            .await
            .map_err(BluetoothRuntimeFault::Time)?;
        installed
            .radio
            .cancel_all_stopped(&sample, &mut self.sink())
            .map_err(|_| BluetoothRuntimeFault::Stop)?;
        // The stopped scheduler takes every withdrawal on its idle path.
        loop {
            match self.pass(installed, &mut self.sink())? {
                Pass::Idle => return Ok(()),
                Pass::Continue => {}
                Pass::Recheck => wait_for(&self.timer, HARDWARE_RECHECK)
                    .await
                    .map_err(BluetoothRuntimeFault::Time)?,
                // A stopped scheduler asks for no stop.
                Pass::Stop => return Err(BluetoothRuntimeFault::Stop),
            }
        }
    }

    /// Resume the scheduler a stop left stopped, against a fresh sample.
    async fn resume_installed(
        &self,
        installed: &mut Installed<
            H,
            LEGACY,
            CONNECTABLE,
            SCANNERS,
            CONNECTIONS,
            SCAN_PACKETS,
            RX_PACKETS,
            ITEMS,
        >,
    ) -> Result<(), BluetoothRuntimeFault<H::StartError>> {
        let sample = sample_time(&self.timer, &mut installed.hardware)
            .await
            .map_err(BluetoothRuntimeFault::Time)?;
        // Exhaustion admits nothing new; the scheduler still resumes.
        let _exhausted = self.observe(&mut installed.radio, &sample);
        installed
            .radio
            .resume(&sample)
            .map_err(|_| BluetoothRuntimeFault::Stop)
    }

    /// Stop the scheduler and resume it at once, unless a disabled port
    /// keeps it stopped until `Enable`.
    async fn restart_scheduler(
        &self,
        installed: &mut Installed<
            H,
            LEGACY,
            CONNECTABLE,
            SCANNERS,
            CONNECTIONS,
            SCAN_PACKETS,
            RX_PACKETS,
            ITEMS,
        >,
    ) -> Result<(), BluetoothRuntimeFault<H::StartError>> {
        if installed.radio.stopped().is_some() {
            return Ok(());
        }
        self.stop_scheduler(installed, &mut self.sink()).await?;
        // The radio stays stopped, and the runtime faulted, without a sample
        // to judge which events have passed.
        let sample = sample_time(&self.timer, &mut installed.hardware)
            .await
            .map_err(BluetoothRuntimeFault::Time)?;
        // Exhaustion admits nothing new; the scheduler still resumes to
        // settle the admitted work.
        let _exhausted = self.observe(&mut installed.radio, &sample);
        installed
            .radio
            .resume(&sample)
            .expect("the radio holds the stopped receipt");
        Ok(())
    }

    /// Finish the list transaction in progress, stop the scheduler, report
    /// what it finished and hand the stopped receipt to the radio.
    async fn stop_scheduler(
        &self,
        installed: &mut Installed<
            H,
            LEGACY,
            CONNECTABLE,
            SCANNERS,
            CONNECTIONS,
            SCAN_PACKETS,
            RX_PACKETS,
            ITEMS,
        >,
        sink: &mut impl BluetoothRadioSink,
    ) -> Result<(), BluetoothRuntimeFault<H::StartError>> {
        while installed.awaiting.is_some() {
            if let Pass::Recheck = self.pass(installed, sink)? {
                wait_for(&self.timer, HARDWARE_RECHECK)
                    .await
                    .map_err(BluetoothRuntimeFault::Time)?;
            }
        }
        let mut stop = BluetoothSchedulerStop::default();
        let stopped = loop {
            match installed.hardware.step_stop(stop) {
                Ok(BluetoothSchedulerStopStep::Stopped(stopped)) => break stopped,
                Ok(BluetoothSchedulerStopStep::Pending(pending)) => {
                    stop = pending;
                    wait_for(&self.timer, HARDWARE_RECHECK)
                        .await
                        .map_err(BluetoothRuntimeFault::Time)?;
                }
                Err(error) => return Err(BluetoothRuntimeFault::StopSequence(error)),
            }
        };
        // Drain a prior captured observation before transferring the final
        // stopped snapshot. No finished-list bit may be silently ignored.
        drain_finished_lists(installed, sink);
        installed
            .hardware
            .capture_stopped_finished_lists(&stopped)
            .map_err(BluetoothRuntimeFault::FinishedLists)?;
        drain_finished_lists(installed, sink);
        installed
            .radio
            .enter_stopped(stopped)
            .map_err(|_| BluetoothRuntimeFault::Stop)
    }

    fn sink(&self) -> QueueSink<'_, M, H::StartError, EVENTS, CONNECTIONS> {
        QueueSink {
            outcomes: &self.outcomes,
            fault: &self.fault,
            changed: &self.changed,
        }
    }

    /// One pass of the driver: report completions, then advance the transaction in
    /// progress or drive the next hardware work.
    fn pass(
        &self,
        installed: &mut Installed<
            H,
            LEGACY,
            CONNECTABLE,
            SCANNERS,
            CONNECTIONS,
            SCAN_PACKETS,
            RX_PACKETS,
            ITEMS,
        >,
        sink: &mut impl BluetoothRadioSink,
    ) -> Result<Pass, BluetoothRuntimeFault<H::StartError>> {
        drain_finished_lists(installed, sink);
        if let Some(wake) = installed.hardware.take_wake() {
            installed
                .hardware
                .capture_finished_lists(wake)
                .map_err(BluetoothRuntimeFault::FinishedLists)?;
            drain_finished_lists(installed, sink);
        }
        if installed.faulted {
            return Ok(Pass::Idle);
        }
        let step = match installed.awaiting {
            Some(wait) => {
                let observation = installed
                    .hardware
                    .observe_wait(wait)
                    .map_err(BluetoothRuntimeFault::Scheduler)?;
                match installed.radio.advance(observation, sink) {
                    Ok(step) => step,
                    Err(fault) => {
                        installed.hardware.recover(fault);
                        installed.awaiting = None;
                        return Ok(Pass::Idle);
                    }
                }
            }
            None => {
                let view = installed
                    .hardware
                    .observe()
                    .map_err(BluetoothRuntimeFault::Scheduler)?;
                installed.radio.drive(view, sink)
            }
        };
        match step {
            RadioStep::Idle => Ok(Pass::Idle),
            RadioStep::Start(insertion) => {
                installed
                    .hardware
                    .start(&installed.radio.item_space(), insertion)
                    .map_err(BluetoothRuntimeFault::Start)?;
                Ok(Pass::Continue)
            }
            RadioStep::StopScheduler => Ok(Pass::Stop),
            RadioStep::EnterTest(insertion) => {
                let route = installed.hardware.disable_phy_etm_route();
                installed.phy_route = Some(route);
                installed
                    .hardware
                    .start(&installed.radio.item_space(), insertion)
                    .map_err(BluetoothRuntimeFault::Start)?;
                Ok(Pass::Continue)
            }
            RadioStep::LeaveTest => {
                installed.restore_phy_route();
                Ok(Pass::Continue)
            }
            RadioStep::Transaction(step) => {
                installed
                    .hardware
                    .perform(&installed.radio.item_space(), &step)
                    .map_err(BluetoothRuntimeFault::Scheduler)?;
                match step.next() {
                    SchedulerNext::Finished => {
                        installed.awaiting = None;
                        Ok(Pass::Continue)
                    }
                    SchedulerNext::Await(wait) => {
                        installed.awaiting = Some(wait);
                        Ok(if step.actions().next().is_some() {
                            Pass::Continue
                        } else {
                            Pass::Recheck
                        })
                    }
                }
            }
        }
    }
}

/// Report the events of every captured finished list zero.
fn drain_finished_lists<
    H: BluetoothRadioHardware,
    const LEGACY: usize,
    const CONNECTABLE: usize,
    const SCANNERS: usize,
    const CONNECTIONS: usize,
    const SCAN_PACKETS: usize,
    const RX_PACKETS: usize,
    const ITEMS: usize,
>(
    installed: &mut Installed<
        H,
        LEGACY,
        CONNECTABLE,
        SCANNERS,
        CONNECTIONS,
        SCAN_PACKETS,
        RX_PACKETS,
        ITEMS,
    >,
    sink: &mut impl BluetoothRadioSink,
) {
    loop {
        match installed.hardware.next_finished_list() {
            SchedulerFinishedListWorkerStep::Idle | SchedulerFinishedListWorkerStep::Complete => {
                return;
            }
            SchedulerFinishedListWorkerStep::List { observed, more } => {
                if observed.index() == BluetoothSchedulerHardwareListIndex::ZERO {
                    installed.radio.complete(sink);
                }
                if !more {
                    return;
                }
            }
        }
    }
}

/// Take one controller-time sample, draining a request a cancelled caller
/// abandoned first.
/// Consecutive passes the runner may continue before it yields.
const CONTINUE_BUDGET: u8 = 16;

/// Bounds how long the runner keeps the executor without awaiting.
///
/// A pass that continues does not await, and its time sample awaits only
/// while the latch is pending. After a fixed number of such passes the
/// runner yields once, so sibling tasks on the same executor always make
/// progress.
struct ContinueBudget(u8);

impl ContinueBudget {
    const fn new() -> Self {
        Self(CONTINUE_BUDGET)
    }

    /// Account one continued pass and yield when the budget is spent.
    async fn spend(&mut self) {
        self.0 -= 1;
        if self.0 == 0 {
            self.refill();
            embassy_futures::yield_now().await;
        }
    }

    /// Start a new budget after the runner awaited.
    fn refill(&mut self) {
        self.0 = CONTINUE_BUDGET;
    }
}

/// Wait for `duration` on `timer`; a deadline past the timer's range never
/// ends.
/// Wait `duration` on `timer`.
///
/// # Errors
///
/// [`BluetoothTimeError::Deadline`] when the timer cannot express the
/// deadline; the caller faults instead of waiting forever.
pub(crate) async fn wait_for(
    timer: &impl Timer,
    duration: Duration,
) -> Result<(), BluetoothTimeError> {
    timer
        .wait_for(duration)
        .await
        .map_err(|_| BluetoothTimeError::Deadline)
}

async fn sample_time(
    timer: &impl Timer,
    hardware: &mut impl BluetoothRadioHardware,
) -> Result<ControllerTimeSample, BluetoothTimeError> {
    let request = match hardware.request_time() {
        Ok(request) => request,
        Err(ControllerTimeRequestError::Busy) => {
            while let ControllerTimeEventStep::Waiting =
                hardware.drain_time().map_err(BluetoothTimeError::Event)?
            {
                wait_for(timer, HARDWARE_RECHECK).await?;
            }
            hardware
                .request_time()
                .map_err(BluetoothTimeError::Request)?
        }
        Err(error) => return Err(BluetoothTimeError::Request(error)),
    };
    loop {
        match hardware
            .recheck_time(request)
            .map_err(BluetoothTimeError::Event)?
        {
            ControllerTimeEventStep::Sample { sample, .. } => return Ok(sample),
            ControllerTimeEventStep::Waiting => wait_for(timer, HARDWARE_RECHECK).await?,
            ControllerTimeEventStep::Idle | ControllerTimeEventStep::OrphanDrained => {
                return Err(BluetoothTimeError::Lost);
            }
        }
    }
}

#[cfg(test)]
mod tests;
