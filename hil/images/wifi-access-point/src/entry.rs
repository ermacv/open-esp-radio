// The access-point image: the library's boot with the Wi-Fi access point
// and the console, the access point's network in the group with its given
// address, the HIL agent serving the base module and the access-point
// service, and one worker that answers its requests in order.

use oer::{
    AppResult, ApplicationEnd, Boot, Channel, Console, Error, GroupBoot,
    GroupPeripherals, GroupStarted, Peripherals, Started,
    time::{Duration, Instant, Timer},
    wifi::{self, AccessPoint, ApSettings, StartRefused},
};
use oer_hil_agent::{Inbox, Mailbox, answer};
use oer_hil_services_wifi_access_point::{
    AccessPointError, Ipv4, Request, Serve, Served, Status, Stopped,
};
use static_cell::ConstStaticCell;

/// The regulatory domain the access point installs.
const COUNTRY: [u8; 2] = *b"DE";
/// The transmit power cap: above the domain's ceiling, which decides.
const TX_CAP_DBM: i8 = 20;
/// The channel the radio holds until a serve names one.
const IDLE_CHANNEL: u8 = 1;
/// The access point's address on its own network, given, without a
/// gateway.
const ADDRESS: wifi::Addressing = wifi::Addressing::Given {
    address: [192, 168, 4, 1],
    prefix: 24,
    gateway: None,
};
/// The network's channel socket, which no case uses, and its producer's.
const PORTS: wifi::Ports = wifi::Ports {
    receiving: 40_000,
    sending: 40_001,
};
/// The PMK derivation, the radio's start and the first beacons.
const SERVE_BOUND: Duration = Duration::from_secs(20);
/// The stations' teardown and the radio's stop.
const STOP_BOUND: Duration = Duration::from_secs(10);
/// How often a bounded wait reads the status.
const POLL: Duration = Duration::from_millis(50);

static MAILBOX: ConstStaticCell<Mailbox<Request>> = ConstStaticCell::new(Mailbox::new());

/// The image's network: its brand and its storage.
enum Brand {}
type Storage = wifi::NetworkStorage<Brand, Silent, wifi::Copying>;
static NETWORK: ConstStaticCell<Storage> = ConstStaticCell::new(Storage::new());
/// The worker asks the group for the network's address, which the group's
/// future reads.
static ADDRESSES: Channel<(), Option<Ipv4>, 1> = Channel::new();

oer::entry!(boot, group = network);

fn boot(p: Peripherals, mut boot: Boot) -> Result<impl Started, Error> {
    let config = wifi::Config::new(COUNTRY, IDLE_CHANNEL, TX_CAP_DBM)?;
    let access_point = AccessPoint::start(p.wifi, config, &mut boot)?;
    let console = Console::open(p.console, &mut boot)?;
    let addresses = ADDRESSES.application(&mut boot)?;
    let evidence = boot.evidence();
    let (outbox, inbox) = MAILBOX.try_take().ok_or(Error::Storage)?.split();
    Ok(boot.run(move || {
        oer_hil_agent::serve(console, evidence, services::IMAGE, outbox, move || {
            worker(access_point, addresses, inbox)
        })
    }))
}

/// The group: the network over the access point's device, its address
/// given, and its answers to the worker.
fn network(p: GroupPeripherals, boot: GroupBoot) -> Result<impl GroupStarted, Error> {
    let device = p.access_point.ok_or(Error::NoDevice)?;
    let storage = NETWORK.try_take().ok_or(Error::Storage)?;
    let (net, _socket, mut boot) =
        wifi::Network::start(device, ADDRESS, boot, storage, Silent, PORTS)?;
    let mut asked = ADDRESSES.group(&mut boot)?;
    Ok(boot.run(move || async move {
        loop {
            let () = asked.receive().await;
            asked.send(address(&net)).await;
        }
    }))
}

/// The address the network holds now.
fn address(net: &wifi::Network<Brand, Silent, wifi::Copying>) -> Option<Ipv4> {
    let ipv4 = net.configuration().ok()??.ipv4?;
    Some(Ipv4 {
        address: ipv4.address(),
        prefix: ipv4.prefix(),
    })
}

/// Answer each request in order, each to completion.
async fn worker(
    access_point: AccessPoint,
    mut addresses: ApplicationEnd<(), Option<Ipv4>, 1>,
    mut requests: Inbox<'static, Request>,
) -> AppResult {
    let mut serving = false;
    loop {
        let (identity, request) = requests.next().await;
        match request {
            Request::Serve(request) => {
                let served = serve(&access_point, &mut serving, request).await;
                answer(identity, &served);
            }
            Request::GetStatus(_) => {
                addresses.send(()).await;
                let ipv4 = addresses.receive().await;
                answer(identity, &status(&access_point, ipv4));
            }
            Request::Stop(_) => {
                let stopped = if !serving {
                    Stopped::Refused(AccessPointError::NotServing)
                } else {
                    access_point.stop();
                    match until(|| !access_point.status().running, STOP_BOUND).await {
                        true => Stopped::Down,
                        false => Stopped::Refused(AccessPointError::Timeout),
                    }
                };
                answer(identity, &stopped);
            }
        }
    }
}

/// Serve the request's BSS once, and wait for its first beacons.
async fn serve(access_point: &AccessPoint, serving: &mut bool, request: Serve) -> Served {
    if *serving {
        return Served::Refused(AccessPointError::AlreadyServing);
    }
    let Ok(config) = wifi::Config::new(COUNTRY, request.channel, TX_CAP_DBM) else {
        return Served::Refused(AccessPointError::InvalidSettings);
    };
    let Ok(settings) = ApSettings::wpa2(
        request.ssid.as_bytes(),
        request.passphrase.as_bytes(),
        request.clients,
    ) else {
        return Served::Refused(AccessPointError::InvalidSettings);
    };
    match access_point.serve(config, settings) {
        Ok(()) => {}
        Err(StartRefused::AlreadyStarted(_)) => {
            return Served::Refused(AccessPointError::AlreadyServing);
        }
        Err(StartRefused::Unsupported { .. }) => {
            return Served::Refused(AccessPointError::Unsupported);
        }
    }
    *serving = true;
    let beaconing = || {
        let status = access_point.status();
        status.running && status.beacons != 0
    };
    match until(beaconing, SERVE_BOUND).await {
        true => Served::Beaconing {
            address: access_point.address(),
        },
        false => Served::Refused(AccessPointError::Timeout),
    }
}

/// Whether `done` holds within `bound`, read every [`POLL`].
async fn until(done: impl Fn() -> bool, bound: Duration) -> bool {
    let deadline = Instant::now() + bound;
    loop {
        if done() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        Timer::after(POLL).await;
    }
}

fn status(access_point: &AccessPoint, ipv4: Option<Ipv4>) -> Status {
    let status = access_point.status();
    Status {
        address: access_point.address(),
        running: status.running,
        associated: status.associated,
        authorized: status.authorized,
        beacons: status.beacons,
        beacons_expired: status.beacons_expired,
        refused: status.refused,
        rx_dropped: status.rx_dropped,
        ipv4,
    }
}

/// The network's producer: this image sends nothing of its own.
struct Silent;

impl wifi::Produce for Silent {
    type Tag = ();
    fn generation(&self) -> u32 {
        0
    }
    fn current(&self, (): ()) -> bool {
        false
    }
    fn offer(&mut self, _: wifi::NetworkInstant) -> Result<wifi::Offer<()>, wifi::Next> {
        Err(wifi::Next::Idle)
    }
    fn fill(
        &mut self,
        _: wifi::NetworkInstant,
        _: wifi::Work<()>,
        _: &mut wifi::Payload<'_>,
    ) -> Result<(), wifi::Declined> {
        Err(wifi::Declined::Stale)
    }
    fn refused(&mut self, (): (), _: wifi::SendRefusal) {}
    fn ended(&mut self, _: wifi::Work<()>, _: wifi::Outcome) {}
    fn register(&mut self, _: &mut core::task::Context<'_>, _: wifi::Interest) -> bool {
        false
    }
}
