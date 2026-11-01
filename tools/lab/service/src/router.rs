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
            forward_host,
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
            let profile = match forward_host {
                Some(host) => profile.forwarding_host(*host),
                None => profile,
            };
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
        RouterOperation::StationDhcp { serve } => Ok(serde_json::json!({
            "serving": station::network::serve_dhcp(&mut router.ssh, *serve)?,
        })),
        RouterOperation::PrepareNetwork => json(station::network::prepare(&mut router.ssh)?),
        RouterOperation::VerifyNetwork { before } => {
            station::network::verify(&mut router.ssh, &router.interface, before, None)?;
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
/// disabled, the rest of its configuration kept, and no wireless interface
/// on its radio's PHY, so it does not radiate between leases. A run's
/// profile enables the section again.
pub(crate) fn park(stand: &Stand) -> Result<()> {
    let mut router = fixture(stand)?;
    let section =
        oer_lab_router::router::quote(&format!("wireless.{}.disabled=1", router.ap_section))?;
    router.ssh.run(&format!(
        "set -e\nuci set {section}\nuci commit wireless\nwifi reload\n"
    ))?;
    let radio = oer_lab_router::router::quote(&router.radio)?;
    // The radio's PHY index, then every interface: `iw dev` lists nothing
    // for a PHY without interfaces, so only the script's exit, which `run`
    // checks, tells that from a failure.
    let interfaces = format!(
        "set -e\nphy=$(iwinfo nl80211 phyname {radio})\n\
         cat \"/sys/class/ieee80211/$phy/index\"\niw dev\n"
    );
    let started = std::time::Instant::now();
    loop {
        let status = router.ssh.run("ubus call network.wireless status")?;
        let listing = router.ssh.run(&interfaces)?;
        let running = match radiating(&status, &listing, &router.radio)? {
            Radio::Quiet => return Ok(()),
            Radio::Pending => "its radio's reload is still pending".to_owned(),
            Radio::Interfaces(names) => {
                format!("its radio still has the interfaces {}", names.join(", "))
            }
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
    /// netifd applied its configuration and the radio's PHY has no
    /// interface: nothing on it can transmit.
    Quiet,
    /// netifd has not finished applying its configuration.
    Pending,
    /// The radio's PHY has these interfaces, whatever their type: a
    /// station scans and associates, a monitor can inject.
    Interfaces(Vec<String>),
}

/// Judge the router's radio `radio` from netifd's `status` (`ubus call
/// network.wireless status`) and `listing`: the radio's PHY index on its
/// first line, then `iw dev`'s listing of every interface.
fn radiating(status: &str, listing: &str, radio: &str) -> Result<Radio> {
    let status: serde_json::Value = serde_json::from_str(status)
        .map_err(|error| format!("the router's wireless status is not JSON: {error}"))?;
    let state = status
        .get(radio)
        .ok_or_else(|| format!("the router's wireless status names no radio {radio}"))?;
    if state.get("pending").and_then(serde_json::Value::as_bool) != Some(false) {
        return Ok(Radio::Pending);
    }
    let (index, interfaces) = listing.split_once('\n').unwrap_or((listing, ""));
    let index: u32 = index
        .trim()
        .parse()
        .map_err(|_| format!("the router named no PHY index for {radio}: `{index}`"))?;
    let on_phy = phy_interfaces(interfaces, index)?;
    Ok(if on_phy.is_empty() {
        Radio::Quiet
    } else {
        Radio::Interfaces(on_phy)
    })
}

/// The interfaces `iw dev` lists on PHY `index`; an error for a listing it
/// cannot read.
fn phy_interfaces(listing: &str, index: u32) -> Result<Vec<String>> {
    let mut phy = None;
    let mut found = Vec::new();
    for line in listing
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Some(number) = line.strip_prefix("phy#") {
            phy = Some(
                number
                    .parse::<u32>()
                    .map_err(|_| format!("`iw dev` printed an unknown PHY `{line}`"))?,
            );
        } else if let Some(name) = line.strip_prefix("Interface ") {
            match phy {
                Some(phy) if phy == index => found.push(name.trim().to_owned()),
                Some(_) => {}
                None => return Err(format!("`iw dev` listed {name} under no PHY")),
            }
        } else if phy.is_none() {
            return Err(format!("`iw dev` printed `{line}` before any PHY"));
        }
    }
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SETTLED: &str = r#"{"radio0":{"up":true,"pending":false,"interfaces":[]}}"#;
    const IW: &str = "phy#1\n\tInterface phy1-ap0\n\t\ttype AP\n\
                      phy#0\n\tInterface phy0-sta0\n\t\tifindex 9\n\t\ttype managed\n";

    #[test]
    fn the_radio_is_quiet_only_without_any_interface_on_its_phy() {
        // As on the parked router: no interface at all.
        assert_eq!(radiating(SETTLED, "0\n", "radio0"), Ok(Radio::Quiet));
        // A station on the radio's PHY scans and associates.
        assert_eq!(
            radiating(SETTLED, &format!("0\n{IW}"), "radio0"),
            Ok(Radio::Interfaces(vec!["phy0-sta0".to_owned()]))
        );
        // The other radio's AP is another PHY.
        assert_eq!(
            radiating(
                SETTLED,
                "0\nphy#1\n\tInterface phy1-ap0\n\t\ttype AP\n",
                "radio0"
            ),
            Ok(Radio::Quiet)
        );
        assert_eq!(
            radiating(r#"{"radio0":{"pending":true}}"#, "0\n", "radio0"),
            Ok(Radio::Pending)
        );
        // Without a pending flag nothing tells the reload ended.
        assert_eq!(
            radiating(r#"{"radio0":{}}"#, "0\n", "radio0"),
            Ok(Radio::Pending)
        );
    }

    #[test]
    fn a_view_that_does_not_answer_is_an_error_never_a_quiet_radio() {
        assert!(radiating("", "0\n", "radio0").is_err(), "no status");
        assert!(radiating(r#"{"radio1":{"pending":false}}"#, "0\n", "radio0").is_err());
        assert!(radiating(SETTLED, "", "radio0").is_err(), "no PHY index");
        assert!(radiating(SETTLED, "0\ncommand failed: No such device", "radio0").is_err());
        assert!(radiating(SETTLED, "0\nphy#x\n", "radio0").is_err());
    }
}
