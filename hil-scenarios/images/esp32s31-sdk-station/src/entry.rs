// The SDK station image: the boot of the library's station example with
// the console added and every channel scanned, the HIL agent serving the
// base module, the SDK station and Wi-Fi station services, and one worker
// that answers their requests in order. The second boot runs the IP
// service and the line exchanges on its sockets.

use oer_espressif::{
    Aes, AppResult, ApplicationEnd, Boot, Channel, Console, Error, Hart1Boot, Hart1Peripherals,
    Hart1Started, Peripherals, SecondEnd, Started,
    time::{Duration, with_timeout},
    wifi::{
        self, ConnectError, Ipv4Configuration, Ipv4Status, LinkState, Net, NetConfig, NetStatus,
        Station, StopError, embassy_net,
    },
};
use oer_hil_agent::base::Either;
use oer_hil_agent_espressif::{Inbox, Mailbox, answer, answer_then_trap};
use oer_hil_protocol::RequestIdentity;
use oer_services_sdk_station::{
    Address, Connect, Connection, Exchange, Exchanged, Ipv4, LinkLoss, Request, Status, WaitIpv4,
    WaitLinkDown,
};
use oer_services_wifi_station::{
    self as wifi_station, Joined, Left, StationError,
    phase::{Action, Answer, Phase},
    runtime as codes,
};
use static_cell::ConstStaticCell;

/// The regulatory domain the station installs, every 2.4 GHz channel it
/// permits scanned at each connect.
const COUNTRY: [u8; 2] = *b"DE";
/// The owner's transmit power cap, below the domain's ceiling.
const TX_CAP_DBM: i8 = 10;

/// A request of the SDK station service, of the Wi-Fi station service or of
/// the fault service.
type Requests = Either<Either<Request, wifi_station::Request>, oer_services_fault::Request>;

static MAILBOX: ConstStaticCell<Mailbox<Requests>> = ConstStaticCell::new(Mailbox::new());

/// The IP service's pools: receive slots, transmit credits and general
/// slots, larger than the library's defaults for the stand's TCP load.
const RX: usize = 32;
const TX: usize = 16;
static POOLS: wifi::Pools<RX, TX, 32> = wifi::Pools::new();

/// The worker hands each line exchange to the second boot's future and
/// hears how it ended.
static EXCHANGES: Channel<Exchange, Exchanged, 1> = Channel::new();

oer_espressif::entry!(boot, hart1 = network);

fn boot(p: Peripherals, mut boot: Boot) -> Result<impl Started, Error> {
    let aes = Aes::start(p.aes, &mut boot)?;
    let config = wifi::Config::all_channels(COUNTRY, TX_CAP_DBM)?;
    let (station, net) = Station::start(p.wifi, &aes, config, &mut boot)?;
    let console = Console::open(p.console, &mut boot)?;
    let exchanges = EXCHANGES.application(&mut boot)?;
    let evidence = boot.evidence();
    let (outbox, inbox) = MAILBOX.try_take().ok_or(Error::Storage)?.split();
    let worker = Worker {
        station,
        net,
        exchanges,
        phase: Phase::Idle,
        requests: inbox,
    };
    Ok(boot.run(oer_hil_agent_espressif::serve(
        console,
        evidence,
        services::IMAGE,
        outbox,
        move || worker.run(),
    )))
}

/// The second boot: the IP service with the image's pools over the
/// station's device, and the line exchanges on its sockets.
fn network(p: Hart1Peripherals, boot: Hart1Boot) -> Result<impl Hart1Started, Error> {
    let device = p.device.ok_or(Error::NoDevice)?;
    let (net, mut boot) = Net::start_with(device, &POOLS, NetConfig::Dhcpv4, boot)?;
    let requests = EXCHANGES.second(&mut boot)?;
    Ok(boot.run(exchanges(net, requests)))
}

/// Answer each line exchange the worker asks for.
async fn exchanges(net: Net<RX, TX>, mut requests: SecondEnd<Exchange, Exchanged, 1>) -> AppResult {
    loop {
        let exchange = requests.receive().await;
        let exchanged = if leased(net.status().ipv4).is_none() {
            Exchanged::NoAddress
        } else {
            with_timeout(milliseconds(exchange.timeout_ms), echo(&net, &exchange))
                .await
                .unwrap_or(Exchanged::TimedOut)
        };
        requests.send(exchanged).await;
    }
}

/// Answers the services' requests one at a time.
struct Worker {
    station: Station,
    net: NetStatus,
    exchanges: ApplicationEnd<Exchange, Exchanged, 1>,
    /// Where the Wi-Fi station service's one join per boot is.
    phase: Phase,
    requests: Inbox<'static, Requests>,
}

impl Worker {
    async fn run(mut self) -> AppResult {
        loop {
            let (identity, request) = self.requests.next().await;
            match request {
                Either::First(Either::First(Request::Connect(connect))) => {
                    answer(identity, &self.connect(connect).await);
                }
                Either::First(Either::First(Request::WaitIpv4(WaitIpv4 { timeout_ms }))) => {
                    answer(identity, &self.wait_ipv4(timeout_ms).await);
                }
                Either::First(Either::First(Request::Exchange(exchange))) => {
                    answer(identity, &self.exchange(exchange).await);
                }
                Either::First(Either::First(Request::WaitLinkDown(WaitLinkDown { timeout_ms }))) => {
                    answer(identity, &self.wait_link_down(timeout_ms).await);
                }
                Either::First(Either::First(Request::GetStatus(_))) => {
                    answer(identity, &self.status().await);
                }
                Either::First(Either::Second(request)) => self.station_request(identity, request).await,
                // Traps once the answer is written; a write the console
                // cannot confirm ends the application, which records it.
                Either::Second(oer_services_fault::Request::RaiseException(_)) => {
                    match answer_then_trap(identity, &oer_services_fault::Raising).await? {}
                }
            }
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
            Err(error) => Joined::Refused(station_error(error)),
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

    async fn station_status(&self) -> wifi_station::Status {
        wifi_station::Status {
            address: self.station.address(),
            link: self.station.linked().map(|linked| wifi_station::Link {
                bssid: linked.bssid,
                channel: linked.channel,
            }),
            ipv4: leased(self.net.status().await.ipv4).map(|configuration| wifi_station::Ipv4 {
                address: configuration.address,
                prefix: configuration.prefix_length,
                gateway: configuration.gateway,
            }),
        }
    }

    async fn wait_ipv4(&self, timeout_ms: u32) -> Address {
        match with_timeout(milliseconds(timeout_ms), self.net.wait_ipv4()).await {
            Ok(Ok(configuration)) => Address::Leased(ipv4(configuration)),
            Ok(Err(_)) => Address::Stopped,
            Err(_) => Address::TimedOut,
        }
    }

    async fn wait_link_down(&self, timeout_ms: u32) -> LinkLoss {
        match with_timeout(milliseconds(timeout_ms), self.net.wait_link_down()).await {
            Ok(Ok(())) => LinkLoss::Down,
            Ok(Err(_)) => LinkLoss::Stopped,
            Err(_) => LinkLoss::TimedOut,
        }
    }

    /// The second boot's future runs the exchange on its sockets.
    async fn exchange(&mut self, exchange: Exchange) -> Exchanged {
        self.exchanges.send(exchange).await;
        self.exchanges.receive().await
    }

    async fn status(&self) -> Status {
        let net = self.net.status().await;
        Status {
            address: self.station.address(),
            link_up: self.station.link().state == LinkState::Up,
            ipv4: leased(net.ipv4).map(ipv4),
        }
    }
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

fn milliseconds(timeout_ms: u32) -> Duration {
    Duration::from_millis(u64::from(timeout_ms))
}

fn leased(status: Ipv4Status) -> Option<Ipv4Configuration> {
    match status {
        Ipv4Status::Lease(configuration) | Ipv4Status::Static(configuration) => Some(configuration),
        Ipv4Status::Unconfigured => None,
    }
}

fn ipv4(configuration: Ipv4Configuration) -> Ipv4 {
    Ipv4 {
        address: configuration.address,
        prefix: configuration.prefix_length,
        gateway: configuration.gateway,
    }
}

/// Send the line to the peer and read as many bytes back.
async fn echo(net: &Net<RX, TX>, exchange: &Exchange) -> Exchanged {
    let mut rx = [0; 128];
    let mut tx = [0; 128];
    let Ok(mut socket) = embassy_net::tcp::TcpSocket::new(net.stack(), &mut rx, &mut tx) else {
        return Exchanged::ConnectionFailed;
    };
    let [a, b, c, d] = exchange.peer;
    if socket
        .connect((embassy_net::wire::Ipv4Addr::new(a, b, c, d), exchange.port))
        .await
        .is_err()
    {
        return Exchanged::ConnectionFailed;
    }
    let line = exchange.line.as_slice();
    let mut sent = 0;
    while let Some(rest) = line.get(sent..).filter(|rest| !rest.is_empty()) {
        match socket.write(rest).await {
            Ok(0) | Err(_) => return Exchanged::ConnectionFailed,
            Ok(count) => sent = sent.saturating_add(count),
        }
    }
    let mut echo = [0; oer_services_sdk_station::MAX_LINE];
    let Some(echo) = echo.get_mut(..line.len()) else {
        return Exchanged::Mismatch;
    };
    let mut read = 0;
    while let Some(rest) = echo.get_mut(read..).filter(|rest| !rest.is_empty()) {
        match socket.read(rest).await {
            Ok(0) | Err(_) => return Exchanged::ConnectionFailed,
            Ok(count) => read = read.saturating_add(count),
        }
    }
    socket.close();
    let _flushed = socket.flush().await;
    if echo == line {
        Exchanged::Echoed
    } else {
        Exchanged::Mismatch
    }
}
