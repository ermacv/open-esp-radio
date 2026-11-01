// A Wi-Fi station on the application entry: join the profile's network,
// take an IPv4 address by DHCP and exchange one line over TCP with the
// network's gateway, again after every reconnection.
mod profile;

use core::future::Future;
use oer_espressif::{
    AppResult, Aes, Boot, Error, Peripherals, Started,
    print::{DiagPath, DiagnosticPath, Ipv4},
    println,
    time::{Duration, Timer, with_timeout},
    wifi::{self, Net, NetConfig, Station, embassy_net},
};
use profile::{PROFILE, Security};

oer_espressif::main!(boot);

fn boot(p: Peripherals, mut boot: Boot) -> Result<Started<impl Future<Output = AppResult>>, Error> {
    let aes = Aes::start(p.aes, &mut boot)?;
    let config = wifi::Config::new(PROFILE.country, PROFILE.channel, PROFILE.cap_dbm)?;
    let (station, device) = Station::start(p.wifi, &aes, config, &mut boot)?;
    let net = Net::start(device, NetConfig::Dhcpv4, &mut boot)?;
    Ok(boot.run(app(station, net)))
}

async fn app(mut station: Station, net: Net) -> AppResult {
    // A refused or failed attempt is a result to report, not a fault.
    loop {
        match station.connect(network()?).await {
            Ok(joined) => {
                println!("OER-WIFI connected channel=", joined.channel);
                break;
            }
            Err(error) => {
                println!("OER-WIFI connect-failed ", error);
                Timer::after(Duration::from_secs(2)).await;
            }
        }
    }
    // The station rejoins by itself after a lost association.
    loop {
        let ipv4 = match with_timeout(Duration::from_secs(30), net.wait_ipv4()).await {
            Ok(result) => result?,
            Err(_) => {
                println!("OER-WIFI ipv4-timeout");
                continue;
            }
        };
        println!("OER-WIFI ipv4 ", Ipv4(ipv4.address), "/", ipv4.prefix_length);
        if let Some(gateway) = ipv4.gateway {
            let _ = match exchange(&net, gateway).await {
                Ok(()) => println!("OER-RESULT: pass wifi-station"),
                Err(error) => println!("OER-WIFI tcp-failed ", error),
            };
        }
        net.wait_link_down().await?;
        println!("OER-WIFI link-down");
    }
}

/// The profile's network.
fn network() -> Result<wifi::Profile, Error> {
    Ok(match PROFILE.security {
        Security::Open => wifi::Profile::open(PROFILE.ssid)?,
        Security::Wpa2Personal { passphrase } => wifi::Profile::wpa2(PROFILE.ssid, passphrase)?,
    })
}

/// Why the line exchange failed.
enum Exchange {
    Socket,
    Connect(embassy_net::tcp::ConnectError),
    Io(embassy_net::tcp::Error),
    Closed,
    Mismatch,
}

impl DiagnosticPath for Exchange {
    fn push_path(&self, path: &mut DiagPath) {
        use embassy_net::tcp::{ConnectError, Error};
        match self {
            Self::Socket => path.name("Socket"),
            Self::Connect(error) => {
                path.name("Connect");
                path.name(match error {
                    ConnectError::InvalidState => "InvalidState",
                    ConnectError::Unaddressable => "Unaddressable",
                    ConnectError::NoFreePorts => "NoFreePorts",
                    ConnectError::InUse => "InUse",
                    ConnectError::ConnectionReset => "ConnectionReset",
                    ConnectError::TimedOut => "TimedOut",
                });
            }
            Self::Io(error) => {
                path.name("Io");
                path.name(match error {
                    Error::ConnectionReset => "ConnectionReset",
                });
            }
            Self::Closed => path.name("Closed"),
            Self::Mismatch => path.name("Mismatch"),
        }
    }
}

/// Send one line to the gateway's echo port and read it back.
async fn exchange(net: &Net, gateway: [u8; 4]) -> Result<(), Exchange> {
    const LINE: &[u8] = b"OER-WIFI hello\n";
    let mut rx = [0; 256];
    let mut tx = [0; 256];
    let mut socket = embassy_net::tcp::TcpSocket::new(net.stack(), &mut rx, &mut tx)
        .map_err(|_| Exchange::Socket)?;
    socket.set_timeout(Some(Duration::from_secs(10)));
    let [a, b, c, d] = gateway;
    socket
        .connect((embassy_net::wire::Ipv4Addr::new(a, b, c, d), PROFILE.echo_port))
        .await
        .map_err(Exchange::Connect)?;
    let mut sent = 0;
    while sent < LINE.len() {
        let rest = LINE.get(sent..).ok_or(Exchange::Mismatch)?;
        sent = sent.saturating_add(socket.write(rest).await.map_err(Exchange::Io)?);
    }
    let mut echo = [0; LINE.len()];
    let mut read = 0;
    while read < echo.len() {
        let rest = echo.get_mut(read..).ok_or(Exchange::Mismatch)?;
        match socket.read(rest).await.map_err(Exchange::Io)? {
            0 => return Err(Exchange::Closed),
            count => read = read.saturating_add(count),
        }
    }
    socket.close();
    if echo == LINE { Ok(()) } else { Err(Exchange::Mismatch) }
}
