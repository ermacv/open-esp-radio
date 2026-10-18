use core::cell::{Cell, RefCell};
use std::vec::Vec;

use core::future::{Future, pending};
use embassy_futures::select::{Either, select};
use embassy_futures::{block_on, join::join};
use embassy_sync::{blocking_mutex::raw::NoopRawMutex, channel::Channel, signal::Signal};
use oer_bluetooth_controller::{LeController, LeControllerConfig};
use oer_bluetooth_hci::bt_hci::{
    ControllerToHostPacket,
    cmd::{
        controller_baseband::Reset,
        le::{LeRand, LeSetAdvEnable, LeSetAdvParams},
    },
    param::{AddrKind, AdvChannelMap, AdvFilterPolicy, AdvKind, BdAddr, Duration as HciDuration},
    transport::Transport,
};
use oer_bluetooth_hci::{BluetoothPublicDeviceAddress, LeControllerBootstrapConfig};
use oer_bluetooth_hci_transport::{
    InProcessHciHostTransport, LeControllerHciEndpoints, LeControllerHciResources,
};

use oer_bluetooth_port::{EnableResult, Lent, PhaseParts, PortBackend, RadioPortEvents, Retire};
use oer_bluetooth_radio::{
    CancelError, ClockError, ClockInfo, ConnectionAllowances, EventId, EventResult, EventsLost,
    LeCapabilities, LeInstant, LePhys, LeRadio, LeRadioBackend, LeRadioCapabilities,
    LegacyAdvertisingTypes, NoCapabilities, NoRadio, Poisoned, PortResult, RadioActivity,
    RadioDuration, RadioOutcome, RadioRequest, RadioTiming, RequestError,
};
use oer_radio_port::Mint;

use oer_time::Duration;
use oer_time_virtual::VirtualClock;

use crate::{Exhaustion, ServeExit, Served, serve};

type Resources = LeControllerHciResources<NoopRawMutex, 4, 4, 258>;
type Host<'c> = InProcessHciHostTransport<'c, NoopRawMutex, 4, 4, 258>;

fn config() -> LeControllerBootstrapConfig {
    LeControllerBootstrapConfig::new(
        BluetoothPublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]),
        251,
        4,
    )
    .unwrap()
}

fn controller_config() -> LeControllerConfig {
    LeControllerConfig {
        bootstrap: config(),
        version: None,
    }
}

fn resources() -> Resources {
    Resources::new(config()).unwrap()
}

/// The status of the next Command Complete.
async fn status(host: &Host<'_>) -> u8 {
    let mut buffer = [0; 258];
    let ControllerToHostPacket::Event(event) = host.read(&mut buffer).await.unwrap() else {
        panic!("event expected");
    };
    assert_eq!(event.kind.0, 0x0e, "Command Complete expected");
    event.data[3]
}

fn nonconnectable() -> LeSetAdvParams {
    LeSetAdvParams::new(
        HciDuration::from_millis(100),
        HciDuration::from_millis(100),
        AdvKind::AdvNonconnInd,
        AddrKind::PUBLIC,
        AddrKind::PUBLIC,
        BdAddr::default(),
        AdvChannelMap::ALL,
        AdvFilterPolicy::default(),
    )
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Recorded {
    ConfigureAdvertising,
    Advertise(EventId),
    Cancel(EventId),
    Other,
}

/// An owned event of the model radio: the event ended without executing.
#[derive(Clone, Copy, Debug, PartialEq)]
struct ModelEvent(EventId);

/// The cause of the poisoned model.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct ModelFault;

const POISONED: Poisoned<ModelFault> = Poisoned { cause: ModelFault };

/// A radio that takes every request, unless told to refuse, and reports the
/// outcomes the test pushes. Its backend and phase parts borrow it.
struct ModelRadio {
    /// The radio epoch's origin.
    started: std::time::Instant,
    now: Cell<Option<LeInstant>>,
    requests: RefCell<Vec<Recorded>>,
    outcomes: Channel<NoopRawMutex, Result<ModelEvent, EventsLost>, 8>,
    /// Admitted events whose end was not taken yet.
    admitted: RefCell<Vec<EventId>>,
    /// Every event the phase's consumer took, in order.
    taken: RefCell<Vec<ModelEvent>>,
    /// Every event `disable` handed to its sink, in order.
    handed: RefCell<Vec<ModelEvent>>,
    /// The transitions, the uninstall and the abandon, in order.
    lifecycle: RefCell<Vec<&'static str>>,
    /// The clock reports the epoch exhausted.
    exhausted: Cell<bool>,
    /// Every call reports the backend poisoned.
    poisoned: Cell<bool>,
    /// Raised when the backend becomes poisoned.
    poisoning: Signal<NoopRawMutex, ()>,
    /// `disable` poisons the backend.
    poison_disable: Cell<bool>,
    refuse: RefCell<Option<RequestError>>,
    submitted: RefCell<usize>,
    activity: RefCell<Vec<RadioActivity>>,
}

impl ModelRadio {
    fn new() -> Self {
        Self {
            requests: RefCell::new(Vec::new()),
            started: std::time::Instant::now(),
            now: Cell::new(None),
            outcomes: Channel::new(),
            admitted: RefCell::new(Vec::new()),
            taken: RefCell::new(Vec::new()),
            handed: RefCell::new(Vec::new()),
            lifecycle: RefCell::new(Vec::new()),
            exhausted: Cell::new(false),
            poisoned: Cell::new(false),
            poisoning: Signal::new(),
            poison_disable: Cell::new(false),
            refuse: RefCell::new(None),
            submitted: RefCell::new(0),
            activity: RefCell::new(Vec::new()),
        }
    }

    fn backend(&self) -> ModelBackend<'_> {
        ModelBackend(self)
    }

    fn poison(&self) {
        self.poisoned.set(true);
        self.poisoning.signal(());
    }

    fn submit(
        &self,
        request: RadioRequest<'_, ModelCaps>,
    ) -> PortResult<(), RequestError, ModelFault> {
        self.check()?;
        *self.submitted.borrow_mut() += 1;
        if let Some(error) = *self.refuse.borrow() {
            return Ok(Err(error));
        }
        self.requests.borrow_mut().push(match request {
            RadioRequest::ConfigureAdvertising(_) => Recorded::ConfigureAdvertising,
            RadioRequest::Advertise(event) => {
                self.admitted.borrow_mut().push(event.get().id);
                Recorded::Advertise(event.get().id)
            }
            _ => Recorded::Other,
        });
        Ok(Ok(()))
    }

    fn clock(&self) -> PortResult<LeInstant, ClockError, ModelFault> {
        self.check()?;
        if self.exhausted.get() {
            return Ok(Err(ClockError::EpochExhausted));
        }
        Ok(Ok(self.now.get().unwrap_or_else(|| {
            LeInstant::from_micros(self.started.elapsed().as_micros() as u64)
        })))
    }

    fn check(&self) -> Result<(), Poisoned<ModelFault>> {
        if self.poisoned.get() {
            Err(POISONED)
        } else {
            Ok(())
        }
    }

    fn advertised(&self) -> Vec<EventId> {
        self.requests
            .borrow()
            .iter()
            .filter_map(|request| match request {
                Recorded::Advertise(id) => Some(*id),
                _ => None,
            })
            .collect()
    }

    async fn until_submitted(&self, count: usize) {
        while *self.submitted.borrow() < count {
            embassy_futures::yield_now().await;
        }
    }

    async fn until(&self, count: usize) {
        while self.advertised().len() < count {
            embassy_futures::yield_now().await;
        }
    }
}

/// The timing the model admits events with.
const TIMING: RadioTiming = RadioTiming {
    preparation_lead: RadioDuration::from_micros(300),
    admission_guard: RadioDuration::from_micros(200),
    connection: ConnectionAllowances {
        local_sleep_clock_ppm: 500,
        widening_jitter: RadioDuration::from_micros(63),
        receive_guard: RadioDuration::from_micros(10),
        receive_tail: RadioDuration::from_micros(2),
        boundary_guard: oer_bluetooth_radio::NonZeroRadioDuration::from_micros(
            core::num::NonZeroU64::MIN,
        ),
        first_event_guard: RadioDuration::from_micros(16),
        event_length: RadioDuration::from_micros(5047),
        first_event_length: RadioDuration::from_micros(5155),
    },
};

/// The backend of the model radio.
struct ModelBackend<'m>(&'m ModelRadio);

/// One part of a phase of the model radio.
struct ModelPart<'m>(&'m ModelRadio);

impl RadioPortEvents for ModelPart<'_> {
    type Event = ModelEvent;
    type Fault = ModelFault;

    async fn next_event(&mut self) -> PortResult<ModelEvent, EventsLost, ModelFault> {
        let radio = self.0;
        radio.check()?;
        let event = match select(radio.outcomes.receive(), radio.poisoning.wait()).await {
            Either::First(event) => event,
            Either::Second(()) => return Err(POISONED),
        };
        if let Ok(event) = event {
            radio
                .admitted
                .borrow_mut()
                .retain(|admitted| *admitted != event.0);
            radio.taken.borrow_mut().push(event);
        }
        Ok(event)
    }
}

impl<'m> PortBackend for ModelBackend<'m> {
    type Event = ModelEvent;
    type Domain = LeRadio;
    type Fault = ModelFault;
    type Refusal = core::convert::Infallible;
    type Setup<'a, 'id>
        = ()
    where
        Self: 'a;
    type Events<'a, 'id>
        = ModelPart<'m>
    where
        Self: 'a,
        'id: 'a;
    type Control<'a, 'id>
        = ModelPart<'m>
    where
        Self: 'a,
        'id: 'a;
    type Clients<'a, 'id>
        = ModelPart<'m>
    where
        Self: 'a,
        'id: 'a;
    type Resources<'id> = ();
    type Held<'id> = ();
    type State = ();
    type Uninstall = ();
    type Uninstalled = ();

    fn enable(
        &mut self,
    ) -> impl Future<Output = EnableResult<core::convert::Infallible, ModelFault>> + '_ {
        let radio = self.0;
        core::future::ready(radio.check().map(|()| {
            radio.lifecycle.borrow_mut().push("enable");
            Ok(())
        }))
    }

    /// Every admitted event ends, after the events the phase left.
    fn disable<'a, 'id>(
        &'a mut self,
        _: &'a Self::Held<'id>,
        sink: &'a mut dyn FnMut(ModelEvent),
    ) -> impl Future<Output = Result<(), Poisoned<ModelFault>>> + 'a
    where
        'id: 'a,
    {
        let radio = self.0;
        async move {
            if radio.poison_disable.get() {
                radio.poison();
            }
            radio.check()?;
            radio.lifecycle.borrow_mut().push("disable");
            let mut hand = |event: ModelEvent| {
                radio
                    .admitted
                    .borrow_mut()
                    .retain(|admitted| *admitted != event.0);
                radio.handed.borrow_mut().push(event);
                sink(event);
            };
            while let Ok(event) = radio.outcomes.try_receive() {
                if let Ok(event) = event {
                    hand(event);
                }
            }
            let admitted: Vec<EventId> = radio.admitted.borrow().clone();
            for id in admitted {
                hand(ModelEvent(id));
            }
            Ok(())
        }
    }

    fn now(&self) -> impl Future<Output = PortResult<LeInstant, ClockError, ModelFault>> + '_ {
        core::future::ready(self.0.clock())
    }

    fn resources<'id>(&mut self, _: Mint<'id>) -> ((), ()) {
        ((), ())
    }

    fn setup<'a, 'id>(&'a mut self) -> Lent<'a, Self::Setup<'a, 'id>> {
        Lent::new(())
    }

    fn split<'a, 'id>(
        &'a mut self,
        _: &'a Self::Held<'id>,
    ) -> PhaseParts<'a, Self::Events<'a, 'id>, Self::Control<'a, 'id>, Self::Clients<'a, 'id>>
    where
        'id: 'a,
    {
        PhaseParts {
            events: Lent::new(ModelPart(self.0)),
            control: Lent::new(ModelPart(self.0)),
            clients: Lent::new(ModelPart(self.0)),
        }
    }

    /// The model serves everything in its calls: nothing to drive.
    fn drive<'p, 'id>(
        &mut self,
        _: &'p Self::Held<'id>,
        _: Retire<'p>,
    ) -> impl Future<Output = ()> + use<'p, 'id, 'm> {
        core::future::ready(())
    }

    fn retire_bound(&self) -> impl Future<Output = ()> + '_ {
        core::future::pending()
    }

    fn detach<'id>(&mut self, (): &()) {
        self.0.lifecycle.borrow_mut().push("detach");
    }

    fn uninstall<'id>(self, (): (), (): (), (): ()) {
        self.0.lifecycle.borrow_mut().push("uninstall");
    }

    fn abandon(&mut self) {
        self.0.lifecycle.borrow_mut().push("abandon");
    }
}

/// The model serves the advertising the tests drive.
#[derive(Debug, PartialEq)]
enum ModelCaps {}

impl LeCapabilities for ModelCaps {
    const CAPS: LeRadioCapabilities = LeRadioCapabilities {
        legacy_advertising: LegacyAdvertisingTypes {
            nonconnectable: true,
            responsive: true,
        },
        phys: LePhys::LE_1M,
        min_tx_power_dbm: 0,
        max_ahead: oer_bluetooth_radio::RadioDuration::from_micros(1 << 30),
        timing: TIMING,
        ..LeRadioCapabilities::NONE
    };
}

impl<'m> LeRadioBackend for ModelBackend<'m> {
    type Caps = ModelCaps;

    const CLOCK: ClockInfo<LeRadio> = ClockInfo::MONOTONIC_MICROS;

    fn view(event: &ModelEvent) -> RadioOutcome<'_> {
        RadioOutcome::EventEnded {
            id: event.0,
            result: EventResult::NotExecuted,
        }
    }

    fn client_now<'c, 'a: 'c, 'id>(
        client: &'c mut Self::Clients<'a, 'id>,
    ) -> impl Future<Output = PortResult<LeInstant, ClockError, ModelFault>> + 'c
    where
        Self: 'a,
        'id: 'a,
    {
        core::future::ready(client.0.clock())
    }

    fn submit<'c, 'a: 'c, 'id>(
        client: &'c mut Self::Clients<'a, 'id>,
        request: RadioRequest<'c, ModelCaps>,
    ) -> impl Future<Output = PortResult<(), RequestError, ModelFault>> + 'c
    where
        Self: 'a,
        'id: 'a,
    {
        core::future::ready(client.0.submit(request))
    }

    /// The event's end follows from the test.
    fn cancel<'c, 'a: 'c, 'id>(
        client: &'c mut Self::Clients<'a, 'id>,
        id: EventId,
    ) -> impl Future<Output = PortResult<(), CancelError, ModelFault>> + 'c
    where
        Self: 'a,
        'id: 'a,
    {
        let radio = client.0;
        core::future::ready(radio.check().map(|()| {
            radio.requests.borrow_mut().push(Recorded::Cancel(id));
            Ok(())
        }))
    }

    fn activity<'a, 'id>(
        control: &Self::Control<'a, 'id>,
        activity: RadioActivity,
    ) -> Result<(), Poisoned<ModelFault>>
    where
        Self: 'a,
        'id: 'a,
    {
        control.0.check()?;
        control.0.activity.borrow_mut().push(activity);
        Ok(())
    }
}

#[test]
fn serves_bootstrap_and_ends_when_the_transport_closes() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, NoCapabilities, 12, 1>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let ((exit, uninstalled), ()) = block_on(join(
        serve(NoRadio, (), &controller, &mut core, &clock, pending()),
        async {
            host.write(&Reset::new()).await.unwrap();
            assert_eq!(status(&host).await, 0x00);
            // No random source: LE Rand is unknown.
            host.write(&LeRand::new()).await.unwrap();
            assert_eq!(status(&host).await, 0x01);
            controller.close();
        },
    ));
    assert_eq!(
        (exit, uninstalled),
        (Served::Stopped(ServeExit::Closed), Some(()))
    );
}

#[test]
fn a_radio_without_hardware_fails_advertising_enable() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, NoCapabilities, 12, 1>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let ((exit, uninstalled), ()) = block_on(join(
        serve(NoRadio, (), &controller, &mut core, &clock, pending()),
        async {
            host.write(&Reset::new()).await.unwrap();
            status(&host).await;
            host.write(&nonconnectable()).await.unwrap();
            assert_eq!(status(&host).await, 0x00);
            // A radio that serves no advertising: the core answers the
            // Host as Unsupported Feature or Parameter Value before any
            // request reaches the port.
            host.write(&LeSetAdvEnable::new(true)).await.unwrap();
            assert_eq!(status(&host).await, 0x11);
            controller.close();
        },
    ));
    assert_eq!(
        (exit, uninstalled),
        (Served::Stopped(ServeExit::Closed), Some(()))
    );
}

#[test]
fn advertising_events_follow_their_outcomes() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, ModelCaps, 12, 1>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let radio = ModelRadio::new();
    let ((exit, uninstalled), ()) = block_on(join(
        serve(
            radio.backend(),
            (),
            &controller,
            &mut core,
            &clock,
            pending(),
        ),
        async {
            host.write(&Reset::new()).await.unwrap();
            status(&host).await;
            host.write(&nonconnectable()).await.unwrap();
            status(&host).await;
            host.write(&LeSetAdvEnable::new(true)).await.unwrap();
            assert_eq!(status(&host).await, 0x00);
            assert_eq!(radio.requests.borrow()[0], Recorded::ConfigureAdvertising);

            radio.until(1).await;
            let first = radio.advertised()[0];
            // The next event waits for the first one's end.
            for _ in 0..16 {
                embassy_futures::yield_now().await;
            }
            assert_eq!(radio.advertised().len(), 1);
            radio.outcomes.send(Ok(ModelEvent(first))).await;
            radio.until(2).await;
            assert_ne!(radio.advertised()[1], first);

            // A loss holds received PDUs only: the service goes on, and the
            // second event's end still plans the third.
            radio.outcomes.send(Err(EventsLost)).await;
            let second = radio.advertised()[1];
            radio.outcomes.send(Ok(ModelEvent(second))).await;
            radio.until(3).await;
            controller.close();
        },
    ));
    assert_eq!(
        (exit, uninstalled),
        (Served::Stopped(ServeExit::Closed), Some(()))
    );
}

#[test]
fn refused_requests_are_retried_after_a_delay() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, ModelCaps, 12, 1>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let radio = ModelRadio::new();
    let ((exit, uninstalled), ()) = block_on(join(
        serve(
            radio.backend(),
            (),
            &controller,
            &mut core,
            &clock,
            pending(),
        ),
        async {
            host.write(&Reset::new()).await.unwrap();
            status(&host).await;
            host.write(&nonconnectable()).await.unwrap();
            status(&host).await;
            host.write(&LeSetAdvEnable::new(true)).await.unwrap();
            status(&host).await;
            radio.until(1).await;
            let first = radio.advertised()[0];
            *radio.refuse.borrow_mut() = Some(RequestError::Busy);
            let before = *radio.submitted.borrow();
            radio.outcomes.send(Ok(ModelEvent(first))).await;
            radio.until_submitted(before + 1).await;
            // Refused: no retry before the delay.
            for _ in 0..8 {
                embassy_futures::yield_now().await;
            }
            assert_eq!(*radio.submitted.borrow(), before + 1);
            *radio.refuse.borrow_mut() = None;
            clock.advance(Duration::from_millis(1)).unwrap();
            radio.until(2).await;
            controller.close();
        },
    ));
    assert_eq!(
        (exit, uninstalled),
        (Served::Stopped(ServeExit::Closed), Some(()))
    );
}

#[test]
fn a_port_poisoned_before_enable_is_fail_stopped_with_its_cause() {
    let mut resources = resources();
    let LeControllerHciEndpoints { controller, .. } = resources.split();
    let mut core = LeController::<'_, ModelCaps, 12, 1>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let radio = ModelRadio::new();
    radio.poison();
    assert_eq!(
        block_on(serve(
            radio.backend(),
            (),
            &controller,
            &mut core,
            &clock,
            pending()
        )),
        (Served::Poisoned(ModelFault), None)
    );
    assert_eq!(*radio.lifecycle.borrow(), ["abandon"]);
}

#[test]
fn a_port_poisoned_while_serving_is_fail_stopped_without_a_disable() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, ModelCaps, 12, 1>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let radio = ModelRadio::new();
    let ((exit, uninstalled), ()) = block_on(join(
        serve(
            radio.backend(),
            (),
            &controller,
            &mut core,
            &clock,
            pending(),
        ),
        async {
            advertise(&host, &radio).await;
            radio.poison();
        },
    ));
    assert_eq!((exit, uninstalled), (Served::Poisoned(ModelFault), None));
    assert_eq!(*radio.lifecycle.borrow(), ["enable", "abandon"]);
    assert!(radio.handed.borrow().is_empty());
}

#[test]
fn host_data_without_a_connection_does_not_block_commands() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, NoCapabilities, 12, 1>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let ((exit, uninstalled), ()) = block_on(join(
        serve(NoRadio, (), &controller, &mut core, &clock, pending()),
        async {
            host.write(&Reset::new()).await.unwrap();
            assert_eq!(status(&host).await, 0x00);
            let data = [1, 2, 3];
            host.write(&oer_bluetooth_hci::bt_hci::data::AclPacket::new(
                oer_bluetooth_hci::bt_hci::param::ConnHandle::new(0),
                oer_bluetooth_hci::bt_hci::data::AclPacketBoundary::FirstNonFlushable,
                oer_bluetooth_hci::bt_hci::data::AclBroadcastFlag::PointToPoint,
                &data,
            ))
            .await
            .unwrap();
            host.write(&LeRand::new()).await.unwrap();
            assert_eq!(status(&host).await, 0x01);
            controller.close();
        },
    ));
    assert_eq!(
        (exit, uninstalled),
        (Served::Stopped(ServeExit::Closed), Some(()))
    );
}

#[test]
fn advertising_enable_and_disable_report_the_active_roles() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, ModelCaps, 12, 1>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let radio = ModelRadio::new();
    let advertising = RadioActivity {
        advertising: true,
        ..RadioActivity::IDLE
    };
    let ((exit, uninstalled), ()) = block_on(join(
        serve(
            radio.backend(),
            (),
            &controller,
            &mut core,
            &clock,
            pending(),
        ),
        async {
            host.write(&Reset::new()).await.unwrap();
            status(&host).await;
            host.write(&nonconnectable()).await.unwrap();
            status(&host).await;
            // Nothing active: nothing reported.
            assert!(radio.activity.borrow().is_empty());
            host.write(&LeSetAdvEnable::new(true)).await.unwrap();
            assert_eq!(status(&host).await, 0x00);
            radio.until(1).await;
            assert_eq!(*radio.activity.borrow(), [advertising]);

            // The disable cancels the event in flight through the port's
            // cancellation and completes once it has ended; advertising stays
            // active until then.
            host.write(&LeSetAdvEnable::new(false)).await.unwrap();
            let last = *radio.advertised().last().expect("an event was requested");
            while !radio.requests.borrow().contains(&Recorded::Cancel(last)) {
                embassy_futures::yield_now().await;
            }
            assert_eq!(*radio.activity.borrow(), [advertising]);
            radio.outcomes.send(Ok(ModelEvent(last))).await;
            assert_eq!(status(&host).await, 0x00);
            assert_eq!(*radio.activity.borrow(), [advertising, RadioActivity::IDLE]);
            controller.close();
        },
    ));
    assert_eq!(
        (exit, uninstalled),
        (Served::Stopped(ServeExit::Closed), Some(()))
    );
}

#[test]
fn required_epoch_exhaustion_exits_with_context_and_retains_the_core() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, ModelCaps, 12, 1>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let radio = ModelRadio::new();
    let ((exit, uninstalled), ()) = block_on(join(
        serve(
            radio.backend(),
            (),
            &controller,
            &mut core,
            &clock,
            pending(),
        ),
        async {
            host.write(&Reset::new()).await.unwrap();
            status(&host).await;
            host.write(&nonconnectable()).await.unwrap();
            status(&host).await;
            host.write(&LeSetAdvEnable::new(true)).await.unwrap();
            assert_eq!(status(&host).await, 0);
            radio.until(1).await;
            radio.now.set(Some(LeInstant::from_micros(u64::MAX)));
            radio
                .outcomes
                .send(Ok(ModelEvent(radio.advertised()[0])))
                .await;
        },
    ));
    let Served::Stopped(ServeExit::EpochExhausted(Exhaustion::Planning(error))) = exit else {
        panic!("required planning must end service: {exit:?}")
    };
    assert_eq!(uninstalled, Some(()));
    assert_eq!(
        error.role,
        oer_bluetooth_controller::PlanningRole::Advertising
    );
    assert_eq!(
        error.calculation,
        oer_bluetooth_controller::PlanningCalculation::Admission
    );
    assert_eq!(radio.advertised().len(), 1);
    assert_ne!(
        core.activity(),
        RadioActivity::IDLE,
        "lifecycle retains the active logical owner"
    );
}

/// Start advertising through `host` and wait for the first event.
async fn advertise(host: &Host<'_>, radio: &ModelRadio) {
    host.write(&Reset::new()).await.unwrap();
    status(host).await;
    host.write(&nonconnectable()).await.unwrap();
    status(host).await;
    host.write(&LeSetAdvEnable::new(true)).await.unwrap();
    assert_eq!(status(host).await, 0);
    radio.until(1).await;
}

#[test]
fn a_requested_stop_ends_every_admitted_event_once_before_returning() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, ModelCaps, 12, 1>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let radio = ModelRadio::new();
    let stop = Signal::<NoopRawMutex, ()>::new();
    let ((exit, uninstalled), ()) = block_on(join(
        serve(
            radio.backend(),
            (),
            &controller,
            &mut core,
            &clock,
            stop.wait(),
        ),
        async {
            advertise(&host, &radio).await;
            stop.signal(());
        },
    ));
    assert_eq!(
        (exit, uninstalled),
        (Served::Stopped(ServeExit::Stopped), Some(()))
    );
    let first = radio.advertised()[0];
    // The disable hands the admitted event's end to the core, once.
    assert!(radio.taken.borrow().is_empty());
    assert_eq!(*radio.handed.borrow(), [ModelEvent(first)]);
    assert!(radio.admitted.borrow().is_empty());
    assert_eq!(
        *radio.lifecycle.borrow(),
        ["enable", "disable", "detach", "uninstall"]
    );
    // Nothing was submitted after the stop began.
    assert_eq!(radio.advertised(), [first]);
}

#[test]
fn an_exhausted_clock_stops_the_service_and_accounts_the_admitted_event() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, ModelCaps, 12, 1>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let radio = ModelRadio::new();
    let ((exit, uninstalled), ()) = block_on(join(
        serve(
            radio.backend(),
            (),
            &controller,
            &mut core,
            &clock,
            pending(),
        ),
        async {
            advertise(&host, &radio).await;
            radio.exhausted.set(true);
            let first = radio.advertised()[0];
            radio.outcomes.send(Ok(ModelEvent(first))).await;
        },
    ));
    assert_eq!(
        (exit, uninstalled),
        (
            Served::Stopped(ServeExit::EpochExhausted(Exhaustion::Clock)),
            Some(())
        )
    );
    assert_eq!(radio.advertised().len(), 1);
    assert_eq!(
        *radio.lifecycle.borrow(),
        ["enable", "disable", "detach", "uninstall"]
    );
}

#[test]
fn an_exhausted_admission_stops_without_a_retry() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, ModelCaps, 12, 1>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let radio = ModelRadio::new();
    let ((exit, uninstalled), ()) = block_on(join(
        serve(
            radio.backend(),
            (),
            &controller,
            &mut core,
            &clock,
            pending(),
        ),
        async {
            advertise(&host, &radio).await;
            *radio.refuse.borrow_mut() = Some(RequestError::EpochExhausted);
            let first = radio.advertised()[0];
            radio.outcomes.send(Ok(ModelEvent(first))).await;
        },
    ));
    assert_eq!(
        (exit, uninstalled),
        (
            Served::Stopped(ServeExit::EpochExhausted(Exhaustion::Clock)),
            Some(())
        )
    );
    // The configuration, the first event and the one refused submission.
    assert_eq!(*radio.submitted.borrow(), 3);
}

#[test]
fn a_disable_that_poisons_keeps_why_the_service_stopped() {
    let mut resources = resources();
    let LeControllerHciEndpoints { host, controller } = resources.split();
    let mut core = LeController::<'_, ModelCaps, 12, 1>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    let radio = ModelRadio::new();
    radio.poison_disable.set(true);
    let ((exit, uninstalled), ()) = block_on(join(
        serve(
            radio.backend(),
            (),
            &controller,
            &mut core,
            &clock,
            pending(),
        ),
        async {
            advertise(&host, &radio).await;
            radio.exhausted.set(true);
            let first = radio.advertised()[0];
            radio.outcomes.send(Ok(ModelEvent(first))).await;
        },
    ));
    assert_eq!(
        (exit, uninstalled),
        (
            Served::DisableFailed {
                exit: ServeExit::EpochExhausted(Exhaustion::Clock),
                cause: ModelFault,
            },
            None
        )
    );
    assert_eq!(*radio.lifecycle.borrow(), ["enable", "abandon"]);
}

#[test]
fn a_port_that_admits_nothing_stops_at_once() {
    let mut resources = resources();
    let LeControllerHciEndpoints { controller, .. } = resources.split();
    let mut core = LeController::<'_, NoCapabilities, 12, 1>::new(controller_config(), None);
    let clock: VirtualClock = VirtualClock::new();
    assert_eq!(
        block_on(serve(
            NoRadio,
            (),
            &controller,
            &mut core,
            &clock,
            core::future::ready(())
        )),
        (Served::Stopped(ServeExit::Stopped), Some(()))
    );
}
