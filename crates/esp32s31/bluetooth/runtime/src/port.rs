//! The port backend of an installed runtime, for the portable Controller
//! service body.

use core::{convert::Infallible, future::Future};

use embassy_sync::blocking_mutex::raw::RawMutex;
use oer_bluetooth_port::{EnableResult, Lent, PhaseParts, PortBackend, RadioPortEvents, Retire};
use oer_bluetooth_radio::{
    CancelError, ClockError, ClockInfo, EventId, EventsLost, LeInstant, LeRadio, LeRadioBackend,
    Poisoned, PortResult, RadioActivity, RadioOutcome, RadioRequest, RequestError,
};
use oer_esp32s31_bluetooth_radio::{BluetoothCapabilities, BluetoothRadioConfig};
use oer_radio_port::Mint;
use oer_time::Timer;

use crate::runtime::Radio;
use crate::{
    BluetoothDriver, BluetoothFault, BluetoothOutcome, BluetoothRadioHardware, BluetoothRetained,
    BluetoothRuntime, TRANSITION_BOUND,
};

/// The port backend of an installed [`BluetoothRuntime`]: what
/// [`oer_bluetooth_port::run_port`] wraps in the port's owner.
///
/// [`BluetoothRuntime::install`] returns it with the radio's driver `D`
/// ([`BluetoothDriver`]) inside, so no backend meets a runtime without a
/// radio: no call is refused as not installed. It is neither `Copy` nor
/// `Clone` and has no public constructor. [`PortBackend::drive`] moves the
/// driver, which owns the radio and its hardware, into the future
/// `run_port` joins with the body; the backend's calls are then requests to
/// it, and the interrupt entry only wakes it. The uninstall takes the
/// retired driver's radio and hardware out by value.
#[must_use = "an installed radio leaves only through its owner's uninstall or fail-stop"]
pub struct BluetoothBackend<'r, R, D> {
    runtime: &'r R,
    /// `None` once [`PortBackend::drive`] took it.
    driver: Option<D>,
}

impl<'r, R, D> BluetoothBackend<'r, R, D> {
    pub(crate) const fn new(runtime: &'r R, driver: D) -> Self {
        Self {
            runtime,
            driver: Some(driver),
        }
    }

    /// Take the driver out, for host tests that drive it themselves.
    #[cfg(test)]
    pub(crate) fn take_driver(&mut self) -> D {
        self.driver.take().expect("the driver is taken once")
    }
}

/// The event consumer of one running phase.
pub struct BluetoothEvents<'a, R> {
    runtime: &'a R,
}

/// The control of one running phase: it publishes the Controller's active
/// roles.
pub struct BluetoothPhaseControl<'a, R> {
    runtime: &'a R,
}

/// The Controller core's client of one running phase: it submits and
/// cancels events and reads the radio clock.
pub struct BluetoothClient<'a, R> {
    runtime: &'a R,
}

impl<
    'r,
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
> PortBackend
    for BluetoothBackend<
        'r,
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
        >,
        BluetoothDriver<
            'r,
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
    >
{
    type Event = BluetoothOutcome;
    type Domain = LeRadio;
    type Fault = BluetoothFault<H::StartError>;
    type Refusal = Infallible;
    type Setup<'a, 'id>
        = ()
    where
        Self: 'a;
    type Events<'a, 'id>
        = BluetoothEvents<
        'a,
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
        >,
    >
    where
        Self: 'a,
        'id: 'a;
    type Control<'a, 'id>
        = BluetoothPhaseControl<
        'a,
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
        >,
    >
    where
        Self: 'a,
        'id: 'a;
    type Clients<'a, 'id>
        = BluetoothClient<
        'a,
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
        >,
    >
    where
        Self: 'a,
        'id: 'a;
    type Resources<'id> = ();
    /// The shared part lives in the runtime the backend and the driver
    /// both reach.
    type Held<'id> = ();
    /// The driver, which owns the radio and its hardware.
    type State = BluetoothDriver<
        'r,
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
    >;
    type Uninstall = ();
    /// The radio and hardware, or the radio the driver did not leave for
    /// the uninstall, with why.
    type Uninstalled = Result<
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
    >;

    fn enable(&mut self) -> impl Future<Output = EnableResult<Infallible, Self::Fault>> + '_ {
        self.runtime.enable()
    }

    fn disable<'a, 'id>(
        &'a mut self,
        (): &'a (),
        sink: &'a mut dyn FnMut(BluetoothOutcome),
    ) -> impl Future<Output = Result<(), Poisoned<Self::Fault>>> + 'a
    where
        'id: 'a,
    {
        self.runtime.disable(sink)
    }

    fn now(&self) -> impl Future<Output = PortResult<LeInstant, ClockError, Self::Fault>> + '_ {
        self.runtime.read_now()
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
        (): &'a (),
    ) -> PhaseParts<'a, Self::Events<'a, 'id>, Self::Control<'a, 'id>, Self::Clients<'a, 'id>>
    where
        'id: 'a,
    {
        PhaseParts {
            events: Lent::new(BluetoothEvents {
                runtime: self.runtime,
            }),
            control: Lent::new(BluetoothPhaseControl {
                runtime: self.runtime,
            }),
            clients: Lent::new(BluetoothClient {
                runtime: self.runtime,
            }),
        }
    }

    /// The driver runs until the uninstall's retirement: it stops a
    /// scheduler a poisoned owner left running, drains a controller-time
    /// request in flight and restores the BLE PHY route, or it returns
    /// early on a fault, which poisoned the port first.
    fn drive<'p, 'id>(
        &mut self,
        (): &'p (),
        retire: Retire<'p>,
    ) -> impl Future<Output = Self::State>
    + use<
        'p,
        'id,
        'r,
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
    > {
        let mut driver = self.driver.take().expect("run_port drives a backend once");
        async move {
            // A fault is the driver's own exit: it poisoned the port, and
            // the uninstall finds the radio retained.
            let _stopped = driver.run(|| retire.is_requested()).await;
            driver
        }
    }

    fn retire_bound(&self) -> impl Future<Output = ()> + '_ {
        let timer = self.runtime.timer();
        async move {
            // A bound the timer cannot express has passed at once.
            let _ = timer.wait_for(TRANSITION_BOUND).await;
        }
    }

    /// The Bluetooth interrupt entry reaches only the runtime's wake
    /// signal, never the install's state: nothing to detach. The
    /// composition removes the CPU routes after the uninstall.
    fn detach<'id>(&mut self, (): &()) {}

    fn uninstall<'id>(self, (): (), driver: Self::State, (): ()) -> Self::Uninstalled {
        driver.into_parts()
    }

    fn abandon(&mut self) {
        self.runtime.abandon();
    }
}

impl<
    'r,
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
> LeRadioBackend
    for BluetoothBackend<
        'r,
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
        >,
        BluetoothDriver<
            'r,
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
    >
{
    type Caps = BluetoothCapabilities<
        C,
        LEGACY,
        CONNECTABLE,
        SCANNERS,
        CONNECTIONS,
        SCAN_PACKETS,
        RX_PACKETS,
        ITEMS,
    >;

    /// The radio time is the controller clock, extended from the 32-bit
    /// controller-time latch the radio samples; the runtime keeps no
    /// measured relation between it and the image's monotonic clock.
    const CLOCK: ClockInfo<LeRadio> = ClockInfo {
        resolution: ClockInfo::<LeRadio>::MONOTONIC_MICROS.resolution,
        monotonic: None,
    };

    fn view(outcome: &BluetoothOutcome) -> RadioOutcome<'_> {
        outcome.portable()
    }

    fn client_now<'c, 'a: 'c, 'id>(
        client: &'c mut Self::Clients<'a, 'id>,
    ) -> impl Future<Output = PortResult<LeInstant, ClockError, Self::Fault>> + 'c
    where
        Self: 'a,
        'id: 'a,
    {
        client.runtime.read_now()
    }

    fn submit<'c, 'a: 'c, 'id>(
        client: &'c mut Self::Clients<'a, 'id>,
        request: RadioRequest<'c, Self::Caps>,
    ) -> impl Future<Output = PortResult<(), RequestError, Self::Fault>> + 'c
    where
        Self: 'a,
        'id: 'a,
    {
        client.runtime.submit_request(request)
    }

    fn cancel<'c, 'a: 'c, 'id>(
        client: &'c mut Self::Clients<'a, 'id>,
        id: EventId,
    ) -> impl Future<Output = PortResult<(), CancelError, Self::Fault>> + 'c
    where
        Self: 'a,
        'id: 'a,
    {
        client.runtime.cancel_event(id)
    }

    fn activity<'a, 'id>(
        control: &Self::Control<'a, 'id>,
        activity: RadioActivity,
    ) -> Result<(), Poisoned<Self::Fault>>
    where
        Self: 'a,
        'id: 'a,
    {
        control.runtime.publish_activity(activity);
        Ok(())
    }
}

impl<
    'a,
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
> RadioPortEvents
    for BluetoothEvents<
        'a,
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
        >,
    >
{
    type Event = BluetoothOutcome;
    type Fault = BluetoothFault<H::StartError>;

    fn next_event(
        &mut self,
    ) -> impl Future<Output = PortResult<BluetoothOutcome, EventsLost, Self::Fault>> + '_ {
        self.runtime.wait_event()
    }
}
