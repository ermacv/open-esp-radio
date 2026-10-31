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
    /// The channels the stand may use on it.
    channels: Vec<u8>,
}

fn fixture(stand: &Stand) -> Result<Fixture> {
    let router = stand
        .router
        .as_ref()
        .ok_or("the stand file names no [router]")?;
    if router.kind != "open-wrt" {
        return Err(format!(
            "the stand file's [router] is `{}`; the service drives `open-wrt`",
            router.kind
        ));
    }
    Ok(Fixture {
        ssh: Ssh::new(router.ssh_target.expose())?,
        radio: router.radio.clone(),
        ap_section: router.ap_section.clone(),
        interface: router.wireless_interface.expose().to_owned(),
        channels: router.channels.clone(),
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
        RouterOperation::ApplyStationProfile {
            ssid,
            passphrase,
            channel,
        } => {
            if !router.channels.contains(channel) {
                return Err(format!(
                    "channel {channel} is not among the router's channels {:?} in the stand file",
                    router.channels
                ));
            }
            let profile = Profile::station_wpa2(
                &router.radio,
                &router.ap_section,
                &router.interface,
                *channel,
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

/// How long the router's AP may take to go down after `wifi reload`.
const AP_DOWN: Duration = Duration::from_secs(20);

/// Park the router: its end state after a lease is the stand's AP section
/// disabled, the rest of its configuration kept, and no access point on its
/// radio, so it does not radiate between leases. A run's profile enables
/// the section again.
pub(crate) fn park(stand: &Stand) -> Result<()> {
    let mut router = fixture(stand)?;
    let section =
        oer_lab_router::router::quote(&format!("wireless.{}.disabled=1", router.ap_section))?;
    router.ssh.run(&format!(
        "set -e\nuci set {section}\nuci commit wireless\nwifi reload\n"
    ))?;
    let started = std::time::Instant::now();
    loop {
        // Each view must answer: a failed one is an error, never an AP down.
        let status = router.ssh.run("ubus call network.wireless status")?;
        // `iw dev` lists nothing for a radio without interfaces: only its
        // exit, which `run` checks, tells that from a failure.
        let interfaces = router.ssh.run("iw dev")?;
        let running = match radiating(
            &status,
            &interfaces,
            &router.radio,
            &router.ap_section,
            &router.interface,
        )? {
            Radio::Quiet => return Ok(()),
            Radio::Pending => "its radio's reload is still pending".to_owned(),
            Radio::Ap(interface) => format!("interface {interface} still runs an access point"),
        };
        if started.elapsed() >= AP_DOWN {
            return Err(format!(
                "{running} {} s after the AP section was disabled",
                AP_DOWN.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_secs(1));
    }
}

/// What the router's radio does, as its views tell.
#[derive(Debug, PartialEq)]
enum Radio {
    /// No interface of the radio, nor the stand's AP interface, is an
    /// access point, and the AP section has no interface.
    Quiet,
    /// netifd has not finished applying its configuration.
    Pending,
    /// This interface is an access point.
    Ap(String),
}

/// Judge the router's radio `radio`, with the stand's AP section `section`
/// on `interface`, from netifd's `status` (`ubus call network.wireless
/// status`) and `iw dev`'s listing of every interface.
fn radiating(
    status: &str,
    interfaces: &str,
    radio: &str,
    section: &str,
    interface: &str,
) -> Result<Radio> {
    let status: serde_json::Value = serde_json::from_str(status)
        .map_err(|error| format!("the router's wireless status is not JSON: {error}"))?;
    let name = radio;
    let radio = status
        .get(name)
        .ok_or_else(|| format!("the router's wireless status names no radio {name}"))?;
    if radio.get("pending").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(Radio::Pending);
    }
    let listed = radio
        .get("interfaces")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| format!("the router's status of {name} lists no interfaces"))?;
    let mut names = vec![interface.to_owned()];
    for listed in listed {
        let Some(ifname) = listed.get("ifname").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if listed.get("section").and_then(serde_json::Value::as_str) == Some(section) {
            return Ok(Radio::Ap(ifname.to_owned()));
        }
        names.push(ifname.to_owned());
    }
    let kinds = interface_kinds(interfaces)?;
    Ok(names
        .into_iter()
        .find(|name| kinds.get(name).is_some_and(|kind| kind == "AP"))
        .map_or(Radio::Quiet, Radio::Ap))
}

/// Each interface `iw dev` lists, with its type. A radio without
/// interfaces lists nothing, so only `iw`'s exit tells an answer from a
/// failure; a listing it cannot read is an error.
fn interface_kinds(listing: &str) -> Result<std::collections::BTreeMap<String, String>> {
    let mut kinds = std::collections::BTreeMap::new();
    let mut interface = None;
    for line in listing.lines().map(str::trim) {
        if line.starts_with("phy#") {
            interface = None;
        } else if let Some(name) = line.strip_prefix("Interface ") {
            interface = Some(name.trim().to_owned());
        } else if let (Some(kind), Some(name)) = (line.strip_prefix("type "), &interface) {
            kinds.insert(name.clone(), kind.trim().to_owned());
        }
    }
    if kinds.is_empty() && listing.lines().any(|line| !line.trim().is_empty()) {
        return Err(format!(
            "`iw dev` on the router printed no interface: {listing}"
        ));
    }
    Ok(kinds)
}

#[cfg(test)]
mod tests {
    use super::*;

    const IW: &str = "phy#0\n\tInterface phy0-sta0\n\t\tifindex 9\n\t\ttype managed\n\
                      phy#1\n\tInterface phy1-ap0\n\t\ttype AP\n";

    fn judge(status: &str, iw: &str) -> Result<Radio> {
        radiating(status, iw, "radio0", "stand_ap", "phy0-ap0")
    }

    #[test]
    fn the_radio_is_quiet_only_when_both_views_answer_without_an_ap_on_it() {
        let down = r#"{"radio0":{"pending":false,"interfaces":[
            {"section":"stand_ap"},{"section":"sta","ifname":"phy0-sta0"}]}}"#;
        // The other radio's AP is not the stand's radio.
        assert_eq!(judge(down, IW), Ok(Radio::Quiet));
        let listed = r#"{"radio0":{"pending":false,"interfaces":[
            {"section":"stand_ap","ifname":"phy0-ap0"}]}}"#;
        assert_eq!(judge(listed, IW), Ok(Radio::Ap("phy0-ap0".to_owned())));
        let other_ap = r#"{"radio0":{"interfaces":[{"section":"guest","ifname":"phy1-ap0"}]}}"#;
        assert_eq!(judge(other_ap, IW), Ok(Radio::Ap("phy1-ap0".to_owned())));
        // The stand's interface as an AP, whatever netifd lists.
        let iw_ap = "phy#0\n\tInterface phy0-ap0\n\t\ttype AP\n";
        assert_eq!(
            judge(r#"{"radio0":{"interfaces":[]}}"#, iw_ap),
            Ok(Radio::Ap("phy0-ap0".to_owned()))
        );
        assert_eq!(
            judge(r#"{"radio0":{"pending":true,"interfaces":[]}}"#, IW),
            Ok(Radio::Pending)
        );
    }

    #[test]
    fn a_view_that_does_not_answer_is_an_error_never_an_ap_down() {
        let down = r#"{"radio0":{"interfaces":[]}}"#;
        // Without interfaces `iw dev` prints nothing, as on the parked router.
        assert_eq!(judge(down, ""), Ok(Radio::Quiet));
        assert!(judge(down, "command failed: No such device").is_err());
        assert!(judge("", IW).is_err(), "no status");
        assert!(judge(r#"{"radio1":{"interfaces":[]}}"#, IW).is_err());
        assert!(judge(r#"{"radio0":{}}"#, IW).is_err());
    }
}
