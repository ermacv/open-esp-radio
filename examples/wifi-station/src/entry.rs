// A Wi-Fi station on the application entry: join the profile's network,
// take an IPv4 address by DHCP and exchange one datagram over UDP with the
// network's gateway, again after every reconnection.
mod profile;

use oer::{
    AppResult, ApplicationEnd, Boot, Channel, Error, GroupBoot, GroupEnd, GroupPeripherals,
    GroupStarted, Peripherals, Started,
    print::{DiagPath, DiagnosticPath, Ipv4},
    println,
    time::{Duration, Timer, with_timeout},
    wifi::{self, Station},
};
use profile::{PROFILE, Security};

/// The image's network: its brand, its storage and its channel socket.
enum Brand {}
type Storage = wifi::NetworkStorage<Brand, Silent, wifi::Copying>;
type Socket = wifi::Socket<Brand>;

static NETWORK: static_cell::ConstStaticCell<Storage> =
    static_cell::ConstStaticCell::new(Storage::new());

/// The channel socket's port, and the producer's, which sends nothing.
const PORTS: wifi::Ports = wifi::Ports {
    receiving: 40_000,
    sending: 40_001,
};

/// The application asks the network's future for an exchange once linked,
/// and hears the address it held and how the exchange ended.
static EXCHANGES: Channel<(), Result<Leased, Exchange>, 1> = Channel::new();

oer::entry!(boot, group = network);

fn boot(p: Peripherals, mut boot: Boot) -> Result<impl Started, Error> {
    let config = wifi::Config::new(PROFILE.country, PROFILE.channel, PROFILE.cap_dbm)?;
    let station = Station::start(p.wifi, p.aes, config, &mut boot)?;
    let exchanges = EXCHANGES.application(&mut boot)?;
    Ok(boot.run(move || app(station, exchanges)))
}

/// The group: the network over the station's device, leased by DHCP, and
/// the exchanges on its channel socket.
fn network(p: GroupPeripherals, boot: GroupBoot) -> Result<impl GroupStarted, Error> {
    let device = p.device.ok_or(Error::NoDevice)?;
    let storage = NETWORK.try_take().ok_or(Error::Storage)?;
    let (net, socket, mut boot) =
        wifi::Network::start(device, wifi::Addressing::Dhcp, boot, storage, Silent, PORTS)?;
    let requests = EXCHANGES.group(&mut boot)?;
    Ok(boot.run(move || exchanges(net, socket, requests)))
}

/// The address the network held for an exchange.
#[derive(Clone, Copy)]
struct Leased {
    address: [u8; 4],
    prefix: u8,
}

/// For each request: wait for an address with a gateway over an up link,
/// then exchange one datagram with the gateway's echo port.
async fn exchanges(
    net: wifi::Network<Brand, Silent, wifi::Copying>,
    mut socket: Socket,
    mut requests: GroupEnd<(), Result<Leased, Exchange>, 1>,
) -> AppResult {
    loop {
        let () = requests.receive().await;
        let result = match with_timeout(Duration::from_secs(30), leased(&net)).await {
            Ok(Ok((leased, gateway))) => exchange(&mut socket, gateway).await.map(|()| leased),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(Exchange::NoAddress),
        };
        requests.send(result).await;
    }
}

/// The address the network holds once it has one with a gateway.
async fn leased(
    net: &wifi::Network<Brand, Silent, wifi::Copying>,
) -> Result<(Leased, [u8; 4]), Exchange> {
    loop {
        let seen = net.changes();
        let configuration = net.configuration().map_err(|_| Exchange::Watch)?;
        let held = configuration
            .filter(|configuration| configuration.link.state == wifi::LinkState::Up)
            .and_then(|configuration| configuration.ipv4)
            .and_then(|ipv4| {
                let gateway = ipv4.gateway()?;
                Some((
                    Leased {
                        address: ipv4.address(),
                        prefix: ipv4.prefix(),
                    },
                    gateway,
                ))
            });
        if let Some(held) = held {
            return Ok(held);
        }
        net.changed(seen).await;
    }
}

async fn app(
    mut station: Station,
    mut exchanges: ApplicationEnd<(), Result<Leased, Exchange>, 1>,
) -> AppResult {
    // A refused or failed attempt is a result to report, not a fault.
    loop {
        match station.connect(station_profile()?).await {
            Ok(joined) => {
                println!("OER-WIFI connected channel={}", joined.channel);
                break;
            }
            Err(error) => {
                println!("OER-WIFI connect-failed {}", error);
                Timer::after(Duration::from_secs(2)).await;
            }
        }
    }
    // The station rejoins by itself after a lost association.
    loop {
        exchanges.send(()).await;
        let _ = match exchanges.receive().await {
            Ok(leased) => {
                println!("OER-WIFI ipv4 {}/{}", Ipv4(leased.address), leased.prefix);
                println!("OER-RESULT: pass wifi-station")
            }
            Err(error) => println!("OER-WIFI udp-failed {}", error),
        };
        Timer::after(Duration::from_secs(30)).await;
    }
}

/// The profile's network.
fn station_profile() -> Result<wifi::Profile, Error> {
    Ok(match PROFILE.security {
        Security::Open => wifi::Profile::open(PROFILE.ssid)?,
        Security::Wpa2Personal { passphrase } => wifi::Profile::wpa2(PROFILE.ssid, passphrase)?,
    })
}

/// Why the exchange failed.
enum Exchange {
    /// No address with a gateway over an up link within the wait.
    NoAddress,
    /// The watch of the address held a publication.
    Watch,
    /// The network's memory had no block.
    NoBlock,
    /// The socket refused the datagram.
    Refused(wifi::Refusal),
    /// The core did not send it.
    NotSent,
    /// No echo came within the wait.
    NoEcho,
    /// The echo was not the datagram.
    Mismatch,
    /// The socket closed.
    Closed,
}

impl DiagnosticPath for Exchange {
    fn push_path(&self, path: &mut DiagPath) {
        match self {
            Self::NoAddress => path.name("NoAddress"),
            Self::Watch => path.name("Watch"),
            Self::NoBlock => path.name("NoBlock"),
            Self::Refused(refusal) => {
                path.name("Refused");
                path.name(match refusal {
                    wifi::Refusal::Closed => "Closed",
                    wifi::Refusal::Busy => "Busy",
                });
            }
            Self::NotSent => path.name("NotSent"),
            Self::NoEcho => path.name("NoEcho"),
            Self::Mismatch => path.name("Mismatch"),
            Self::Closed => path.name("Closed"),
        }
    }
}

/// Send one datagram to the gateway's echo port and read it back.
async fn exchange(socket: &mut Socket, gateway: [u8; 4]) -> Result<(), Exchange> {
    const LINE: &[u8] = b"OER-WIFI hello\n";
    let remote = wifi::Remote {
        address: gateway,
        port: PROFILE.echo_port,
    };
    let mut block = socket.reserve().await.map_err(|_| Exchange::Closed)?;
    block.fill(LINE).map_err(|_| Exchange::NoBlock)?;
    let mut observer = socket
        .send(remote, block)
        .map_err(|refused| Exchange::Refused(refused.reason))?;
    observer.finished().await;
    match observer.take() {
        Ok(wifi::Sent::Ended(wifi::Outcome::Accepted)) => {}
        _ => return Err(Exchange::NotSent),
    }
    let echo = with_timeout(Duration::from_secs(5), socket.receive())
        .await
        .map_err(|_| Exchange::NoEcho)?
        .map_err(|_| Exchange::Closed)?;
    let same = echo.remote == remote && echo.block.octets() == LINE;
    socket
        .release(echo.block)
        .map_err(|refused| Exchange::Refused(refused.reason))?;
    if same { Ok(()) } else { Err(Exchange::Mismatch) }
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
