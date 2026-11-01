// The SDK station image: the boot of the library's station example with
// the console added and every channel scanned, the HIL agent serving the
// base module and the SDK station service, and one worker that answers the
// service's requests in order.

use core::{convert::Infallible, future::Future};

use oer_espressif::{
    Aes, AppResult, Boot, Console, Error, Peripherals, Started,
    time::{Duration, with_timeout},
    wifi::{
        self, ConnectError, Ipv4Configuration, Ipv4Status, LinkState, Net, NetConfig, Station,
        embassy_net,
    },
};
use oer_hil_agent_espressif::{Inbox, Mailbox, answer};
use oer_services_sdk_station::{
    Address, Connect, Connection, Exchange, Exchanged, Ipv4, LinkLoss, Request, Status, WaitIpv4,
    WaitLinkDown,
};
use oer_services_wifi_station::{StationError, runtime as codes};
use static_cell::ConstStaticCell;

/// The regulatory domain the station installs, every 2.4 GHz channel it
/// permits scanned at each connect.
const COUNTRY: [u8; 2] = *b"DE";
/// The owner's transmit power cap, below the domain's ceiling.
const TX_CAP_DBM: i8 = 10;

static MAILBOX: ConstStaticCell<Mailbox<Request>> = ConstStaticCell::new(Mailbox::new());

oer_espressif::main!(boot);

fn boot(p: Peripherals, mut boot: Boot) -> Result<Started<impl Future<Output = AppResult>>, Error> {
    let aes = Aes::start(p.aes, &mut boot)?;
    let config = wifi::Config::all_channels(COUNTRY, TX_CAP_DBM)?;
    let (station, device) = Station::start(p.wifi, &aes, config, &mut boot)?;
    let net = Net::start(device, NetConfig::Dhcpv4, &mut boot)?;
    let console = Console::open(p.console, &mut boot)?;
    let evidence = boot.evidence();
    let (outbox, inbox) = MAILBOX.try_take().ok_or(Error::Storage)?.split();
    let worker = Worker {
        station,
        net,
        requests: inbox,
    };
    Ok(boot.run(oer_hil_agent_espressif::serve(
        console,
        evidence,
        services::IMAGE,
        outbox,
        worker.run(),
    )))
}

/// Answers the service's requests one at a time.
struct Worker {
    station: Station,
    net: Net,
    requests: Inbox<'static, Request>,
}

impl Worker {
    async fn run(mut self) -> Infallible {
        loop {
            let (identity, request) = self.requests.next().await;
            match request {
                Request::Connect(connect) => answer(identity, &self.connect(connect).await),
                Request::WaitIpv4(WaitIpv4 { timeout_ms }) => {
                    answer(identity, &self.wait_ipv4(timeout_ms).await);
                }
                Request::Exchange(exchange) => answer(identity, &self.exchange(exchange).await),
                Request::WaitLinkDown(WaitLinkDown { timeout_ms }) => {
                    answer(identity, &self.wait_link_down(timeout_ms).await);
                }
                Request::GetStatus(_) => answer(identity, &self.status()),
            }
        }
    }

    async fn connect(&mut self, connect: Connect) -> Connection {
        let Ok(profile) =
            wifi::Profile::wpa2(connect.ssid.as_bytes(), connect.passphrase.as_bytes())
        else {
            return Connection::InvalidProfile;
        };
        let failed = |error: StationError| match error {
            StationError::Failed { code } => Connection::Failed { code },
            _ => Connection::Stopped,
        };
        match self.station.connect(profile).await {
            Ok(linked) => Connection::Linked {
                bssid: linked.bssid,
                channel: linked.channel,
            },
            Err(ConnectError::Failed(reason)) => failed(codes::connect_failed(&reason)),
            Err(ConnectError::Refused(refusal)) => failed(codes::refused(refusal)),
            Err(ConnectError::Busy) => {
                failed(StationError::failed(codes::class::NOT_ADMITTED, 0, 0))
            }
            Err(ConnectError::Profile(_)) => Connection::InvalidProfile,
            Err(ConnectError::Stopped) => Connection::Stopped,
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

    async fn exchange(&self, exchange: Exchange) -> Exchanged {
        if leased(self.net.status().ipv4).is_none() {
            return Exchanged::NoAddress;
        }
        match with_timeout(
            milliseconds(exchange.timeout_ms),
            echo(&self.net, &exchange),
        )
        .await
        {
            Ok(exchanged) => exchanged,
            Err(_) => Exchanged::TimedOut,
        }
    }

    fn status(&self) -> Status {
        let net = self.net.status();
        Status {
            address: self.station.address(),
            link_up: self.station.link().state == LinkState::Up,
            ipv4: leased(net.ipv4).map(ipv4),
        }
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
async fn echo(net: &Net, exchange: &Exchange) -> Exchanged {
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
