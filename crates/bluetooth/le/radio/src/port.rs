//! The radio backend a Controller service body drives.

use core::{
    convert::Infallible,
    future::{Future, pending, ready},
};

use oer_bluetooth_port::{
    ClockError, EnableResult, Lent, PhaseParts, PortBackend, RadioPortEvents, Retire,
};
use oer_radio_port::{ClockInfo, EventsLost, Mint, Poisoned, PortResult};

use crate::{
    EventId, LeCapabilities, LeInstant, LeRadio, NoCapabilities, RadioActivity, RadioOutcome,
    RadioRequest, RequestError,
};
use oer_bluetooth_port::CancelError;

/// A Bluetooth LE radio backend, as a Controller service body drives it
/// through its port owner ([`oer_bluetooth_port::Owner`]).
///
/// The lifecycle is the owner's: its disabled owner enables the port, an
/// enabled one lends one running phase of three parts and disables the port
/// within the backend's bound, handing every admitted event's end to the
/// body. An LE radio has no channel to retune, so nothing refuses `Enable`
/// ([`PortBackend::Refusal`] is [`Infallible`]). A phase's parts are the one event consumer
/// ([`PortBackend::Events`]), the control that publishes the active roles
/// to a coexistence arbiter ([`Self::activity`]) and the client of the
/// Controller core ([`Self::submit`], [`Self::cancel`], [`Self::client_now`]),
/// which exists only while the port is enabled: no request is refused as
/// disabled.
///
/// Its capabilities are a type ([`Self::Caps`]), known when the image is
/// built, with the [`RadioTiming`](crate::RadioTiming) a planner keeps
/// reservations apart by. A request carries values checked against them
/// ([`crate::Supported`]), so the backend refuses nothing they exclude; it
/// refuses what the world decides ([`RequestError::Late`] when time passed
/// between placing a request and its admission, the clock, the epoch) and
/// what its role state holds now. The outer
/// [`Poisoned`](oer_radio_port::Poisoned) means the backend's state is
/// unknown, with its cause: every later call returns it and only a reset
/// restores the port.
///
/// # Events
///
/// The phase's one consumer takes the events; the backend's scheduler work
/// runs in its driver ([`PortBackend::drive`]), which
/// [`run_port`](oer_bluetooth_port::run_port) joins with the body. The backend reserves the slots of an
/// admitted event's end ([`RadioOutcome::EventEnded`], with the
/// [`RadioOutcome::TransmitAcknowledged`] or [`RadioOutcome::TestReport`]
/// before it) and of the data PDUs a connection event receives (the Link
/// Layer promises the peer reliable delivery of what the hardware
/// acknowledged) when it admits them, so they are never lost;
/// [`EventsLost`] stands for advertising and scan reports only.
///
/// Submission and the clock are asynchronous because a backend may have to
/// wait for hardware to admit a request: the ESP32-S31 takes a fresh
/// controller-time latch for every request and every clock reading, which
/// completes only after the hardware latched the time, and its driver
/// serves a request between its scheduler passes. Neither wait depends on
/// the caller; both end within the backend's own bounds.
///
/// Who acknowledges data PDUs is part of the capabilities: either way the
/// port carries only LLID and payload, reports
/// [`RadioOutcome::TransmitAcknowledged`] once the peer acknowledged the
/// connection's queued PDU and reports only new, non-empty receptions.
///
/// # Dropping a call
///
/// A call's future may be dropped. A backend that admits in the caller's
/// future admits nothing then. A backend whose driver admits (the
/// ESP32-S31's) withdraws a request its driver has not taken, so nothing
/// happens; a request the driver took it serves to the end: its effects
/// stand (an admitted event's end reaches the event consumer as any other
/// event's) and its reply is discarded, never given to the next call. The
/// Controller service body drops no call.
///
/// # One client call at a time
///
/// The client's calls take it by `&mut`, so a backend serves one of them
/// at a time and needs no queue of callers; two at once do not compile:
///
/// ```compile_fail,E0499
/// use oer_bluetooth_radio::{EventId, LeRadioBackend, NoRadio, NoRadioPart};
/// fn twice(client: &mut NoRadioPart) {
///     let first = NoRadio::cancel(client, EventId::new(1));
///     let second = NoRadio::cancel(client, EventId::new(2));
///     drop((first, second));
/// }
/// ```
///
/// Its compiling twin finishes the first call before the second:
///
/// ```
/// use oer_bluetooth_radio::{EventId, LeRadioBackend, NoRadio, NoRadioPart};
/// async fn in_turn(client: &mut NoRadioPart) {
///     let _ = NoRadio::cancel(client, EventId::new(1)).await;
///     let _ = NoRadio::cancel(client, EventId::new(2)).await;
/// }
/// ```
pub trait LeRadioBackend: PortBackend<Domain = LeRadio, Refusal = Infallible> {
    /// What the backend serves, known when the image is built.
    type Caps: LeCapabilities;

    /// The resolution of the radio time and how its epoch relates to the
    /// image's monotonic time.
    const CLOCK: ClockInfo<LeRadio>;

    /// The portable view of an owned event.
    fn view(event: &Self::Event) -> RadioOutcome<'_>;

    /// The radio clock, read through the phase's client.
    fn client_now<'c, 'a: 'c, 'id>(
        client: &'c mut Self::Clients<'a, 'id>,
    ) -> impl Future<Output = PortResult<LeInstant, ClockError, Self::Fault>> + 'c
    where
        Self: 'a,
        'id: 'a;

    /// Submit one request: `Ok(Err(_))` when the radio refused it.
    fn submit<'c, 'a: 'c, 'id>(
        client: &'c mut Self::Clients<'a, 'id>,
        request: RadioRequest<'c, Self::Caps>,
    ) -> impl Future<Output = PortResult<(), RequestError, Self::Fault>> + 'c
    where
        Self: 'a,
        'id: 'a;

    /// Withdraw one admitted event, whose end still follows; a refusal as
    /// [`CancelError::NotRunning`] says the air ended it first.
    fn cancel<'c, 'a: 'c, 'id>(
        client: &'c mut Self::Clients<'a, 'id>,
        id: EventId,
    ) -> impl Future<Output = PortResult<(), CancelError, Self::Fault>> + 'c
    where
        Self: 'a,
        'id: 'a;

    /// The roles active now, through the phase's control. The body reports
    /// every change, starting from [`RadioActivity::IDLE`], so a radio that
    /// shares the antenna can publish them to its coexistence arbiter.
    fn activity<'a, 'id>(
        control: &Self::Control<'a, 'id>,
        activity: RadioActivity,
    ) -> Result<(), Poisoned<Self::Fault>>
    where
        Self: 'a,
        'id: 'a;
}

/// A backend without a radio: time stands still and its capabilities serve
/// nothing ([`NoCapabilities`]), so no request but a release can be built,
/// and a release names nothing configured ([`RequestError::Unknown`]).
/// Radio commands then complete with a failure status. Enabling and
/// disabling it change nothing, and it produces no event.
#[derive(Debug, Default)]
pub struct NoRadio;

/// One part of a phase of [`NoRadio`].
#[derive(Debug)]
pub struct NoRadioPart(());

impl RadioPortEvents for NoRadioPart {
    type Event = Infallible;
    type Fault = Infallible;

    fn next_event(
        &mut self,
    ) -> impl Future<Output = PortResult<Infallible, EventsLost, Infallible>> + '_ {
        pending()
    }
}

impl PortBackend for NoRadio {
    type Event = Infallible;
    type Domain = LeRadio;
    type Fault = Infallible;
    type Refusal = Infallible;
    type Setup<'a, 'id>
        = ()
    where
        Self: 'a;
    type Events<'a, 'id>
        = NoRadioPart
    where
        Self: 'a,
        'id: 'a;
    type Control<'a, 'id>
        = NoRadioPart
    where
        Self: 'a,
        'id: 'a;
    type Clients<'a, 'id>
        = NoRadioPart
    where
        Self: 'a,
        'id: 'a;
    type Resources<'id> = ();
    type Held<'id> = ();
    type State = ();
    type Uninstall = ();
    type Uninstalled = ();

    fn enable(&mut self) -> impl Future<Output = EnableResult<Infallible, Infallible>> + '_ {
        ready(Ok(Ok(())))
    }

    fn disable<'a, 'id>(
        &'a mut self,
        _: &'a Self::Held<'id>,
        _: &'a mut dyn FnMut(Infallible),
    ) -> impl Future<Output = Result<(), Poisoned<Infallible>>> + 'a
    where
        'id: 'a,
    {
        ready(Ok(()))
    }

    fn now(&self) -> impl Future<Output = PortResult<LeInstant, ClockError, Infallible>> + '_ {
        ready(Ok(Ok(LeInstant::from_micros(0))))
    }

    fn resources<'id>(&mut self, _: Mint<'id>) -> ((), ()) {
        ((), ())
    }

    fn setup<'a, 'id>(&'a mut self) -> Lent<'a, Self::Setup<'a, 'id>> {
        Lent::new(())
    }

    #[allow(clippy::type_complexity, reason = "the phase's three parts")]
    fn split<'a, 'id>(
        &'a mut self,
        _: &'a Self::Held<'id>,
    ) -> PhaseParts<'a, Self::Events<'a, 'id>, Self::Control<'a, 'id>, Self::Clients<'a, 'id>>
    where
        'id: 'a,
    {
        PhaseParts {
            events: Lent::new(NoRadioPart(())),
            control: Lent::new(NoRadioPart(())),
            clients: Lent::new(NoRadioPart(())),
        }
    }

    /// Nothing to drive: it returns at once.
    fn drive<'p, 'id>(
        &mut self,
        _: &'p Self::Held<'id>,
        _: Retire<'p>,
    ) -> impl Future<Output = ()> + use<'p, 'id> {
        ready(())
    }

    fn retire_bound(&self) -> impl Future<Output = ()> + '_ {
        pending()
    }

    fn detach<'id>(&mut self, (): &()) {}

    fn uninstall<'id>(self, (): (), (): (), (): ()) {}

    fn abandon(&mut self) {}
}

impl LeRadioBackend for NoRadio {
    type Caps = NoCapabilities;

    const CLOCK: ClockInfo<LeRadio> = ClockInfo {
        resolution: oer_time::Duration::from_micros(1),
        monotonic: None,
    };

    fn view(event: &Infallible) -> RadioOutcome<'_> {
        match *event {}
    }

    fn client_now<'c, 'a: 'c, 'id>(
        _: &'c mut Self::Clients<'a, 'id>,
    ) -> impl Future<Output = PortResult<LeInstant, ClockError, Infallible>> + 'c
    where
        Self: 'a,
        'id: 'a,
    {
        ready(Ok(Ok(LeInstant::from_micros(0))))
    }

    fn submit<'c, 'a: 'c, 'id>(
        _: &'c mut Self::Clients<'a, 'id>,
        _: RadioRequest<'c, NoCapabilities>,
    ) -> impl Future<Output = PortResult<(), RequestError, Infallible>> + 'c
    where
        Self: 'a,
        'id: 'a,
    {
        ready(Ok(Err(RequestError::Unknown)))
    }

    fn cancel<'c, 'a: 'c, 'id>(
        _: &'c mut Self::Clients<'a, 'id>,
        _: EventId,
    ) -> impl Future<Output = PortResult<(), CancelError, Infallible>> + 'c
    where
        Self: 'a,
        'id: 'a,
    {
        ready(Ok(Err(CancelError::NotRunning)))
    }

    fn activity<'a, 'id>(
        _: &Self::Control<'a, 'id>,
        _: RadioActivity,
    ) -> Result<(), Poisoned<Infallible>>
    where
        Self: 'a,
        'id: 'a,
    {
        Ok(())
    }
}
