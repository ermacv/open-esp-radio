//! The runtime on the IEEE 802.15.4 owner shape ([`oer_i::owner`]):
//! [`IRuntimeBackend`] is the `PortBackend` a composition hands to
//! [`run_port`](oer_i_port::run_port).
//!
//! The engine stays in the runtime's blocking mutex, shared with the MAC
//! interrupt entry: an interim critical section the owner allowed until
//!  moves the engine into the interrupt context. The joined driver
//! ([`PortBackend::drive`]) runs the CSMA-CA backoff and the retry delays
//! that the runner beside the consumer ran before.
//!
//! - The one operation permit lives in the held part of the install
//!   ([`RuntimeHeld`]): a client awaits it on its interface's seat, a
//!   submission moves it in, and the events consumer gives it back when it
//!   takes the operation's terminal, before it hands the event out.
//! - Receive and sleep are per-interface wishes, also in the held part. The
//!   radio receives on the one channel every receiving interface shares;
//!   a receive on another channel waits until those interfaces sleep. A
//!   wish made while an operation runs takes effect when it ends.
//! - A scheduled receive window is a wish too. It takes the radio's resting
//!   state on its channel and owes no event; the runtime marks the window
//!   ended when the engine reports it, and returns to the receive union.
//! - A radio fault poisons the port: the consumer reports the events before
//!   it, then `Poisoned` at every call.
//!
//! The engine's pending table keeps one entry per address. A slot of the
//! owner shape stands for one entry; adding an address twice through two
//! slots keeps one entry, which the first removal clears.

use core::{
    cell::{Cell, RefCell},
    future::{Future, poll_fn, ready},
    marker::PhantomData,
    task::{Poll, Waker},
};

use embassy_futures::select::select;
use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_espressif_ieee802154_engine::ll::Ieee802154LowLevel;
use oer_espressif_ieee802154_radio::Ieee802154Random;
use oer_ieee802154::{
    CcaMode, Channel, CommandError, Configuration, CslReceiver, EnhancedAckGeneration,
    FrameAddress, FrameCounterUpdate, Ieee802154Instant, Interface, InterfaceSetting, MacKeys,
    ProbingInitiator, RadioCommand, RadioFault, RadioSetting, RadioState, RequestId,
    ScheduledReceiveRequest, TxRequest,
    mac::pending::AutoPendingMode,
    owner::{
        AutomaticAcknowledgement, ClearChannelAssessment, ConfiguredInterface, ExtendedPending,
        Has, Ieee802154Backend, Ieee802154Caps, Ieee802154Client, Ieee802154Clients,
        Ieee802154Control, Ieee802154Events, Ieee802154Setup, InterfaceKind, Operation,
        OperationPermit, OperationPermits, PhaseEvent, Promiscuous, RadioSettings, RecentRssi,
        ShortPending, SourceMatch, SupportedCca, SupportedCcaThreshold, SupportedEnergyScan,
        SupportedHeaderIes, SupportedPower, SupportedScheduledReceive, SupportedTransmit,
    },
};
use oer_ieee802154_port::{
    CancelError, ClockError, EnableResult, Lent, PhaseParts, PortBackend, RadioPortClient,
    RadioPortEvents, RadioWork, Retire, Work,
};
use oer_radio_permit_pool::FifoPermitPool;
use oer_radio_port::{EventsLost, Mint, PermitPool, Poisoned, PortResult, Seat, Slot};
use oer_time::Timer;

use crate::{Ieee802154RadioEvent, Ieee802154Runtime, Ieee802154RuntimeParts};

/// The most interfaces a runtime serves: the engine's multi-PAN contexts.
pub const RUNTIME_INTERFACES: usize = 4;

/// What a composition's capabilities marker adds for the runtime: how many
/// interfaces it mints resources for, as a type.
pub trait Ieee802154RuntimeCaps: Ieee802154Caps {
    /// The interfaces' port-lifetime resources.
    type Interfaces<'id>;

    /// Mint the interfaces' resources.
    fn interfaces<'id>(mint: &mut Mint<'id>) -> Self::Interfaces<'id>;
}

/// What one interface asks of the radio's resting state.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Wish {
    #[default]
    Sleep,
    Receive(Channel),
    /// Waiting to receive on a channel another interface does not receive
    /// on; meanwhile it does not receive.
    Waiting(Channel),
    /// A scheduled receive window.
    Window(ScheduledReceiveRequest),
}

/// The kind of the operation in flight, for its terminal when a fault ends
/// it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OperationKind {
    Transmit,
    EnergyScan,
    Cca,
}

/// The runtime's held part of the install: the operation permit, the
/// interfaces' wishes and the wakers of their waiting futures.
pub struct RuntimeHeld<'id> {
    permits: OperationPermits<'id, FifoPermitPool<'id, Operation, 1, RUNTIME_INTERFACES>>,
    kind: Cell<Option<OperationKind>>,
    wishes: RefCell<[Wish; RUNTIME_INTERFACES]>,
    waiters: RefCell<[Option<Waker>; RUNTIME_INTERFACES]>,
    /// The resting state the radio was last asked for.
    applied: Cell<Option<Wish>>,
}

impl RuntimeHeld<'_> {
    fn wake_all(&self) {
        for waker in self.waiters.borrow_mut().iter_mut() {
            if let Some(waker) = waker.take() {
                waker.wake();
            }
        }
    }

    /// The channel the interfaces other than `except` receive on.
    fn receiving(&self, except: Option<usize>) -> Option<Channel> {
        self.wishes
            .borrow()
            .iter()
            .enumerate()
            .filter(|(index, _)| Some(*index) != except)
            .find_map(|(_, wish)| match *wish {
                Wish::Receive(channel) => Some(channel),
                Wish::Sleep | Wish::Waiting(_) | Wish::Window(_) => None,
            })
    }

    /// The resting state the wishes ask for: a window first, which
    /// preempts, then the receive union, else sleep.
    fn resting(&self) -> Wish {
        let window = self
            .wishes
            .borrow()
            .iter()
            .find(|wish| matches!(wish, Wish::Window(_)))
            .copied();
        if let Some(window) = window {
            return window;
        }
        match self.receiving(None) {
            Some(channel) => Wish::Receive(channel),
            None => Wish::Sleep,
        }
    }
}

/// The runtime's port-lifetime resources.
pub struct RuntimeResources<'r, 'id, I, Rt> {
    /// The interfaces' configuration and pending-table slots.
    pub interfaces: I,
    /// One operation seat per interface, by its index.
    pub operation: [Seat<'id, Operation>; RUNTIME_INTERFACES],
    /// The recent-RSSI reader.
    pub rssi: RuntimeRssi<'r, Rt>,
}

/// The recent-RSSI reader of an install: the live RSSI the hardware reports
/// (`esp_i_get_recent_rssi`), whatever the port's state; the
/// lowest RSSI while no radio is installed.
pub struct RuntimeRssi<'r, Rt> {
    runtime: &'r Rt,
}

impl<'storage, M: RawMutex, H, T, R, const EVENTS: usize>
    Ieee802154Runtime<'storage, M, H, T, R, EVENTS>
{
    /// A recent-RSSI reader that outlives the installs, for a reader built
    /// before the port runs (OpenThread's instance).
    pub const fn recent_rssi_reader(&self) -> RuntimeRssi<'_, Self> {
        RuntimeRssi { runtime: self }
    }
}

impl<M, H, T, R, const EVENTS: usize> RecentRssi
    for RuntimeRssi<'_, Ieee802154Runtime<'_, M, H, T, R, EVENTS>>
where
    M: RawMutex,
    H: Ieee802154LowLevel + oer_espressif_ieee802154_engine::ll::Ieee802154RecentRssi,
    T: Timer,
    R: Ieee802154Random,
{
    // CAPABILITY: i-phy-and-rf-rssi
    fn recent_rssi(&self) -> i8 {
        self.runtime
            .with_held(|_, hardware, _| hardware.recent_rssi())
            .unwrap_or(i8::MIN)
    }
}

/// What detaches the MAC interrupt's entry synchronously before the
/// uninstall: the composition's bound CPU route.
pub trait Ieee802154RouteDetach {
    /// Mask the route, so the interrupt entry never runs again.
    fn detach(&mut self);
}

/// No route to detach, as a backend that never ran a port has.
pub struct NoRoute;

impl Ieee802154RouteDetach for NoRoute {
    fn detach(&mut self) {}
}

/// The installed runtime as the IEEE 802.15.4 port's backend, with the
/// capabilities marker `K` its composition names and the route `D` its
/// `detach` masks before the uninstall takes the engine.
pub struct Ieee802154RuntimeBackend<
    'r,
    'storage,
    K,
    M: RawMutex,
    H,
    T,
    R,
    const EVENTS: usize,
    D = NoRoute,
> {
    runtime: &'r Ieee802154Runtime<'storage, M, H, T, R, EVENTS>,
    route: Option<D>,
    caps: PhantomData<fn() -> K>,
}

impl<'r, 'storage, K, M: RawMutex, H, T, R, const EVENTS: usize>
    Ieee802154RuntimeBackend<'r, 'storage, K, M, H, T, R, EVENTS>
{
    pub(crate) const fn new(runtime: &'r Ieee802154Runtime<'storage, M, H, T, R, EVENTS>) -> Self {
        Self {
            runtime,
            route: None,
            caps: PhantomData,
        }
    }

    /// The backend whose `detach` masks `route`.
    pub fn with_route<D: Ieee802154RouteDetach>(
        self,
        route: D,
    ) -> Ieee802154RuntimeBackend<'r, 'storage, K, M, H, T, R, EVENTS, D> {
        Ieee802154RuntimeBackend {
            runtime: self.runtime,
            route: Some(route),
            caps: PhantomData,
        }
    }
}

impl<'r, 'storage, K, M: RawMutex, H, T, R, const EVENTS: usize, D>
    Ieee802154RuntimeBackend<'r, 'storage, K, M, H, T, R, EVENTS, D>
{
    /// The runtime it serves.
    pub const fn runtime(&self) -> &'r Ieee802154Runtime<'storage, M, H, T, R, EVENTS> {
        self.runtime
    }
}

impl<'storage, K, M, H, T, R, const EVENTS: usize, D>
    Ieee802154RuntimeBackend<'_, 'storage, K, M, H, T, R, EVENTS, D>
where
    M: RawMutex,
    H: Ieee802154LowLevel,
    T: Timer,
    R: Ieee802154Random,
{
    /// Take the engine and the hardware back from a backend that never ran
    /// a port, as a bring-up failure after the install does, with its route
    /// not yet detached.
    pub fn uninstall_unused(self) -> (Ieee802154RuntimeParts<'storage, H>, Option<D>) {
        (self.runtime.uninstall(), self.route)
    }
}

/// The port's owned event: the runtime's event and, for a terminal, the
/// interface whose operation it ends.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ieee802154PortEvent {
    /// Whose operation the event ends; `None` for a received frame.
    pub interface: Option<Interface>,
    /// The event.
    pub event: Ieee802154RadioEvent,
}

/// A configured interface `I` of the runtime.
pub struct RuntimeConfigured<'id, const I: u8> {
    slot: Slot<'id, InterfaceKind<I>>,
}

impl<'id, const I: u8> ConfiguredInterface<'id, I> for RuntimeConfigured<'id, I> {}

/// The runtime's disabled-only settings.
pub struct RuntimeSetup<'a, 'id, Rt, K> {
    runtime: &'a Rt,
    brand: PhantomData<fn(&'id ()) -> &'id ()>,
    caps: PhantomData<fn() -> K>,
}

/// The runtime's phase control.
pub struct RuntimeControl<'a, Rt, K> {
    runtime: &'a Rt,
    caps: PhantomData<fn() -> K>,
}

/// The runtime's one events consumer.
pub struct RuntimeEvents<'a, 'id, Rt> {
    runtime: &'a Rt,
    held: &'a RuntimeHeld<'id>,
}

/// The runtime's clients: one per configured interface.
pub struct RuntimeClients<'a, 'id, Rt, K> {
    runtime: &'a Rt,
    held: &'a RuntimeHeld<'id>,
    caps: PhantomData<fn() -> K>,
}

/// The client of interface `I` in a running phase.
pub struct RuntimeClient<'c, 'id, Rt, K, const I: u8> {
    runtime: &'c Rt,
    held: &'c RuntimeHeld<'id>,
    caps: PhantomData<fn() -> K>,
}

/// The work token of the runtime.
#[derive(Debug)]
pub struct RuntimeToken {
    id: RequestId,
}

impl RadioWork for RuntimeToken {
    type Id = RequestId;

    fn id(&self) -> RequestId {
        self.id
    }
}

/// An installed short-address pending entry of interface `I`.
pub struct RuntimeShortEntry<'id, const I: u8> {
    slot: Slot<'id, ShortPending<I>>,
    address: [u8; 2],
}

/// An installed extended-address pending entry of interface `I`.
pub struct RuntimeExtendedEntry<'id, const I: u8> {
    slot: Slot<'id, ExtendedPending<I>>,
    address: [u8; 8],
}

impl<M, H, T, R, const EVENTS: usize> Ieee802154Runtime<'_, M, H, T, R, EVENTS>
where
    M: RawMutex,
    H: Ieee802154LowLevel,
    T: Timer,
    R: Ieee802154Random,
{
    /// A request identifier of the runtime's own.
    fn mint_id(&self) -> RequestId {
        self.next_id.lock(|next| {
            let id = next.get();
            next.set(id.wrapping_add(1).max(1));
            RequestId::new(id)
        })
    }

    /// The radio's fault, once it poisoned the port.
    fn poisoned(&self) -> Result<(), Poisoned<RadioFault>> {
        match self.fault.lock(Cell::get) {
            Some(cause) => Err(Poisoned { cause }),
            None => Ok(()),
        }
    }

    /// Poison the port with `fault`.
    fn poison(&self, fault: RadioFault) {
        self.fault.lock(|cell| {
            if cell.get().is_none() {
                cell.set(Some(fault));
            }
        });
        self.events.changed.signal(());
    }

    /// Apply one configuration through the radio role, in any state the
    /// owner shape calls it in (the disabled setup, a running phase, an
    /// operation in flight), as the vendor's setters do; the owner shape's
    /// types admit only configurations it takes, so a refusal is a fault.
    fn configure(&self, configuration: Configuration) {
        if self
            .with_installed(|radio, hardware, _| radio.configure(hardware, configuration))
            .is_err()
        {
            self.poison(RadioFault::StateLost);
        }
    }

    /// One setting of `interface`: its own on a multi-PAN radio, the
    /// radio's on a single-interface one.
    fn configure_interface(&self, interface: Interface, setting: InterfaceSetting) {
        let multipan = self.with_installed(|radio, _, _| radio.interfaces()) > 1;
        let configuration = if multipan {
            Configuration::Interface { interface, setting }
        } else {
            match setting {
                InterfaceSetting::PanId(pan) => Configuration::PanId(pan),
                InterfaceSetting::ShortAddress(short) => Configuration::ShortAddress(short),
                InterfaceSetting::ExtendedAddress(extended) => {
                    Configuration::ExtendedAddress(extended)
                }
                InterfaceSetting::PendingMode(mode) => Configuration::PendingMode(mode),
                InterfaceSetting::AddPendingAddress(address) => {
                    Configuration::AddPendingAddress(address)
                }
                InterfaceSetting::RemovePendingAddress(address) => {
                    Configuration::RemovePendingAddress(address)
                }
                InterfaceSetting::ResetPendingTable(half) => Configuration::ResetPendingTable(half),
                // A single interface is always enabled.
                InterfaceSetting::Enabled(_) => return,
            }
        };
        self.configure(configuration);
    }

    /// Apply one radio setting; the validated values admit only settings
    /// the radio takes, so a refusal is a fault.
    fn setting(&self, setting: RadioSetting<'_>) {
        if self.apply_setting(setting).is_err() {
            self.poison(RadioFault::StateLost);
        }
    }

    /// Ask the radio for the resting state the wishes of `held` ask for,
    /// unless an operation runs: the consumer applies it at the
    /// operation's terminal. A disabled radio rests as it is: the disable
    /// takes the terminals it ended, and the enable starts it asleep.
    fn rest(&self, held: &RuntimeHeld<'_>) {
        let resting = held.resting();
        if held.applied.get() == Some(resting) || held.permits.in_flight().is_some() {
            return;
        }
        if self.with_installed(|radio, _, _| radio.state()) == RadioState::Disabled {
            return;
        }
        let command = match resting {
            Wish::Receive(channel) => RadioCommand::Receive {
                id: self.mint_id(),
                channel,
            },
            Wish::Window(window) => RadioCommand::ScheduledReceive(window),
            Wish::Sleep | Wish::Waiting(_) => RadioCommand::Sleep { id: self.mint_id() },
        };
        match self.submit_locked(command) {
            Ok(_) => held.applied.set(Some(resting)),
            // An operation the radio still runs; its terminal applies it.
            Err(CommandError::Busy { .. }) => {}
            Err(_) => self.poison(RadioFault::StateLost),
        }
    }

    /// Submit the operation `command` of `interface`, whose identity is
    /// `id`, holding `permit` until its terminal.
    fn submit_operation<'id>(
        &self,
        held: &RuntimeHeld<'id>,
        interface: Interface,
        permit: OperationPermit<'_, 'id>,
        kind: OperationKind,
        id: RequestId,
        command: RadioCommand<'_>,
    ) -> Result<(), Poisoned<RadioFault>> {
        self.poisoned()?;
        if self.submit_locked(command).is_err() {
            // The permit and the validated values exclude every refusal:
            // the radio's state and the runtime's disagree.
            self.poison(RadioFault::StateLost);
            return self.poisoned();
        }
        held.kind.set(Some(kind));
        held.permits.admit(interface, id, permit);
        Ok(())
    }
}

/// The runtime type a backend of these parameters serves.
type Rt<'storage, M, H, T, R, const EVENTS: usize> =
    Ieee802154Runtime<'storage, M, H, T, R, EVENTS>;

impl<'r, 'storage, K, M, H, T, R, const EVENTS: usize, D> PortBackend
    for Ieee802154RuntimeBackend<'r, 'storage, K, M, H, T, R, EVENTS, D>
where
    D: Ieee802154RouteDetach,
    K: Ieee802154RuntimeCaps,
    M: RawMutex,
    H: Ieee802154LowLevel + oer_espressif_ieee802154_engine::ll::Ieee802154RecentRssi,
    T: Timer,
    R: Ieee802154Random,
{
    type Event = Ieee802154PortEvent;
    type Domain = oer_ieee802154::Ieee802154Radio;
    type Fault = RadioFault;
    /// The radio was not in a state to enable.
    type Refusal = ();
    type Setup<'a, 'id>
        = RuntimeSetup<'a, 'id, Rt<'storage, M, H, T, R, EVENTS>, K>
    where
        Self: 'a;
    type Events<'a, 'id>
        = RuntimeEvents<'a, 'id, Rt<'storage, M, H, T, R, EVENTS>>
    where
        Self: 'a,
        'id: 'a;
    type Control<'a, 'id>
        = RuntimeControl<'a, Rt<'storage, M, H, T, R, EVENTS>, K>
    where
        Self: 'a,
        'id: 'a;
    type Clients<'a, 'id>
        = RuntimeClients<'a, 'id, Rt<'storage, M, H, T, R, EVENTS>, K>
    where
        Self: 'a,
        'id: 'a;
    type Resources<'id> =
        RuntimeResources<'r, 'id, K::Interfaces<'id>, Rt<'storage, M, H, T, R, EVENTS>>;
    type Held<'id> = RuntimeHeld<'id>;
    type State = ();
    type Uninstall = ();
    type Uninstalled = Ieee802154RuntimeParts<'storage, H>;

    fn enable(&mut self) -> impl Future<Output = EnableResult<(), RadioFault>> + '_ {
        ready(
            self.runtime
                .poisoned()
                .map(|()| self.runtime.enable_locked().map_err(|_| ())),
        )
    }

    fn disable<'a, 'id>(
        &'a mut self,
        held: &'a RuntimeHeld<'id>,
        sink: &'a mut dyn FnMut(Ieee802154PortEvent),
    ) -> impl Future<Output = Result<(), Poisoned<RadioFault>>> + 'a
    where
        'id: 'a,
    {
        let runtime = self.runtime;
        // Disabling ends the operation in flight with its terminal; every
        // queued terminal goes to the sink, its permit back first.
        runtime.disable_locked();
        while let Some(taken) = runtime.events.take() {
            if let Ok(event) = taken
                && let Some(port) = take_terminal(runtime, held, event)
            {
                sink(port);
            }
        }
        *held.wishes.borrow_mut() = [Wish::Sleep; RUNTIME_INTERFACES];
        held.applied.set(None);
        held.wake_all();
        ready(runtime.poisoned())
    }

    fn now(
        &self,
    ) -> impl Future<Output = PortResult<Ieee802154Instant, ClockError, RadioFault>> + '_ {
        ready(self.runtime.poisoned().map(|()| {
            Ok(Ieee802154Instant::from_micros(
                self.runtime.clock().now().as_micros(),
            ))
        }))
    }

    fn resources<'id>(&mut self, mut mint: Mint<'id>) -> (RuntimeHeld<'id>, Self::Resources<'id>) {
        let (permits, operation) = OperationPermits::new(&mut mint, |permit, line| {
            FifoPermitPool::new([permit], line)
        });
        let held = RuntimeHeld {
            permits,
            kind: Cell::new(None),
            wishes: RefCell::new([Wish::Sleep; RUNTIME_INTERFACES]),
            waiters: RefCell::new([const { None }; RUNTIME_INTERFACES]),
            applied: Cell::new(None),
        };
        let resources = RuntimeResources {
            interfaces: K::interfaces(&mut mint),
            operation,
            rssi: RuntimeRssi {
                runtime: self.runtime,
            },
        };
        (held, resources)
    }

    fn setup<'a, 'id>(&'a mut self) -> Lent<'a, Self::Setup<'a, 'id>> {
        Lent::new(RuntimeSetup {
            runtime: self.runtime,
            brand: PhantomData,
            caps: PhantomData,
        })
    }

    fn split<'a, 'id>(
        &'a mut self,
        held: &'a RuntimeHeld<'id>,
    ) -> PhaseParts<'a, Self::Events<'a, 'id>, Self::Control<'a, 'id>, Self::Clients<'a, 'id>>
    where
        'id: 'a,
    {
        PhaseParts {
            events: Lent::new(RuntimeEvents {
                runtime: self.runtime,
                held,
            }),
            control: Lent::new(RuntimeControl {
                runtime: self.runtime,
                caps: PhantomData,
            }),
            clients: Lent::new(RuntimeClients {
                runtime: self.runtime,
                held,
                caps: PhantomData,
            }),
        }
    }

    /// The CSMA-CA backoff and the retry delays, until the port retires.
    fn drive<'p, 'id>(
        &mut self,
        _: &'p Self::Held<'id>,
        retire: Retire<'p>,
    ) -> impl Future<Output = ()> + use<'p, 'id, 'r, 'storage, K, M, H, T, R, EVENTS, D> {
        let runtime = self.runtime;
        async move {
            select(runtime.run(), retire.requested()).await;
        }
    }

    /// The driver retires at its next poll.
    fn retire_bound(&self) -> impl Future<Output = ()> + '_ {
        core::future::pending()
    }

    /// Mask the MAC interrupt's route, so the interrupt entry never runs
    /// again before the uninstall takes the engine.
    fn detach<'id>(&mut self, _: &RuntimeHeld<'id>) {
        if let Some(route) = self.route.as_mut() {
            route.detach();
        }
    }

    fn uninstall<'id>(
        self,
        _: RuntimeHeld<'id>,
        (): (),
        (): (),
    ) -> Ieee802154RuntimeParts<'storage, H> {
        self.runtime.uninstall()
    }

    fn abandon(&mut self) {
        self.runtime.poison(RadioFault::StateLost);
        self.runtime.disable_locked();
        if let Some(route) = self.route.as_mut() {
            route.detach();
        }
    }
}

impl<'r, 'storage, K, M, H, T, R, const EVENTS: usize, D> Ieee802154Backend
    for Ieee802154RuntimeBackend<'r, 'storage, K, M, H, T, R, EVENTS, D>
where
    D: Ieee802154RouteDetach,
    K: Ieee802154RuntimeCaps,
    M: RawMutex,
    H: Ieee802154LowLevel + oer_espressif_ieee802154_engine::ll::Ieee802154RecentRssi,
    T: Timer,
    R: Ieee802154Random,
{
    type Caps = K;
}

/// Take one queued runtime event for the port: give a terminal's permit
/// back and name its interface; mark an ended window; poison on a fault,
/// handing out the failed terminal of the operation it ended. `None` for
/// what the port does not hand out.
fn take_terminal<M, H, T, R, const EVENTS: usize>(
    runtime: &Rt<'_, M, H, T, R, EVENTS>,
    held: &RuntimeHeld<'_>,
    event: Ieee802154RadioEvent,
) -> Option<Ieee802154PortEvent>
where
    M: RawMutex,
    H: Ieee802154LowLevel,
    T: Timer,
    R: Ieee802154Random,
{
    let ended = match &event {
        Ieee802154RadioEvent::Received(_) => {
            return Some(Ieee802154PortEvent {
                interface: None,
                event,
            });
        }
        Ieee802154RadioEvent::TransmitDone { id, .. }
        | Ieee802154RadioEvent::EnergyScanDone { id, .. }
        | Ieee802154RadioEvent::EnergyScanFailed { id }
        | Ieee802154RadioEvent::ClearChannelAssessmentDone { id, .. }
        | Ieee802154RadioEvent::ClearChannelAssessmentFailed { id } => *id,
        Ieee802154RadioEvent::ScheduledReceiveDone { id } => {
            let mut ended = false;
            for wish in held.wishes.borrow_mut().iter_mut() {
                if matches!(*wish, Wish::Window(window) if window.id == *id) {
                    *wish = Wish::Sleep;
                    ended = true;
                }
            }
            if ended {
                held.applied.set(None);
                held.wake_all();
                runtime.rest(held);
            }
            return None;
        }
        Ieee802154RadioEvent::Fault { id, fault } => {
            runtime.poison(*fault);
            let id = (*id)?;
            let kind = held.kind.get()?;
            let interface = held.permits.in_flight().map(|(interface, _)| interface);
            if let Some(interface) = interface {
                held.permits.complete(interface, id);
            }
            held.wake_all();
            let event = match kind {
                OperationKind::Transmit => Ieee802154RadioEvent::TransmitDone {
                    id,
                    status: oer_ieee802154::TxStatus::HardwareFailure,
                    acknowledgement: None,
                    security: None,
                },
                OperationKind::EnergyScan => Ieee802154RadioEvent::EnergyScanFailed { id },
                OperationKind::Cca => Ieee802154RadioEvent::ClearChannelAssessmentFailed { id },
            };
            return Some(Ieee802154PortEvent { interface, event });
        }
        Ieee802154RadioEvent::Lifecycle(_) => return None,
    };
    let interface = held.permits.in_flight().map(|(interface, _)| interface);
    if let Some(interface) = interface {
        held.permits.complete(interface, ended);
    }
    held.kind.set(None);
    // A wish made while the operation ran takes effect now.
    held.applied.set(None);
    runtime.rest(held);
    Some(Ieee802154PortEvent { interface, event })
}

impl<M, H, T, R, const EVENTS: usize> RadioPortEvents
    for RuntimeEvents<'_, '_, Rt<'_, M, H, T, R, EVENTS>>
where
    M: RawMutex,
    H: Ieee802154LowLevel,
    T: Timer,
    R: Ieee802154Random,
{
    type Event = Ieee802154PortEvent;
    type Fault = RadioFault;

    async fn next_event(&mut self) -> PortResult<Ieee802154PortEvent, EventsLost, RadioFault> {
        loop {
            match self.runtime.events.take() {
                Some(Err(EventsLost)) => return Ok(Err(EventsLost)),
                Some(Ok(event)) => {
                    if let Some(port) = take_terminal(self.runtime, self.held, event) {
                        return Ok(Ok(port));
                    }
                    continue;
                }
                None => {}
            }
            self.runtime.poisoned()?;
            select(
                self.runtime.events.entries.ready_to_receive(),
                self.runtime.events.changed.wait(),
            )
            .await;
        }
    }
}

impl<M, H, T, R, const EVENTS: usize> Ieee802154Events
    for RuntimeEvents<'_, '_, Rt<'_, M, H, T, R, EVENTS>>
where
    M: RawMutex,
    H: Ieee802154LowLevel,
    T: Timer,
    R: Ieee802154Random,
{
    fn view(event: &Ieee802154PortEvent) -> PhaseEvent<'_> {
        let interface = event.interface.unwrap_or(Interface::PRIMARY);
        match &event.event {
            Ieee802154RadioEvent::Received(frame) => PhaseEvent::Received(frame.received()),
            Ieee802154RadioEvent::TransmitDone {
                id,
                status,
                acknowledgement,
                security,
            } => PhaseEvent::TransmitDone {
                interface,
                id: *id,
                status: *status,
                acknowledgement: acknowledgement
                    .as_ref()
                    .map(crate::Ieee802154OwnedFrame::received),
                security: *security,
            },
            Ieee802154RadioEvent::EnergyScanDone { id, energy_dbm } => PhaseEvent::EnergyScanDone {
                interface,
                id: *id,
                energy_dbm: *energy_dbm,
            },
            Ieee802154RadioEvent::EnergyScanFailed { id } => {
                PhaseEvent::EnergyScanFailed { interface, id: *id }
            }
            Ieee802154RadioEvent::ClearChannelAssessmentDone { id, idle } => {
                PhaseEvent::ClearChannelAssessmentDone {
                    interface,
                    id: *id,
                    idle: *idle,
                }
            }
            Ieee802154RadioEvent::ClearChannelAssessmentFailed { id } => {
                PhaseEvent::ClearChannelAssessmentFailed { interface, id: *id }
            }
            // The port hands out neither a window's end, a fault nor a
            // lifecycle terminal ([`take_terminal`]).
            Ieee802154RadioEvent::ScheduledReceiveDone { .. }
            | Ieee802154RadioEvent::Fault { .. }
            | Ieee802154RadioEvent::Lifecycle(_) => {
                unreachable!("the port hands out frames and operation terminals only")
            }
        }
    }
}

/// The radio-wide settings of the setup and the control.
macro_rules! radio_settings {
    ($part:ident < $($lt:lifetime),* >) => {
        impl<$($lt,)* 'storage, K, M, H, T, R, const EVENTS: usize> RadioSettings
            for $part<$($lt,)* Rt<'storage, M, H, T, R, EVENTS>, K>
        where
            K: Ieee802154Caps,
            M: RawMutex,
            H: Ieee802154LowLevel,
            T: Timer,
            R: Ieee802154Random,
        {
            type Caps = K;

            fn set_transmit_power(&mut self, power: SupportedPower<K>) {
                self.runtime.setting(RadioSetting::TransmitPowerDbm(power.dbm()));
            }

            fn set_channel_power(&mut self, channel: Channel, power: SupportedPower<K>) {
                self.runtime.setting(RadioSetting::ChannelTransmitPowerDbm {
                    channel,
                    power_dbm: power.dbm(),
                });
            }

            fn set_cca_threshold(&mut self, threshold: SupportedCcaThreshold<K>) {
                self.runtime.setting(RadioSetting::CcaThresholdDbm(threshold.dbm()));
            }

            fn set_cca_mode(&mut self, _: Has<K, ClearChannelAssessment>, mode: CcaMode) {
                self.runtime.setting(RadioSetting::CcaMode(mode));
            }

            fn set_promiscuous(&mut self, _: Has<K, Promiscuous>, promiscuous: bool) {
                self.runtime.configure(Configuration::Promiscuous(promiscuous));
            }

            fn set_automatic_acknowledgement(
                &mut self,
                _: Has<K, AutomaticAcknowledgement>,
                acknowledge: bool,
            ) {
                self.runtime
                    .configure(Configuration::AutomaticAcknowledgement(acknowledge));
            }

            fn set_pan_coordinator(&mut self, coordinator: bool) {
                self.runtime.configure(Configuration::PanCoordinator(coordinator));
            }

            fn set_csl(&mut self, csl: CslReceiver) {
                self.runtime.setting(RadioSetting::Csl(csl));
            }

            fn set_enhanced_ack(&mut self, generation: Option<EnhancedAckGeneration>) {
                self.runtime.setting(RadioSetting::EnhancedAck(generation));
            }

            /// Kept while generation is off, and sent once it is on.
            fn set_enhanced_ack_header_ies(&mut self, ies: SupportedHeaderIes<'_, K>) {
                self.runtime.setting(RadioSetting::EnhancedAckHeaderIes(ies.get()));
            }

            /// Kept while generation is off, and served once it is on.
            fn set_enhanced_ack_probing(&mut self, initiators: &[ProbingInitiator]) {
                self.runtime.setting(RadioSetting::EnhancedAckProbing(initiators));
            }
        }
    };
}

radio_settings!(RuntimeSetup<'a, 'id>);
radio_settings!(RuntimeControl<'a>);

impl<M, H, T, R, const EVENTS: usize, K: Ieee802154Caps> Ieee802154Control
    for RuntimeControl<'_, Rt<'_, M, H, T, R, EVENTS>, K>
where
    M: RawMutex,
    H: Ieee802154LowLevel,
    T: Timer,
    R: Ieee802154Random,
{
}

impl<'id, M, H, T, R, const EVENTS: usize, K: Ieee802154Caps> Ieee802154Setup<'id>
    for RuntimeSetup<'_, 'id, Rt<'_, M, H, T, R, EVENTS>, K>
where
    M: RawMutex,
    H: Ieee802154LowLevel,
    T: Timer,
    R: Ieee802154Random,
{
    type Configured<const I: u8> = RuntimeConfigured<'id, I>;

    fn configure<const I: u8>(
        &mut self,
        slot: Slot<'id, InterfaceKind<I>>,
    ) -> RuntimeConfigured<'id, I> {
        self.runtime
            .configure_interface(Interface::new(I), InterfaceSetting::Enabled(true));
        RuntimeConfigured { slot }
    }

    fn remove<const I: u8>(
        &mut self,
        interface: RuntimeConfigured<'id, I>,
    ) -> Slot<'id, InterfaceKind<I>> {
        self.runtime
            .configure_interface(Interface::new(I), InterfaceSetting::Enabled(false));
        interface.slot
    }
}

impl<'a, 'id, 'storage, M, H, T, R, const EVENTS: usize, K> Ieee802154Clients<'id>
    for RuntimeClients<'a, 'id, Rt<'storage, M, H, T, R, EVENTS>, K>
where
    K: Ieee802154Caps,
    M: RawMutex,
    H: Ieee802154LowLevel,
    T: Timer,
    R: Ieee802154Random,
{
    type Configured<const I: u8> = RuntimeConfigured<'id, I>;
    type Client<'c, const I: u8>
        = RuntimeClient<'c, 'id, Rt<'storage, M, H, T, R, EVENTS>, K, I>
    where
        Self: 'c;

    fn client<'c, const I: u8>(&'c self, _: &RuntimeConfigured<'id, I>) -> Self::Client<'c, I> {
        RuntimeClient {
            runtime: self.runtime,
            held: self.held,
            caps: PhantomData,
        }
    }
}

impl<M, H, T, R, const EVENTS: usize, K, const I: u8> RadioPortClient
    for RuntimeClient<'_, '_, Rt<'_, M, H, T, R, EVENTS>, K, I>
where
    M: RawMutex,
    H: Ieee802154LowLevel,
    T: Timer,
    R: Ieee802154Random,
{
    type Token = RuntimeToken;
    type Domain = oer_ieee802154::Ieee802154Radio;
    type Fault = RadioFault;

    fn now(
        &self,
    ) -> impl Future<Output = PortResult<Ieee802154Instant, ClockError, RadioFault>> + '_ {
        ready(self.runtime.poisoned().map(|()| {
            Ok(Ieee802154Instant::from_micros(
                self.runtime.clock().now().as_micros(),
            ))
        }))
    }

    fn end(
        &self,
        token: RuntimeToken,
    ) -> impl Future<Output = PortResult<(), CancelError, RadioFault>> + '_ {
        ready(
            self.runtime
                .poisoned()
                .map(|()| self.runtime.cancel_locked(token.id)),
        )
    }
}

impl<'c, 'id, M, H, T, R, const EVENTS: usize, K, const I: u8> Ieee802154Client<'id, I>
    for RuntimeClient<'c, 'id, Rt<'_, M, H, T, R, EVENTS>, K, I>
where
    K: Ieee802154Caps,
    M: RawMutex,
    H: Ieee802154LowLevel,
    T: Timer,
    R: Ieee802154Random,
{
    type Caps = K;
    type ShortEntry = RuntimeShortEntry<'id, I>;
    type ExtendedEntry = RuntimeExtendedEntry<'id, I>;

    fn set_pan_id(&self, pan_id: u16) {
        self.runtime
            .configure_interface(Interface::new(I), InterfaceSetting::PanId(pan_id));
    }

    fn set_short_address(&self, address: u16) {
        self.runtime
            .configure_interface(Interface::new(I), InterfaceSetting::ShortAddress(address));
    }

    fn set_extended_address(&self, address: [u8; 8]) {
        self.runtime.configure_interface(
            Interface::new(I),
            InterfaceSetting::ExtendedAddress(address),
        );
    }

    fn set_mac_keys(&self, key_id: u8, previous: [u8; 16], current: [u8; 16], next: [u8; 16]) {
        self.runtime.setting(RadioSetting::MacKeys {
            interface: Interface::new(I),
            key_id,
            previous,
            current,
            next,
        });
    }

    fn remove_mac_keys(&self) {
        self.runtime.setting(RadioSetting::RemoveMacKeys {
            interface: Interface::new(I),
        });
    }

    fn update_frame_counter(&self, update: FrameCounterUpdate) {
        self.runtime.setting(RadioSetting::FrameCounter {
            interface: Interface::new(I),
            update,
        });
    }

    fn frame_counter(&self) -> Option<u32> {
        self.runtime.with_installed(|radio, _, _| {
            radio
                .interface_mac_keys(Interface::new(I))
                .and_then(|keys| keys.as_ref().map(MacKeys::frame_counter))
        })
    }

    fn set_pending_mode(&self, _: Has<K, SourceMatch>, mode: AutoPendingMode) {
        self.runtime
            .configure_interface(Interface::new(I), InterfaceSetting::PendingMode(mode));
    }

    fn add_pending_short(
        &self,
        slot: Slot<'id, ShortPending<I>>,
        address: [u8; 2],
    ) -> RuntimeShortEntry<'id, I> {
        self.runtime.configure_interface(
            Interface::new(I),
            InterfaceSetting::AddPendingAddress(FrameAddress::Short(address)),
        );
        RuntimeShortEntry { slot, address }
    }

    fn add_pending_extended(
        &self,
        slot: Slot<'id, ExtendedPending<I>>,
        address: [u8; 8],
    ) -> RuntimeExtendedEntry<'id, I> {
        self.runtime.configure_interface(
            Interface::new(I),
            InterfaceSetting::AddPendingAddress(FrameAddress::Extended(address)),
        );
        RuntimeExtendedEntry { slot, address }
    }

    fn remove_pending_short(&self, entry: RuntimeShortEntry<'id, I>) -> Slot<'id, ShortPending<I>> {
        self.runtime.configure_interface(
            Interface::new(I),
            InterfaceSetting::RemovePendingAddress(FrameAddress::Short(entry.address)),
        );
        entry.slot
    }

    fn remove_pending_extended(
        &self,
        entry: RuntimeExtendedEntry<'id, I>,
    ) -> Slot<'id, ExtendedPending<I>> {
        self.runtime.configure_interface(
            Interface::new(I),
            InterfaceSetting::RemovePendingAddress(FrameAddress::Extended(entry.address)),
        );
        entry.slot
    }

    fn receive(&self, channel: Channel) -> impl Future<Output = Result<(), Poisoned<RadioFault>>> {
        let index = usize::from(I);
        poll_fn(move |context| {
            self.runtime.poisoned()?;
            // A receive on another channel waits; a window and a waiting
            // interface do not count. The request replaces the previous
            // wish at once.
            if self
                .held
                .receiving(Some(index))
                .is_some_and(|on| on != channel)
            {
                let waiting = Wish::Waiting(channel);
                if self.held.wishes.borrow()[index] != waiting {
                    self.held.wishes.borrow_mut()[index] = waiting;
                    self.runtime.rest(self.held);
                    self.held.wake_all();
                }
                self.held.waiters.borrow_mut()[index] = Some(context.waker().clone());
                return Poll::Pending;
            }
            self.held.wishes.borrow_mut()[index] = Wish::Receive(channel);
            self.runtime.rest(self.held);
            self.held.wake_all();
            Poll::Ready(Ok(()))
        })
    }

    fn sleep(&self) {
        self.held.wishes.borrow_mut()[usize::from(I)] = Wish::Sleep;
        self.runtime.rest(self.held);
        self.held.wake_all();
    }

    fn scheduled_receive(&self, window: SupportedScheduledReceive<K>) {
        let (channel, start, duration_us) = window.get();
        self.held.wishes.borrow_mut()[usize::from(I)] = Wish::Window(ScheduledReceiveRequest {
            id: self.runtime.mint_id(),
            channel,
            start,
            duration_us,
        });
        self.runtime.rest(self.held);
        self.held.wake_all();
    }

    fn window_ended(&self) -> impl Future<Output = Result<(), Poisoned<RadioFault>>> {
        let index = usize::from(I);
        poll_fn(move |context| {
            self.runtime.poisoned()?;
            if matches!(self.held.wishes.borrow()[index], Wish::Window(_)) {
                self.held.waiters.borrow_mut()[index] = Some(context.waker().clone());
                return Poll::Pending;
            }
            Poll::Ready(Ok(()))
        })
    }

    fn operation<'o, 's>(
        &'o self,
        seat: &'s mut Seat<'id, Operation>,
    ) -> impl Future<Output = OperationPermit<'o, 'id>> + 's
    where
        'id: 'o,
        'o: 's,
    {
        let permits = &self.held.permits;
        async move {
            let permit = permits.pool().take(seat).await;
            permits.guard(permit)
        }
    }

    fn transmit<'o>(
        &'o self,
        permit: OperationPermit<'o, 'id>,
        transmission: SupportedTransmit<'_, K>,
    ) -> Result<Work<'o, Self>, Poisoned<RadioFault>>
    where
        'id: 'o,
    {
        let transmission = transmission.get();
        let id = self.runtime.mint_id();
        self.runtime.submit_operation(
            self.held,
            Interface::new(I),
            permit,
            OperationKind::Transmit,
            id,
            RadioCommand::Transmit(TxRequest {
                id,
                frame: transmission.frame,
                channel: transmission.channel,
                mode: transmission.mode,
                transmit_power_dbm: transmission.power.as_ref().map(SupportedPower::dbm),
                max_frame_retries: transmission.max_frame_retries,
                security: transmission.security,
                interface: Interface::new(I),
                time_sync: transmission.time_sync,
            }),
        )?;
        Ok(Work::new(self, RuntimeToken { id }))
    }

    fn energy_scan<'o>(
        &'o self,
        permit: OperationPermit<'o, 'id>,
        scan: SupportedEnergyScan<K>,
    ) -> Result<Work<'o, Self>, Poisoned<RadioFault>>
    where
        'id: 'o,
    {
        let (channel, duration_us) = scan.get();
        let id = self.runtime.mint_id();
        self.runtime.submit_operation(
            self.held,
            Interface::new(I),
            permit,
            OperationKind::EnergyScan,
            id,
            RadioCommand::EnergyScan(oer_ieee802154::EnergyScanRequest {
                id,
                channel,
                duration_us,
            }),
        )?;
        Ok(Work::new(self, RuntimeToken { id }))
    }

    fn clear_channel_assessment<'o>(
        &'o self,
        permit: OperationPermit<'o, 'id>,
        cca: SupportedCca<K>,
    ) -> Result<Work<'o, Self>, Poisoned<RadioFault>>
    where
        'id: 'o,
    {
        let id = self.runtime.mint_id();
        self.runtime.submit_operation(
            self.held,
            Interface::new(I),
            permit,
            OperationKind::Cca,
            id,
            RadioCommand::ClearChannelAssessment {
                id,
                channel: cca.get(),
            },
        )?;
        Ok(Work::new(self, RuntimeToken { id }))
    }
}
