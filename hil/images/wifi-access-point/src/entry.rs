// The access-point image: the library's boot with the Wi-Fi access point
// and the console, the HIL agent serving the base module and the
// access-point service, and one worker that answers its requests in order.

use oer::{
    AppResult, Boot, Console, Error, Peripherals, Started,
    time::{Duration, Instant, Timer},
    wifi::{self, AccessPoint, ApSettings, Ipv4Configuration, StartRefused},
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
/// The access point's static address on its own network, without a
/// gateway.
const ADDRESS: Ipv4Configuration = Ipv4Configuration {
    address: [192, 168, 4, 1],
    prefix_length: 24,
    gateway: None,
};
/// The PMK derivation, the radio's start and the first beacons.
const SERVE_BOUND: Duration = Duration::from_secs(20);
/// The stations' teardown and the radio's stop.
const STOP_BOUND: Duration = Duration::from_secs(10);
/// How often a bounded wait reads the status.
const POLL: Duration = Duration::from_millis(50);

static MAILBOX: ConstStaticCell<Mailbox<Request>> = ConstStaticCell::new(Mailbox::new());

oer::entry!(boot);

fn boot(p: Peripherals, mut boot: Boot) -> Result<impl Started, Error> {
    let config = wifi::Config::new(COUNTRY, IDLE_CHANNEL, TX_CAP_DBM)?;
    let access_point = AccessPoint::start(p.wifi, config, ADDRESS, &mut boot)?;
    let console = Console::open(p.console, &mut boot)?;
    let evidence = boot.evidence();
    let (outbox, inbox) = MAILBOX.try_take().ok_or(Error::Storage)?.split();
    Ok(boot.run(move || {
        oer_hil_agent::serve(console, evidence, services::IMAGE, outbox, move || {
            worker(access_point, inbox)
        })
    }))
}

/// Answer each request in order, each to completion.
async fn worker(access_point: AccessPoint, mut requests: Inbox<'static, Request>) -> AppResult {
    let mut serving = false;
    loop {
        let (identity, request) = requests.next().await;
        match request {
            Request::Serve(request) => {
                let served = serve(&access_point, &mut serving, request).await;
                answer(identity, &served);
            }
            Request::GetStatus(_) => answer(identity, &status(&access_point)),
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

fn status(access_point: &AccessPoint) -> Status {
    let status = access_point.status();
    let ipv4 = match access_point.net_status().ipv4 {
        wifi::Ipv4Status::Static(config) | wifi::Ipv4Status::Lease(config) => Some(Ipv4 {
            address: config.address,
            prefix: config.prefix_length,
        }),
        wifi::Ipv4Status::Unconfigured => None,
    };
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
