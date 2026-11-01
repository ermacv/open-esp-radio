// The SDK station image: the library's station and IP service, the HIL
// console serving the base module and the SDK station service, and one
// worker that answers the service's requests in order.

use core::{convert::Infallible, future::Future};

use oer_espressif::{
    Aes, AppResult, Boot, Console, Error, Evidence, Peripherals, ResetCause, Started, UsbConsole,
    time::{Duration, with_timeout},
    wifi::{
        self, ConnectError, Ipv4Configuration, Ipv4Status, LinkState, Net, NetConfig, Station,
        embassy_net,
    },
};
use oer_hil_protocol::RequestIdentity;
use oer_memory::{WakingSpscQueue, WakingSpscReceiver, WakingSpscSender};
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

oer_espressif::main!(boot);

fn boot(p: Peripherals, mut boot: Boot) -> Result<Started<impl Future<Output = AppResult>>, Error> {
    let aes = Aes::start(p.aes, &mut boot)?;
    let config = wifi::Config::all_channels(COUNTRY, TX_CAP_DBM)?;
    let (station, device) = Station::start(p.wifi, &aes, config, &mut boot)?;
    let net = Net::start(device, NetConfig::Dhcpv4, &mut boot)?;
    let console = Console::open(p.console, &mut boot)?;
    let evidence = Platform(boot.evidence());
    let (sender, receiver) = REQUESTS.try_take().ok_or(Error::Storage)?.split();
    Ok(boot.run(app(
        console,
        evidence,
        sender,
        Worker {
            station,
            net,
            requests: receiver,
        },
    )))
}

/// Serve the host and the worker side by side until the chip resets.
async fn app(
    console: UsbConsole,
    evidence: Platform,
    requests: Sender,
    worker: Worker,
) -> AppResult {
    let boot = oer_espressif::boot_id();
    CONSOLE.start(boot);
    let intake = oer_hil_agent::base::Intake::new(
        boot,
        services::KEYS,
        services::MAXIMUM_PAYLOAD_BYTES,
        oer_espressif_staged_runtime::layout::payload_crc32(),
    );
    let serve = CONSOLE.run(
        console,
        intake,
        &evidence,
        move |identity, request: Request| {
            queue(&requests, identity, request);
        },
    );
    match embassy_futures::select::select(serve, worker.run()).await {
        // The console's transport broke its contract.
        embassy_futures::select::Either::First(_fault) => Err(Error::Console),
        embassy_futures::select::Either::Second(never) => match never {},
    }
}

// ---------------------------------------------------------------- console

const NOTES: usize = 8;
const MESSAGES: usize = 8;

/// Writes a note through the early USB writer, until the console opens.
struct EarlyUsb;

impl oer_hil_agent::console::Immediate for EarlyUsb {
    fn note(note: &oer_diagnostics::Note<'_>) -> oer_diagnostics::Written {
        oer_esp32s31_compose::console::early(note)
    }

    fn take_lost() -> u32 {
        oer_esp32s31_compose::console::take_early_lost()
    }
}

type HilConsole = oer_hil_agent::console::Console<NOTES, MESSAGES, EarlyUsb>;

oer_espressif_staged_runtime::placed_static! {
    static CONSOLE: HilConsole = oer_hil_agent::console::Console::new(), in critical_data "logging";
}

/// Publish `body` as the answer to `identity`, without waiting.
fn emit<M: oer_hil_protocol::Message>(identity: RequestIdentity, body: &M) {
    let _sequence = CONSOLE.publish(identity.session_id, identity.request_id, body);
}

/// The boot evidence the base module serves, as the library read it.
struct Platform(Evidence);

impl oer_hil_agent::base::Platform for Platform {
    fn boot_evidence(&self) -> oer_hil_protocol::base::BootEvidence {
        use oer_hil_protocol::base::{BootEvidence, PlatformPanic, ResetReason};
        let cause = self.0.reset_cause;
        BootEvidence {
            reset_reason: match cause {
                ResetCause::CoreSoftware => ResetReason::Software,
                ResetCause::CoreMainWatchdog1 => ResetReason::MainWatchdog1,
                ResetCause::ChipPowerOn => ResetReason::PowerOn,
                ResetCause::SystemBrownOut => ResetReason::Brownout,
                ResetCause::CoreMainWatchdog0 => ResetReason::MainWatchdog0,
                ResetCause::CoreRtcWatchdog
                | ResetCause::CpuRtcWatchdog
                | ResetCause::SystemRtcWatchdog => ResetReason::RtcWatchdog,
                ResetCause::SystemSuperWatchdog => ResetReason::SuperWatchdog,
                ResetCause::CoreUsbJtag | ResetCause::CoreUsbUart => ResetReason::UsbSerialJtag,
                ResetCause::CpuJtag => ResetReason::Jtag,
                ResetCause::CpuLockup => ResetReason::CpuLockup,
                _ => ResetReason::Other,
            },
            raw_reset_reason: cause.code(),
            post_mortem: None,
            platform_panic: self.0.previous_panic.as_ref().map(|record| PlatformPanic {
                hart: record.hart,
                in_interrupt: record.in_interrupt,
                interrupted_pc: record.interrupted_pc,
                // At most 48 bytes, the protocol's bound.
                file: record.file().try_into().unwrap_or_default(),
                line: record.line,
                column: record.column,
            }),
        }
    }

    /// The library printed and cleared the previous boot's panic before the
    /// boot.
    fn boot_evidence_written(&self) {}

    /// Every page is empty: the image keeps no post-mortem checkpoints.
    fn post_mortem_checkpoints(&self, first: u8) -> oer_hil_protocol::base::PostMortemCheckpoints {
        oer_hil_protocol::base::PostMortemCheckpoints {
            first,
            checkpoints: Default::default(),
        }
    }
}

// ---------------------------------------------------------------- requests

type Queue = WakingSpscQueue<(RequestIdentity, Request), 1>;
static REQUESTS: ConstStaticCell<Queue> = ConstStaticCell::new(WakingSpscQueue::new());
type Sender = WakingSpscSender<'static, (RequestIdentity, Request), 1>;
type Receiver = WakingSpscReceiver<'static, (RequestIdentity, Request), 1>;

/// Hand `request` to the worker, or refuse it while one waits.
fn queue(requests: &Sender, identity: RequestIdentity, request: Request) {
    if requests.try_send((identity, request)).is_err() {
        use oer_hil_protocol::base::{RejectReason, Rejected};
        emit(identity, &Rejected(RejectReason::Busy));
    }
}

/// Answers the service's requests one at a time.
struct Worker {
    station: Station,
    net: Net,
    requests: Receiver,
}

impl Worker {
    async fn run(mut self) -> Infallible {
        loop {
            let (identity, request) = self.next().await;
            match request {
                Request::Connect(connect) => emit(identity, &self.connect(connect).await),
                Request::WaitIpv4(WaitIpv4 { timeout_ms }) => {
                    emit(identity, &self.wait_ipv4(timeout_ms).await);
                }
                Request::Exchange(exchange) => emit(identity, &self.exchange(exchange).await),
                Request::WaitLinkDown(WaitLinkDown { timeout_ms }) => {
                    emit(identity, &self.wait_link_down(timeout_ms).await);
                }
                Request::GetStatus(_) => emit(identity, &self.status()),
            }
        }
    }

    async fn next(&mut self) -> (RequestIdentity, Request) {
        loop {
            self.requests.ready().await;
            if let Some(request) = self.requests.try_receive() {
                return request;
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
