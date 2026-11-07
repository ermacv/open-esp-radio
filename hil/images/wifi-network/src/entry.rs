// The network core's station image: the boot of `hil-wifi-station` with the
// network core in the IP service's place, the HIL agent serving the base
// module, the Wi-Fi station and load services, and one worker that answers
// their requests in order beside the load server's relay. The group runs the
// load server on the network's sockets; the first run's start gives the
// network its address.

use core::pin::pin;

use embassy_futures::select::{Either as Raced, select};
use oer::{
    AppResult, ApplicationEnd, Boot, Channel, Console, Error, GroupBoot, GroupPeripherals,
    GroupStarted, Peripherals, Started, observe,
    wifi::{self, ConnectError, Station, StopError},
};
use oer_protocols_hil_agent::base::Either;
use oer_hil_agent::{Inbox, Mailbox, answer};
use oer_protocols_hil_protocol::RequestIdentity;
use oer_hil_services_wifi_station::{
    self as wifi_station, Joined, Left, StationError,
    phase::{Action, Answer, Phase},
    runtime as codes,
};
use static_cell::ConstStaticCell;

/// The regulatory domain the station installs, every 2.4 GHz channel it
/// permits scanned at each connect.
const COUNTRY: [u8; 2] = *b"DE";
/// The transmit power cap: above the domain's ceiling, which decides.
const TX_CAP_DBM: i8 = 20;

/// A request of the Wi-Fi station service or of the load service.
type Requests = Either<wifi_station::Request, oer_hil_services_load::Request>;

static MAILBOX: ConstStaticCell<Mailbox<Requests>> = ConstStaticCell::new(Mailbox::new());

oer::entry!(boot, group = network);

fn boot(p: Peripherals, mut boot: Boot) -> Result<impl Started, Error> {
    // The load scenario measures the station at HT 40 MHz where the stand's
    // access point offers it.
    let config = wifi::Config::all_channels(COUNTRY, TX_CAP_DBM)?.with_ht40();
    let (station, _no_ip_service) = Station::start(p.wifi, p.aes, config, &mut boot)?;
    let console = Console::open(p.console, &mut boot)?;
    let load = load::serve::LOAD.application(&mut boot)?;
    let relay = load::relay::RELAY.application(&mut boot)?;
    let hart0 = boot.observe()?;
    let relay_part = observe::Part::claim("relay")?;
    let evidence = boot.evidence();
    let (outbox, inbox) = MAILBOX.try_take().ok_or(Error::Storage)?.split();
    let worker = Worker {
        station,
        load,
        phase: Phase::Idle,
        requests: inbox,
    };
    Ok(boot.run(move || {
        oer_hil_agent::serve(
            console,
            evidence,
            services::IMAGE,
            outbox,
            move || application(worker, relay, hart0, relay_part),
        )
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

/// The group: the network over the station's device and the load server on it.
fn network(p: GroupPeripherals, mut boot: GroupBoot) -> Result<impl GroupStarted, Error> {
    let device = p.device.ok_or(Error::NoDevice)?;
    let hart1 = boot.observe().ok_or(Error::ObserveTaken)?;
    let (net, mut boot) = wifi::Network::start(device, boot)?;
    let load = load::serve::LOAD.group(&mut boot)?;
    let relay = load::relay::RELAY.group(&mut boot)?;
    let tally = load::serve::TALLY.try_take().ok_or(Error::Storage)?;
    let part = observe::Part::claim("load")?;
    Ok(boot.run(move || sockets(net, load, relay, hart1, tally, part)))
}

/// The load server, which never ends.
async fn sockets(
    net: wifi::Network,
    load: load::serve::Requests,
    relay: load::relay::Client,
    hart1: observe::Hart1,
    tally: load::serve::RunTally,
    part: observe::Part,
) -> AppResult {
    let net = udp::Network::new(net);
    let mut server = pin!(load::serve::serve(&net, load, relay, hart1, tally));
    match part.polled(server.as_mut()).await {}
}

/// Answers the services' requests one at a time.
struct Worker {
    station: Station,
    /// The load server's requests and answers.
    load: ApplicationEnd<(RequestIdentity, oer_hil_services_load::Request), load::serve::Answer, 1>,
    /// Where the Wi-Fi station service's one join per boot is.
    phase: Phase,
    requests: Inbox<'static, Requests>,
}

impl Worker {
    async fn run(mut self) -> AppResult {
        loop {
            let (identity, request) = self.requests.next().await;
            match request {
                Either::First(request) => self.station_request(identity, request).await,
                // The load server in the group answers it.
                Either::Second(request) => {
                    self.load.send((identity, request)).await;
                    self.load.receive().await.publish(identity);
                }
            }
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
        let profile = wifi::Profile::wpa2(join.ssid.as_bytes(), join.passphrase.as_bytes());
        let (Ok(profile), Ok(_)) = (profile, wifi::Config::new(COUNTRY, join.channel, TX_CAP_DBM))
        else {
            return Joined::Refused(StationError::InvalidSettings);
        };
        self.phase = Phase::Joined;
        match self.station.connect(profile).await {
            Ok(linked) => Joined::Linked {
                bssid: linked.bssid,
                channel: linked.channel,
            },
            Err(error) => {
                if matches!(error, ConnectError::Failed(_)) {
                    self.phase = Phase::Idle;
                }
                Joined::Refused(station_error(error))
            }
        }
    }

    /// Stop the station: the IP service returns its receive buffers, then
    /// the radio retires.
    async fn leave(&mut self) -> Left {
        self.phase = Phase::Leaving;
        let left = match self.station.stop().await {
            Ok(()) => Left::Down,
            Err(StopError::Failed) => Left::Refused(StationError::Unavailable),
        };
        self.phase = Phase::Stopped;
        left
    }

    /// The address is the host's: the station's status names none.
    async fn station_status(&self) -> wifi_station::Status {
        wifi_station::Status {
            address: self.station.address(),
            link: self.station.linked().map(|linked| wifi_station::Link {
                bssid: linked.bssid,
                channel: linked.channel,
            }),
            ipv4: None,
        }
    }
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
