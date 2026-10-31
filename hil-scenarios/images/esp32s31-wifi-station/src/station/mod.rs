//! The Wi-Fi station service over the product station facade
//! (`oer-esp32s31-wifi`) and IPv4 by DHCP (`oer_embassy_net::ip`).
//!
//! The boot composes the shared RF actor, the radio executor and the Wi-Fi
//! interrupt, and keeps every owner the station's construction takes. The
//! station work answers each request; the boot's one join constructs the
//! station for its channel, spawns its driver on the radio executor and runs
//! the station and the IP stack beside the requests until the station ends.
//! Nothing radiates before the join. The load server shares the work's
//! task and opens its sockets on the IP stack the work lends it.

mod future;
pub(crate) mod ip;
pub(crate) mod irq;
pub(crate) mod radio;

use core::cell::Cell;
use core::convert::Infallible;
use core::sync::atomic::{AtomicBool, Ordering};

use embassy_futures::{
    join::join as both,
    select::{Either, select},
};
use oer_esp32s31_compose::Esp32s31;
use oer_esp32s31_wifi as wifi;
use oer_espressif_executor::task::Spawner;
use oer_hil_protocol::RequestIdentity;
use oer_memory::{
    PlacedFuture, StaticPlace, WakingSpscQueue, WakingSpscReceiver, WakingSpscSender,
};
use oer_services_wifi_station::runtime::{self as answers, class};
use oer_services_wifi_station::{Ipv4, Join, Joined, Left, Request, StationError, Status};
use static_cell::{ConstStaticCell, StaticCell};

use crate::fatal::Failure;
use crate::load::parts::{Part, Polled};
use crate::requests::{Action, Answer, Phase, action};
use crate::transport::CONSOLE;
use future::{Boundary, Tracked};

/// The regulatory domain the station installs.
const COUNTRY: [u8; 2] = *b"DE";
/// The owner's transmit power cap, below the domain's ceiling.
const TX_CAP_DBM: i8 = 10;
/// The correlation of the boot's one connect command.
const JOIN: u64 = 1;

oer_espressif_executor::tasks! {
    /// The radio executor's tasks (P2): the shared RF actor from the boot,
    /// the station's driver from its join.
    pub(crate) RadioTasks: InterruptExecutor<Esp32s31, 2> {
        @observer: crate::load::cpu::Observer::<{ crate::load::cpu::RADIO }>;
        radio_actor(actor: &'static mut Actor) = radio_actor;
        driver(driver: wifi::Driver<radio::binding::Binding>) = radio::drive;
    }
}

pub(crate) type Actor = wifi::RadioActor<crate::phy_pilot::Client>;
pub(crate) static ACTOR: StaticCell<Actor> = StaticCell::new();

async fn radio_actor(actor: &'static mut Actor) {
    let run = core::pin::pin!(actor.run());
    match (Polled {
        part: Part::RadioActor,
        future: run,
    })
    .await {}
}

type Queue = WakingSpscQueue<(RequestIdentity, Request), 1>;
static REQUESTS: ConstStaticCell<Queue> = ConstStaticCell::new(WakingSpscQueue::new());
/// The console's end of the request queue.
pub(crate) type Sender = WakingSpscSender<'static, (RequestIdentity, Request), 1>;
type Receiver = WakingSpscReceiver<'static, (RequestIdentity, Request), 1>;

/// The request queue's two ends, split once.
pub(crate) fn requests() -> (Sender, Receiver) {
    REQUESTS
        .try_take()
        .unwrap_or_else(|| crate::fail(Failure::Storage))
        .split()
}

/// Hand `request` to the station work, or refuse it while one waits.
pub(crate) fn queue(requests: &Sender, identity: RequestIdentity, request: Request) {
    if requests.try_send((identity, request)).is_err() {
        use oer_hil_protocol::base::{RejectReason, Rejected};
        emit(identity, &Rejected(RejectReason::Busy));
    }
}

/// Every owner the station's construction takes, kept from the boot.
pub(crate) struct Deferred {
    pub engine: oer_esp32s31_compose::crypto::KeyUnwrapEngine,
    pub hardware: wifi::WifiHardware,
    pub rng: oer_esp32s31_compose::platform::Entropy,
    pub address: [u8; 6],
    pub ap: [u8; 6],
    pub bound: oer_espressif_interrupt_table::Bound<crate::WifiToken>,
    pub bell: wifi::Bell<crate::WifiToken, crate::RadioToken>,
    pub entropy: u32,
    pub dma: wifi::DmaResources,
}

/// What the station work starts with.
pub(crate) struct Inputs {
    pub requests: Receiver,
    pub deferred: Deferred,
    pub radio: Spawner<RadioTasks>,
    pub seed: u64,
    pub nonces: wifi::NonceSource,
}

pub(crate) type Work = impl core::future::Future<Output = ()>;
static WORK: ConstStaticCell<StaticPlace<Work>> = ConstStaticCell::new(StaticPlace::new());

/// The station work and the load server beside it, with its sampler,
/// written in their own static place.
pub(crate) fn place(
    inputs: Inputs,
    load: crate::load::serve::Receiver,
    sampler: crate::load::sampler::Sampler,
) -> PlacedFuture<'static, Work> {
    WORK.try_take()
        .unwrap_or_else(|| crate::fail(Failure::Storage))
        .future(|| work_future(inputs, load, sampler))
}

#[define_opaque(Work)]
fn work_future(
    inputs: Inputs,
    load: crate::load::serve::Receiver,
    sampler: crate::load::sampler::Sampler,
) -> Work {
    async move {
        let network = crate::load::serve::Network::new();
        let station = core::pin::pin!(work(inputs, &network));
        let load = core::pin::pin!(crate::load::serve::serve(load, &network, sampler));
        let load = Polled {
            part: Part::Load,
            future: Boundary(load),
        };
        match select(Boundary(station), load).await {
            Either::First(never) | Either::Second(never) => match never {},
        }
    }
}

/// The station's task: its work lives in its own static place.
pub(crate) async fn run(work: PlacedFuture<'static, Work>) {
    Polled {
        part: Part::StationWork,
        future: work,
    }
    .await;
}

/// The station's callbacks: the boot-seeded nonce stream.
pub struct Callbacks(wifi::NonceSource);

impl wifi::Callbacks for Callbacks {
    fn nonce(&mut self) -> [u8; 32] {
        self.0
            .next_nonce()
            .unwrap_or_else(|_| crate::fail(Failure::NonceExhausted))
    }
}

/// Set once the boot's join is answered: by its connect's terminal or by
/// the station's end, whichever comes first.
static JOIN_ANSWERED: AtomicBool = AtomicBool::new(false);

/// Answer the boot's join `identity` with `joined`, unless it was answered.
fn answer_join(identity: RequestIdentity, joined: &Joined) {
    if !JOIN_ANSWERED.swap(true, Ordering::Relaxed) {
        emit(identity, joined);
    }
}

/// The station's product events: the terminal of the join's connect.
pub struct Events {
    join: RequestIdentity,
}

impl wifi::Events for Events {
    fn event(&mut self, event: wifi::Event) {
        match event {
            wifi::Event::CommandCompleted(terminal) if terminal.request.correlation() == JOIN => {
                answer_join(self.join, &answers::joined(&terminal.outcome));
            }
            wifi::Event::CommandRefused(refused) if refused.request.correlation() == JOIN => {
                answer_join(
                    self.join,
                    &Joined::Refused(answers::refused(refused.reason)),
                );
            }
            _ => {}
        }
    }
}

async fn work(inputs: Inputs, network: &crate::load::serve::Network) -> Infallible {
    let Inputs {
        mut requests,
        deferred,
        radio,
        seed,
        nonces,
    } = inputs;
    let address = deferred.address;
    let down = || Status {
        address,
        link: None,
        ipv4: None,
    };

    // Idle until a join with valid settings.
    let (join, settings, config) = loop {
        let (identity, join) = take_join(&mut requests, down).await;
        match validate(&join) {
            Ok((settings, config)) => break (identity, settings, config),
            Err(error) => emit(identity, &Joined::Refused(error)),
        }
    };

    // Prepare the credentials while answering requests; a leave ends the join.
    let no_leave = Cell::new(None);
    let prepared = select(
        settings.prepare(),
        take_leave(&mut requests, &no_leave, down),
    )
    .await;
    let credentials = match prepared {
        Either::First(Ok(credentials)) => credentials,
        Either::First(Err(error)) => {
            answer_join(join, &Joined::Refused(credentials_failed(error)));
            stopped(&mut requests, down).await
        }
        Either::Second(leave) => {
            answer_join(join, &Joined::Refused(ended(&wifi::StationExit::Stopped)));
            emit(leave, &Left::Down);
            stopped(&mut requests, down).await
        }
    };
    let profile = match wifi::Profile::<radio::binding::Binding>::new(
        &config,
        address,
        credentials,
        Callbacks(nonces),
        oer_trace::Noop,
        (),
    ) {
        Ok(profile) => profile,
        Err(error) => {
            answer_join(join, &Joined::Refused(initialization_failed(&error)));
            stopped(&mut requests, down).await
        }
    };

    // Construct the station for the join's channel and start its driver.
    let Deferred {
        engine,
        hardware,
        rng,
        address,
        ap,
        bound,
        bell,
        entropy,
        dma,
    } = deferred;
    let (mut control, net, runners) = wifi::station(
        wifi::Resources {
            engine,
            events: Events { join },
            storage: radio::binding::storage(),
            hardware,
            rng,
            address,
            ap,
            bound,
            bell,
            entropy,
            interrupts: &irq::LINE,
            mac: &irq::MAC_SIGNAL,
            power: &irq::POWER_SIGNAL,
            dma,
        },
        config,
    );
    if RadioTasks::driver(&radio, runners.driver).is_err() {
        crate::fail(Failure::TaskAllocation);
    }
    let (ip, interface, running) = match ip::start(address, seed, net) {
        Ok(started) => started,
        Err(error) => {
            answer_join(join, &Joined::Refused(network_failed(error)));
            stopped(&mut requests, down).await
        }
    };
    network.lend(interface);
    if let Err(error) = control.try_connect(JOIN) {
        answer_join(join, &Joined::Refused(answers::not_admitted(error)));
    }

    // Run the station and the IP stack beside the requests until the station
    // ends: by a leave, or by its own failure.
    let status = control.status_reader();
    let up = || Status {
        address,
        link: answers::link(&status.status()),
        ipv4: ipv4(ip.status().ipv4),
    };
    let leaving = Cell::new(None);
    let network_done = Cell::new(false);
    let mut running = running;
    let mut station = core::pin::pin!(runners.station.run(profile));
    let exit = {
        let serving = core::pin::pin!(serve_running(&mut requests, &mut control, &leaving, up));
        match select(
            Polled {
                part: Part::Station,
                future: Boundary(station.as_mut()),
            },
            both(
                Polled {
                    part: Part::Ip,
                    future: Tracked {
                        future: core::pin::Pin::new(&mut running),
                        done: &network_done,
                    },
                },
                Polled {
                    part: Part::Requests,
                    future: serving,
                },
            ),
        )
        .await
        {
            Either::First(exit) => exit,
            Either::Second((_, never)) => match never {},
        }
    };
    answer_join(join, &Joined::Refused(ended(&exit)));
    if let Some(leave) = leaving.get() {
        match exit {
            wifi::StationExit::Stopped => emit(leave, &Left::Down),
            exit => emit(leave, &Left::Refused(ended(&exit))),
        }
    }
    ip.stop();
    Tracked {
        future: core::pin::Pin::new(&mut running),
        done: &network_done,
    }
    .await;
    network.withdraw();
    stopped(&mut requests, down).await
}

/// The join's settings: WPA2-Personal with its passphrase on its channel in
/// the image's regulatory domain, without automatic reconnect.
fn validate(join: &Join) -> Result<(wifi::StationSettings, wifi::Config), StationError> {
    let settings =
        wifi::StationSettings::wpa2_passphrase(join.ssid.as_bytes(), join.passphrase.as_bytes())
            .map_err(|_| StationError::InvalidSettings)?;
    // A trace build's configuration names no access point to observe.
    #[cfg(oer_trace)]
    let config = wifi::Config::new(join.channel, COUNTRY, TX_CAP_DBM, None, false);
    #[cfg(not(oer_trace))]
    let config = wifi::Config::new(join.channel, COUNTRY, TX_CAP_DBM, false);
    let config = config.map_err(|_| StationError::InvalidSettings)?;
    Ok((settings, config))
}

/// The next request, waiting for one.
async fn next(requests: &mut Receiver) -> (RequestIdentity, Request) {
    loop {
        requests.ready().await;
        if let Some(request) = requests.try_receive() {
            return request;
        }
    }
}

/// Answer what `phase` answers by itself and return the join or leave it
/// takes. Every answer is published without waiting, so a request taken
/// from the queue is answered even when its server is dropped next.
fn answer(
    identity: RequestIdentity,
    action: Action,
    status: impl FnOnce() -> Status,
) -> Option<Action> {
    match action {
        Action::Answer(Answer::Joined(joined)) => emit(identity, &joined),
        Action::Answer(Answer::Left(left)) => emit(identity, &left),
        Action::Status => emit(identity, &status()),
        taken @ (Action::Join(_) | Action::Leave) => return Some(taken),
    }
    None
}

/// Serve the idle station until a join.
async fn take_join(
    requests: &mut Receiver,
    status: impl Fn() -> Status,
) -> (RequestIdentity, Join) {
    loop {
        let (identity, request) = next(requests).await;
        if let Some(Action::Join(join)) = answer(identity, action(Phase::Idle, request), &status) {
            return (identity, join);
        }
    }
}

/// Serve the joined station until a leave; once `leaving` holds one, refuse
/// the next.
async fn take_leave(
    requests: &mut Receiver,
    leaving: &Cell<Option<RequestIdentity>>,
    status: impl Fn() -> Status,
) -> RequestIdentity {
    loop {
        let phase = match leaving.get() {
            None => Phase::Joined,
            Some(_) => Phase::Leaving,
        };
        let (identity, request) = next(requests).await;
        if let Some(Action::Leave) = answer(identity, action(phase, request), &status) {
            return identity;
        }
    }
}

/// Serve the running station; a leave stops it gracefully.
async fn serve_running(
    requests: &mut Receiver,
    control: &mut wifi::Control<'static>,
    leaving: &Cell<Option<RequestIdentity>>,
    status: impl Fn() -> Status,
) -> Infallible {
    loop {
        let leave = take_leave(requests, leaving, &status).await;
        leaving.set(Some(leave));
        control.stop();
    }
}

/// Serve the stopped station until the chip resets.
async fn stopped(requests: &mut Receiver, status: impl Fn() -> Status) -> ! {
    loop {
        let (identity, request) = next(requests).await;
        // A stopped station takes neither a join nor a leave.
        let _taken = answer(identity, action(Phase::Stopped, request), &status);
    }
}

/// The IPv4 configuration DHCP leased.
fn ipv4(status: oer_embassy_net::ip::Ipv4Status) -> Option<Ipv4> {
    use oer_embassy_net::ip::Ipv4Status;
    match status {
        Ipv4Status::Lease(config) | Ipv4Status::Static(config) => Some(Ipv4 {
            address: config.address,
            prefix: config.prefix_length,
            gateway: config.gateway,
        }),
        Ipv4Status::Unconfigured => None,
    }
}

/// The station ended before its join linked or its leave stopped it.
fn ended(exit: &wifi::StationExit) -> StationError {
    use wifi::{StationExit, UnavailableCause};
    let (kind, detail) = match exit {
        StationExit::Stopped => (0, 0),
        StationExit::Failed(_) => (1, 0),
        StationExit::Poisoned(_) => (2, 0),
        StationExit::Quarantined(_) => (3, 0),
        StationExit::Unavailable(UnavailableCause::Abandoned) => (4, 0),
        StationExit::Unavailable(UnavailableCause::Initialization(error)) => {
            (5, initialization(error))
        }
    };
    StationError::failed(class::STATION_ENDED, kind, detail)
}

/// The station's construction refused the join's profile.
fn initialization_failed(error: &wifi::InitializationError) -> StationError {
    StationError::failed(class::STATION_ENDED, 5, initialization(error))
}

/// The position of `error` in [`wifi::InitializationError`].
fn initialization(error: &wifi::InitializationError) -> u16 {
    use wifi::InitializationError as E;
    match error {
        E::Phy(_) => 0,
        E::Country => 1,
        E::Domain => 2,
        E::Channel => 3,
        E::Address => 4,
        E::Ssid => 5,
        E::Arbiter => 6,
        E::Storage => 7,
        E::Control => 8,
        E::Frontend => 9,
        E::MembershipUnavailable => 10,
        E::Request(_) => 11,
        E::Radio => 12,
        E::Backend => 13,
        E::Registration => 14,
        E::Rx => 15,
        E::Tx => 16,
        E::Dma(_) => 17,
    }
}

fn credentials_failed(error: wifi::CredentialsError) -> StationError {
    let kind = match error {
        wifi::CredentialsError::SsidLength => 0,
        wifi::CredentialsError::PassphraseLength => 1,
    };
    StationError::failed(class::CREDENTIALS, kind, 0)
}

fn network_failed(error: oer_embassy_net::ip::Error) -> StationError {
    use oer_embassy_net::ip::Error as E;
    let kind = match error {
        E::InvalidPrefixLength(_) => 0,
        E::EndpointAlreadySplit => 1,
        E::RxWaiterUnavailable => 2,
        E::InterfaceTableFull => 3,
        E::UnsupportedMedium => 4,
        E::HardwareAddressMismatch => 5,
        E::AddressNotUnicast => 6,
        E::AddressTableFull => 7,
        E::GatewayNotUnicast => 8,
        E::RouteTableFull => 9,
        E::DhcpMediumMismatch => 10,
        E::ContextAccessRefused => 11,
    };
    StationError::failed(class::NETWORK, kind, 0)
}

// Keep the bounded wire-record constructor out of the station's poll frames.
#[inline(never)]
pub(crate) fn emit<M: oer_hil_protocol::Message>(identity: RequestIdentity, body: &M) {
    let _sequence = CONSOLE.publish(identity.session_id, identity.request_id, body);
}
