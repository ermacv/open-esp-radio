// The station image: the boot of the library's station example with the
// console added, every channel scanned and the network core in the IP
// service's place, the HIL agent serving the base module, the SDK station,
// Wi-Fi station, fault and load services, and one worker that answers their
// requests in order beside the load server's relay. The group runs the
// network, its address and link waits and the load server on its sockets,
// bound at its start; the network leases its address by DHCP.

use core::pin::pin;

use embassy_futures::select::{Either as Raced, Either3, select, select3};
use oer::{
    AppResult, ApplicationEnd, Boot, Channel, Console, Error, GroupBoot, GroupEnd,
    GroupPeripherals, GroupStarted, Peripherals, Started, observe,
    time::{Duration, with_timeout},
    wifi::{self, ConnectError, LinkState, NetworkError, Station, StopError, WatchRefused},
};
use oer_hil_agent::{Inbox, Mailbox, answer, answer_then_trap};
use oer_hil_services_sdk_station::{
    Address, Connect, Connection, Exchanged, LinkLoss, Request, Status, WaitIpv4,
    WaitLinkDown,
};
use oer_hil_services_wifi_station::{
    self as wifi_station, Joined, Left, StationError,
    phase::{Action, Answer, Phase},
    runtime as codes,
};
use oer_protocols_hil_agent::base::Either;
use oer_protocols_hil_protocol::RequestIdentity;
use static_cell::ConstStaticCell;

/// The regulatory domain the station installs, every 2.4 GHz channel it
/// permits scanned at each connect.
const COUNTRY: [u8; 2] = *b"DE";
/// The transmit power cap: above the domain's ceiling, which decides.
const TX_CAP_DBM: i8 = 20;

/// A request of the SDK station service, of the Wi-Fi station service, of
/// the fault service or of the load service.
type Requests = Either<
    Either<Either<Request, wifi_station::Request>, oer_hil_services_fault::Request>,
    oer_hil_services_load::Request,
>;

static MAILBOX: ConstStaticCell<Mailbox<Requests>> = ConstStaticCell::new(Mailbox::new());
/// The network's service, with the load's producer in it.
static NETWORK: ConstStaticCell<wifi::NetworkStorage<udp::LoadProducer, wifi::Queued>> =
    ConstStaticCell::new(wifi::NetworkStorage::new());
/// What the load's sender hands the producer.
static SOURCE: udp::Source = udp::Source::new();
/// The worker asks the group for the network's address, which only the
/// group's hart reads.
static ADDRESSES: Channel<(), Option<wifi_station::Ipv4>, 1> = Channel::new();
/// The worker hands the group each wait for an address, with its timeout.
static IPV4_WAITS: Channel<u32, Address, 1> = Channel::new();
/// The worker hands the group each wait for the link's loss, with its
/// timeout.
static LINK_WAITS: Channel<u32, LinkLoss, 1> = Channel::new();

oer::entry!(boot, group = network);

fn boot(p: Peripherals, mut boot: Boot) -> Result<impl Started, Error> {
    // The load scenario measures the station at HT 40 MHz where the stand's
    // access point offers it.
    let config = wifi::Config::all_channels(COUNTRY, TX_CAP_DBM)?.with_ht40();
    let (station, _no_ip_service) = Station::start(p.wifi, p.aes, config, &mut boot)?;
    let console = Console::open(p.console, &mut boot)?;
    let load = load::serve::LOAD.application(&mut boot)?;
    let relay = load::relay::RELAY.application(&mut boot)?;
    let asks = Asks {
        addresses: ADDRESSES.application(&mut boot)?,
        ipv4: IPV4_WAITS.application(&mut boot)?,
        links: LINK_WAITS.application(&mut boot)?,
    };
    let hart0 = boot.observe()?;
    let relay_part = observe::Part::claim("relay")?;
    let evidence = boot.evidence();
    let (outbox, inbox) = MAILBOX.try_take().ok_or(Error::Storage)?.split();
    let worker = Worker {
        station,
        load,
        asks,
        phase: Phase::Idle,
        requests: inbox,
    };
    Ok(boot.run(move || {
        oer_hil_agent::serve(console, evidence, services::IMAGE, outbox, move || {
            application(worker, relay, hart0, relay_part)
        })
    }))
}

/// The worker beside the load server's relay, which never ends.
async fn application(
    worker: Worker,
    relay: load::relay::Relay,
    hart0: observe::Hart0,
    part: observe::Part,
) -> AppResult {
    let mut relay = pin!(load::relay::serve(relay, hart0));
    match select(worker.run(), part.polled(relay.as_mut())).await {
        Raced::First(result) => result,
        Raced::Second(never) => match never {},
    }
}

/// The group: the network over the station's device, its answers to the
/// worker and the load server on it.
fn network(p: GroupPeripherals, mut boot: GroupBoot) -> Result<impl GroupStarted, Error> {
    let device = p.device.ok_or(Error::NoDevice)?;
    let hart1 = boot.observe().ok_or(Error::ObserveTaken)?;
    let storage = NETWORK.try_take().ok_or(Error::Storage)?;
    let producer = udp::LoadProducer::new(&SOURCE);
    let (net, socket, mut boot) =
        wifi::Network::start(device, boot, storage, producer, udp::PORTS)?;
    let load = load::serve::LOAD.group(&mut boot)?;
    let relay = load::relay::RELAY.group(&mut boot)?;
    let asks = Asked {
        addresses: ADDRESSES.group(&mut boot)?,
        ipv4: IPV4_WAITS.group(&mut boot)?,
        links: LINK_WAITS.group(&mut boot)?,
    };
    let tally = load::serve::TALLY.try_take().ok_or(Error::Storage)?;
    let turns = udp::TURNS.try_take().ok_or(Error::Storage)?;
    let part = observe::Part::claim("load")?;
    let net = udp::Network::new(net, socket, &SOURCE);
    Ok(boot.run(move || sockets(net, load, relay, asks, hart1, tally, turns, part)))
}

/// The load server beside the answers to the worker, which never end but
/// for a fault.
#[allow(
    clippy::too_many_arguments,
    reason = "the group's owners, each moved in once"
)]
async fn sockets(
    net: udp::Network,
    load: load::serve::Requests,
    relay: load::relay::Client,
    asks: Asked,
    hart1: observe::Hart1,
    tally: load::serve::RunTally,
    turns: &'static mut udp::Turns,
    part: observe::Part,
) -> AppResult {
    net.keep_turns(turns);
    let mut server = pin!(load::serve::serve(&net, load, relay, hart1, tally));
    match select(part.polled(server.as_mut()), answer_asks(&net, asks)).await {
        Raced::First(never) => match never {},
        Raced::Second(fault) => fault,
    }
}

/// The worker's ends of the group's answers.
struct Asks {
    addresses: ApplicationEnd<(), Option<wifi_station::Ipv4>, 1>,
    ipv4: ApplicationEnd<u32, Address, 1>,
    links: ApplicationEnd<u32, LinkLoss, 1>,
}

/// The group's ends of them.
struct Asked {
    addresses: GroupEnd<(), Option<wifi_station::Ipv4>, 1>,
    ipv4: GroupEnd<u32, Address, 1>,
    links: GroupEnd<u32, LinkLoss, 1>,
}

/// Answer the worker's asks one at a time, so one wait at most reads the
/// network's publications. A refused read is an impossible state here: the
/// service publishes within its own turn, on this hart, so no read meets
/// one; it ends the group, which records it.
async fn answer_asks(net: &udp::Network, mut asks: Asked) -> AppResult {
    let published = udp::published(net);
    loop {
        match select3(
            asks.addresses.receive(),
            asks.ipv4.receive(),
            asks.links.receive(),
        )
        .await
        {
            Either3::First(()) => {
                let ipv4 = udp::address(net).map_err(watch_fault)?;
                asks.addresses.send(ipv4.map(station_ipv4)).await;
            }
            Either3::Second(timeout_ms) => {
                let address = address(published, timeout_ms).await?;
                asks.ipv4.send(address).await;
            }
            Either3::Third(timeout_ms) => {
                let loss = link_loss(published, timeout_ms).await?;
                asks.links.send(loss).await;
            }
        }
    }
}

/// Wait for an address over an up link ([`waits::leased`]); across a link
/// loss the wait goes on to the next one.
async fn address(published: &impl waits::Published, timeout_ms: u32) -> Result<Address, Error> {
    let leased = waits::until(published, waits::leased);
    match with_timeout(milliseconds(timeout_ms), leased).await {
        Ok(Ok(Some(ipv4))) => Ok(Address::Leased(sdk_ipv4(ipv4))),
        Ok(Ok(None)) => Ok(Address::Stopped),
        Ok(Err(refused)) => Err(watch_fault(refused)),
        Err(_) => Ok(Address::TimedOut),
    }
}

/// Wait until the link up now is lost, as the network last saw it.
async fn link_loss(published: &impl waits::Published, timeout_ms: u32) -> Result<LinkLoss, Error> {
    let Some(start) = waits::Published::configuration(published)
        .map_err(watch_fault)?
        .map(|configuration| configuration.link)
    else {
        return Ok(LinkLoss::Stopped);
    };
    let lost = waits::until(published, |configuration| {
        waits::lost(start, configuration.link).then_some(())
    });
    match with_timeout(milliseconds(timeout_ms), lost).await {
        Ok(Ok(Some(()))) => Ok(LinkLoss::Down),
        Ok(Ok(None)) => Ok(LinkLoss::Stopped),
        Ok(Err(refused)) => Err(watch_fault(refused)),
        Err(_) => Ok(LinkLoss::TimedOut),
    }
}

fn watch_fault(refused: WatchRefused) -> Error {
    Error::Network(NetworkError::Watch(refused))
}

/// Answers the services' requests one at a time.
struct Worker {
    station: Station,
    /// The load server's requests and answers.
    load: ApplicationEnd<(RequestIdentity, oer_hil_services_load::Request), load::serve::Answer, 1>,
    /// The group's answers of the network's address and waits.
    asks: Asks,
    /// Where the Wi-Fi station service's one join per boot is.
    phase: Phase,
    requests: Inbox<'static, Requests>,
}

impl Worker {
    async fn run(mut self) -> AppResult {
        loop {
            let (identity, request) = self.requests.next().await;
            match request {
                Either::First(Either::First(Either::First(request))) => {
                    self.sdk_request(identity, request).await;
                }
                Either::First(Either::First(Either::Second(request))) => {
                    self.station_request(identity, request).await;
                }
                // Traps once the answer is written; a write the console
                // cannot confirm ends the application, which records it.
                Either::First(Either::Second(oer_hil_services_fault::Request::RaiseException(
                    _,
                ))) => match answer_then_trap(identity, &oer_hil_services_fault::Raising).await? {},
                Either::First(Either::Second(
                    oer_hil_services_fault::Request::RefusePublication(_),
                )) => {
                    oer::wifi::debug::refuse_next_publication();
                    answer(identity, &oer_hil_services_fault::Refusing);
                }
                Either::First(Either::Second(oer_hil_services_fault::Request::GetRestarts(_))) => {
                    answer(identity, &self.restarts());
                }
                // The load server in the group answers it.
                Either::Second(request) => {
                    self.load.send((identity, request)).await;
                    self.load.receive().await.publish(identity);
                }
            }
        }
    }

    /// An SDK station service request.
    async fn sdk_request(&mut self, identity: RequestIdentity, request: Request) {
        match request {
            Request::Connect(connect) => answer(identity, &self.connect(connect).await),
            Request::WaitIpv4(WaitIpv4 { timeout_ms }) => {
                self.asks.ipv4.send(timeout_ms).await;
                answer(identity, &self.asks.ipv4.receive().await);
            }
            // The network core has no TCP: the exchange opens no connection.
            Request::Exchange(_) => answer(identity, &Exchanged::NoTcp),
            Request::WaitLinkDown(WaitLinkDown { timeout_ms }) => {
                self.asks.links.send(timeout_ms).await;
                answer(identity, &self.asks.links.receive().await);
            }
            Request::GetStatus(_) => answer(identity, &self.status().await),
        }
    }

    async fn connect(&mut self, connect: Connect) -> Connection {
        let Ok(profile) =
            wifi::Profile::wpa2(connect.ssid.as_bytes(), connect.passphrase.as_bytes())
        else {
            return Connection::InvalidProfile;
        };
        match self.station.connect(profile).await {
            Ok(linked) => Connection::Linked {
                bssid: linked.bssid,
                channel: linked.channel,
            },
            Err(ConnectError::Profile(_)) => Connection::InvalidProfile,
            Err(error) => match station_error(error) {
                StationError::Failed { code } => Connection::Failed { code },
                _ => Connection::Stopped,
            },
        }
    }

    /// The station's restarts without a reset and the last one's cause.
    fn restarts(&self) -> oer_hil_services_fault::Restarts {
        let restarts = self.station.restarts();
        let mut cause = [0; oer_hil_services_fault::MAX_CAUSE];
        let length = oer::wifi::restart_cause_text(&restarts, &mut cause);
        oer_hil_services_fault::Restarts {
            count: restarts.count,
            cause: cause
                .get(..length)
                .and_then(|text| core::str::from_utf8(text).ok())
                .and_then(|text| heapless::String::try_from(text).ok())
                .unwrap_or_default(),
        }
    }

    /// A Wi-Fi station service request: one join per boot, a leave of the
    /// joined station once ([`wifi_station::phase`]).
    async fn station_request(&mut self, identity: RequestIdentity, request: wifi_station::Request) {
        match wifi_station::phase::action(self.phase, request) {
            Action::Answer(Answer::Joined(joined)) => answer(identity, &joined),
            Action::Answer(Answer::Left(left)) => answer(identity, &left),
            Action::Status => answer(identity, &self.station_status().await),
            Action::Join(join) => {
                let joined = self.join(join).await;
                answer(identity, &joined);
            }
            Action::Leave => {
                let left = self.leave().await;
                answer(identity, &left);
            }
            Action::ConnectTimes => answer(identity, &connect_times()),
            Action::ScanVisit(index) => answer(identity, &scan_visit(index)),
        }
    }

    /// Connect to the join's network. The station scans every channel the
    /// domain permits; the join's channel must be one of them, and the
    /// host checks that the link came up on it. Settings the station
    /// refuses leave the image idle for another join.
    async fn join(&mut self, join: wifi_station::Join) -> Joined {
        let profile = wifi::Profile::wpa2(join.ssid.as_bytes(), join.passphrase.as_bytes())
            .map(|profile| {
                if join.he20 {
                    profile.with_he20()
                } else {
                    profile
                }
            });
        let (Ok(profile), Ok(_)) = (
            profile,
            wifi::Config::new(COUNTRY, join.channel, TX_CAP_DBM),
        ) else {
            return Joined::Refused(StationError::InvalidSettings);
        };
        self.phase = Phase::Joined;
        match self.station.connect(profile).await {
            Ok(linked) => Joined::Linked {
                bssid: linked.bssid,
                channel: linked.channel,
                phy: wifi_station::runtime::phy(linked.phy),
            },
            Err(error) => {
                if matches!(error, ConnectError::Failed(_)) {
                    self.phase = Phase::Idle;
                }
                Joined::Refused(station_error(error))
            }
        }
    }

    /// Stop the station: the network returns its receive buffers, then the
    /// radio retires.
    async fn leave(&mut self) -> Left {
        self.phase = Phase::Leaving;
        let left = match self.station.stop().await {
            Ok(()) => Left::Down,
            Err(StopError::Failed) => Left::Refused(StationError::Unavailable),
            // The radio's stop was confirmed; the network's was not.
            Err(StopError::NetworkLost) => Left::DownNetworkLost,
        };
        self.phase = Phase::Stopped;
        left
    }

    /// The address the network holds now, which the group reads.
    async fn ipv4(&mut self) -> Option<wifi_station::Ipv4> {
        self.asks.addresses.send(()).await;
        self.asks.addresses.receive().await
    }

    /// The station and the address its network leased.
    async fn station_status(&mut self) -> wifi_station::Status {
        let ipv4 = self.ipv4().await;
        wifi_station::Status {
            address: self.station.address(),
            link: self.station.linked().map(|linked| wifi_station::Link {
                bssid: linked.bssid,
                channel: linked.channel,
                phy: wifi_station::runtime::phy(linked.phy),
            }),
            ipv4,
        }
    }

    /// The SDK station service's status of the same.
    async fn status(&mut self) -> Status {
        let ipv4 = self.ipv4().await;
        Status {
            address: self.station.address(),
            link_up: self.station.link().state == LinkState::Up,
            ipv4: ipv4.map(|ipv4| oer_hil_services_sdk_station::Ipv4 {
                address: ipv4.address,
                prefix: ipv4.prefix,
                gateway: ipv4.gateway,
            }),
        }
    }
}

fn station_ipv4(ipv4: wifi::Ipv4) -> wifi_station::Ipv4 {
    wifi_station::Ipv4 {
        address: ipv4.address(),
        prefix: ipv4.prefix(),
        gateway: ipv4.gateway(),
    }
}

fn sdk_ipv4(ipv4: wifi::Ipv4) -> oer_hil_services_sdk_station::Ipv4 {
    oer_hil_services_sdk_station::Ipv4 {
        address: ipv4.address(),
        prefix: ipv4.prefix(),
        gateway: ipv4.gateway(),
    }
}

fn milliseconds(timeout_ms: u32) -> Duration {
    Duration::from_millis(u64::from(timeout_ms))
}

/// The last connect as the service reports it.
fn connect_times() -> wifi_station::ConnectTimes {
    let connect = observe::connect();
    let stages = connect.stages;
    wifi_station::ConnectTimes {
        visits: connect.visits,
        lost_visits: connect.lost_visits,
        ended: connect.ended,
        ask: stages.ask,
        tune: stages.tune,
        configure: stages.configure,
        authenticate: stages.authenticate,
        associate: stages.associate,
        start_link: stages.start_link,
        handshake: stages.handshake,
        connected: stages.connected,
    }
}

/// Visit `index` of the last connect's scan as the service reports it.
fn scan_visit(index: u8) -> wifi_station::ScanVisitAnswer {
    wifi_station::ScanVisitAnswer(observe::connect_visit(usize::from(index)).map(|visit| {
        wifi_station::ScanVisit {
            channel: visit.channel,
            asked: visit.asked,
            granted: visit.granted,
            tuned: visit.tuned,
            configured: visit.configured,
            listening: visit.listening,
            ended: visit.ended,
            first_heard: visit.first_heard,
            first_own: visit.first_own,
            heard: visit.heard,
            own_beacons: visit.own_beacons,
            own_probe_responses: visit.own_probe_responses,
            responses_to_us: visit.responses_to_us,
            responses_overheard: visit.responses_overheard,
            probe_ended: visit.probe_ended,
            probe: match visit.probe {
                observe::connect::ProbeState::None => wifi_station::ProbeState::None,
                observe::connect::ProbeState::Built => wifi_station::ProbeState::Built,
                observe::connect::ProbeState::Unsent => wifi_station::ProbeState::Unsent,
                observe::connect::ProbeState::Sent => wifi_station::ProbeState::Sent,
                observe::connect::ProbeState::Answered => wifi_station::ProbeState::Answered,
                observe::connect::ProbeState::TimedOut => wifi_station::ProbeState::TimedOut,
                observe::connect::ProbeState::Unknown => wifi_station::ProbeState::Unknown,
            },
            best_own_rssi: visit.best_own_rssi,
        }
    }))
}

/// The Wi-Fi station service's error of a connect the station did not
/// complete, numbered as its runtime numbers them.
fn station_error(error: ConnectError) -> StationError {
    match error {
        ConnectError::Failed(reason) => codes::connect_failed(&reason),
        ConnectError::Refused(refusal) => codes::refused(refusal),
        ConnectError::Busy => StationError::failed(codes::class::NOT_ADMITTED, 0, 0),
        ConnectError::Profile(_) => StationError::InvalidSettings,
        ConnectError::Stopped => StationError::Unavailable,
    }
}
