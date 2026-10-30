//! The lab router's operations, run by the service over the router's SSH
//! with the logic of `oer-lab-router`: the AP profile a run applies, the
//! station, DHCP and ping views, the WPA2 checks, a deauthentication, the
//! routed network's check, and the echo and departure observers. Every one
//! needs a lease that holds the router. The stand file names the router and
//! its AP; its secrets never leave the service.

use std::{sync::atomic::Ordering, time::Duration};

use oer_lab_client::protocol::RouterOperation;
use oer_lab_model::Stand;
use oer_lab_router::{
    mac::Mac,
    router::Profile,
    ssh::{Remote, Ssh},
    station::{self, stream},
    station_wpa2,
};
use serde::Serialize;

use crate::service::OperationIo;

type Result<T> = std::result::Result<T, String>;

/// How long an observer looks at its stop flag between checks.
const POLL: Duration = Duration::from_millis(100);

/// The router the stand file names, and its AP.
struct Fixture {
    ssh: Ssh,
    radio: String,
    ap_section: String,
    interface: String,
    channel: u8,
}

fn fixture(stand: &Stand) -> Result<Fixture> {
    let fixture = &stand.station_fixture;
    let missing = |key: &str| format!("the stand file's station_fixture names no `{key}`");
    let target = fixture
        .ssh_target
        .as_ref()
        .ok_or_else(|| missing("ssh_target"))?;
    Ok(Fixture {
        ssh: Ssh::new(target.expose())?,
        radio: fixture.radio.clone().ok_or_else(|| missing("radio"))?,
        ap_section: fixture
            .ap_section
            .clone()
            .ok_or_else(|| missing("ap_section"))?,
        interface: fixture
            .wireless_interface
            .as_ref()
            .ok_or_else(|| missing("wireless_interface"))?
            .expose()
            .to_owned(),
        channel: fixture.channel.ok_or_else(|| missing("channel"))?,
    })
}

fn mac(text: &str) -> Result<Mac> {
    Mac::parse(text).ok_or_else(|| "not a unicast MAC".to_owned())
}

fn json(value: impl Serialize) -> Result<serde_json::Value> {
    serde_json::to_value(value).map_err(|error| error.to_string())
}

/// Run `operation` on the router of `stand`.
pub(crate) fn run(
    operation: &RouterOperation,
    stand: &Stand,
    io: &OperationIo,
) -> Result<serde_json::Value> {
    let mut router = fixture(stand)?;
    let execute = |ssh: &Ssh| {
        let ssh = ssh.parallel();
        move |script: &str| ssh.output(script, Duration::from_secs(30))
    };
    match operation {
        RouterOperation::ApplyStationProfile { ssid, passphrase } => {
            let profile = Profile::station_wpa2(
                &router.radio,
                &router.ap_section,
                &router.interface,
                router.channel,
                ssid,
                passphrase,
            )?;
            let applied = oer_lab_router::router::apply(&mut router.ssh, &profile)?;
            json(applied.describe())
        }
        RouterOperation::VerifyObservationTools => {
            station_wpa2::verify_observation_tools(&mut router.ssh)?;
            Ok(serde_json::Value::Null)
        }
        RouterOperation::VerifyRuntime => {
            station_wpa2::verify_runtime(&mut router.ssh, &router.radio, &router.interface)?;
            Ok(serde_json::Value::Null)
        }
        RouterOperation::GroupRekeySeconds => json(station_wpa2::group_rekey_seconds(
            &mut router.ssh,
            &router.radio,
            &router.interface,
        )?),
        RouterOperation::Station { mac: station } => json(station::station(
            &mut router.ssh,
            &router.interface,
            mac(station)?,
        )?),
        RouterOperation::Lease { mac: station } => {
            json(station::lease(&mut router.ssh, mac(station)?)?)
        }
        RouterOperation::Ping { address, count } => {
            json(station::ping(&router.ssh, *address, *count)?)
        }
        RouterOperation::Deauthenticate { mac: station } => {
            station_wpa2::deauthenticate(execute(&router.ssh), &router.interface, mac(station)?)?;
            Ok(serde_json::Value::Null)
        }
        RouterOperation::PrepareNetwork => json(station::network::prepare(&mut router.ssh)?),
        RouterOperation::VerifyNetwork { before } => {
            station::network::verify(&mut router.ssh, &router.interface, before)?;
            Ok(serde_json::Value::Null)
        }
        RouterOperation::CaptureEchoes { address, count } => {
            let capture =
                stream::Capture::start_peer(&router.ssh, &router.interface, *address, *count)?;
            wait(io);
            let (echoes, ended) = capture.finish()?;
            Ok(serde_json::json!({
                "ended": ended,
                "echoes": echoes
                    .iter()
                    .map(|echo| serde_json::json!({
                        "at_us": echo.at_us,
                        "source": echo.source,
                        "destination": echo.destination,
                        "reply": echo.reply,
                        "identifier": echo.identifier,
                        "sequence": echo.sequence,
                    }))
                    .collect::<Vec<_>>(),
            }))
        }
        RouterOperation::Departure { mac: station } => {
            let departure = stream::Departure::start(&router.ssh, mac(station)?)?;
            wait(io);
            json(departure.finish()?.to_string())
        }
    }
}

/// Wait until the operation is told to stop.
fn wait(io: &OperationIo) {
    while !io.stop.load(Ordering::SeqCst) {
        std::thread::park_timeout(POLL);
    }
}

/// The router's end state after a lease: it answers over SSH and its
/// access point's interface runs as an access point, as the stand file
/// names it. Its observers end by the timeouts they run under, and the next
/// run applies its own profile; what else a command could have changed is
/// marked before it runs, never guessed from here.
pub(crate) fn end_state(stand: &Stand) -> Result<()> {
    let mut router = fixture(stand)?;
    let script = format!(
        "iw dev {} info | sed -n 's/^[[:space:]]*type //p'",
        oer_lab_router::router::quote(&router.interface)?
    );
    let kind = router.ssh.run(&script)?;
    if kind.trim() != "AP" {
        return Err(format!(
            "its access point's interface runs as `{}`, not as an access point",
            kind.trim()
        ));
    }
    Ok(())
}
