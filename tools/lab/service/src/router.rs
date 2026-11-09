//! The lab router's operations, run by the service over the router's SSH
//! with the logic of `oer-tools-lab-router`: the AP profile a run applies, the
//! station, station link, DHCP and ping views, the WPA2 checks, a deauthentication, the
//! routed network's check, the echo and departure observers, the AP
//! radio's airtime queue limits and a monitor capture. Every one needs a lease that holds the
//! router. The stand file names the router and
//! its AP; its secrets never leave the service.

use std::{sync::atomic::Ordering, time::Duration};

use oer_tools_lab_arbiter::journal::{AirtimeChange, AirtimePhase, BoardEventKind, Journal};
use oer_tools_lab_client::protocol::{RouterAccessCategory, RouterChannelWidth, RouterOperation};
use oer_tools_lab_model::Stand;
use oer_tools_lab_router::{
    mac::Mac,
    monitor,
    router::{ChannelWidth, Profile},
    ssh::{Remote, Ssh},
    station::{
        self,
        airtime::{self, AccessCategory, Limit},
        stream,
    },
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

/// Run `operation` on the router of `stand`; a change of the router's
/// airtime limits goes into `journal`.
pub(crate) fn run(
    operation: &RouterOperation,
    stand: &Stand,
    io: &OperationIo,
    journal: &Journal,
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
            width,
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
            )?
            .width(match width {
                RouterChannelWidth::Ht20 => ChannelWidth::Ht20,
                RouterChannelWidth::Ht40 => ChannelWidth::Ht40,
                RouterChannelWidth::He20 => ChannelWidth::He20,
            });
            let profile = match forward_host {
                Some(host) => profile.forwarding_host(*host),
                None => profile,
            };
            let applied = oer_tools_lab_router::router::apply(&mut router.ssh, &profile)?;
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
        RouterOperation::StationLink { mac: station } => json(station::link::link(
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
        RouterOperation::SetAirtimeLimit {
            ac,
            low_us,
            high_us,
            station,
        } => {
            let station = station.as_deref().map(mac).transpose()?;
            let owner = io.owner.clone();
            json(airtime::set_recorded(
                &mut router.ssh,
                &router.radio,
                category(*ac),
                Limit {
                    low_us: *low_us,
                    high_us: *high_us,
                },
                station.map(|station| (router.interface.as_str(), station)),
                &mut |entry| record(journal, owner.clone(), entry),
            )?)
        }
        RouterOperation::MonitorCapture { channel } => {
            if !router.channels.contains(channel) {
                return Err(format!(
                    "channel {channel} is not among the router's channels {:?} in the stand file",
                    router.channels
                ));
            }
            let mut host = monitor::ssh::SshHost::new(router.ssh.parallel(), &router.radio);
            let mut sink = Capture { io };
            json(monitor::capture(&mut host, *channel, &mut sink)?)
        }
        RouterOperation::AirtimeLimits { mac: station } => {
            let station = station.as_deref().map(mac).transpose()?;
            json(airtime::read(
                &mut router.ssh,
                &router.radio,
                station.map(|station| (router.interface.as_str(), station)),
            )?)
        }
    }
}

/// The capture's stream: the operation's output, which refuses what its
/// client does not take; the operation's stop is the cancellation.
struct Capture<'a> {
    io: &'a OperationIo,
}

impl monitor::Sink for Capture<'_> {
    fn write(&mut self, bytes: &[u8]) -> Result<()> {
        self.io.output.try_write(bytes)
    }

    fn cancelled(&self) -> bool {
        self.io.stop.load(Ordering::SeqCst)
    }
}

fn category(ac: RouterAccessCategory) -> AccessCategory {
    match ac {
        RouterAccessCategory::Vo => AccessCategory::Vo,
        RouterAccessCategory::Vi => AccessCategory::Vi,
        RouterAccessCategory::Be => AccessCategory::Be,
        RouterAccessCategory::Bk => AccessCategory::Bk,
    }
}

/// Journal one phase of a change of the router's airtime limits under
/// `owner`.
fn record(journal: &Journal, owner: String, entry: &airtime::Record<'_>) -> Result<()> {
    journal
        .record_by(
            owner,
            None,
            BoardEventKind::RouterAirtimeLimit {
                phase: match entry.phase {
                    airtime::Phase::Intended => AirtimePhase::Intended,
                    airtime::Phase::Confirmed => AirtimePhase::Confirmed,
                    airtime::Phase::Unconfirmed => AirtimePhase::Unconfirmed,
                },
                changes: entry
                    .changes
                    .iter()
                    .map(|change| AirtimeChange {
                        ac: change.ac.label().to_ascii_lowercase(),
                        before: [change.before.low_us, change.before.high_us],
                        after: [change.after.low_us, change.after.high_us],
                    })
                    .collect(),
                station: entry.station,
                restored: entry.restored,
                reason: entry.reason.map(str::to_owned),
            },
        )
        .map_err(|error| error.to_string())
}

/// The owner the end state's journal entries name.
const END_STATE: &str = "lab-service";

/// Return the radio's airtime limits to mac80211's defaults, confirmed by
/// reading them back, and journal each category that changed.
fn restore_airtime(router: &mut Fixture, journal: &Journal) -> Result<()> {
    // Without mac80211's file no lease could have set them.
    if !airtime::present(&mut router.ssh, &router.radio)? {
        return Ok(());
    }
    airtime::restore_recorded(&mut router.ssh, &router.radio, &mut |entry| {
        record(journal, END_STATE.to_owned(), entry)
    })?;
    Ok(())
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
/// on its radio's PHY, so it does not radiate between leases: one netifd
/// does not own, as a monitor a run added, is removed. A run's profile
/// enables the section again. Its radio's airtime limits return to
/// mac80211's defaults, journaled in `journal`. Both are attempted
/// whatever the other's outcome; the end state is confirmed only when both
/// are.
pub(crate) fn park(stand: &Stand, journal: &Journal) -> Result<()> {
    let mut router = fixture(stand)?;
    let quiet = quiet(&mut router);
    let airtime = restore_airtime(&mut router, journal);
    end_state(quiet, airtime)
}

/// The end state is confirmed only when the radio is quiet and its airtime
/// limits are restored; each failure keeps its reason.
fn end_state(quiet: Result<()>, airtime: Result<()>) -> Result<()> {
    match (quiet, airtime) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) => Err(error),
        (Ok(()), Err(error)) => Err(format!("its airtime limits: {error}")),
        (Err(quiet), Err(airtime)) => Err(format!("{quiet}; its airtime limits: {airtime}")),
    }
}

/// The AP section disabled and no interface on the radio's PHY.
fn quiet(router: &mut Fixture) -> Result<()> {
    let section =
        oer_tools_lab_router::router::quote(&format!("wireless.{}.disabled=1", router.ap_section))?;
    router.ssh.run(&format!(
        "set -e\nuci set {section}\nuci commit wireless\nwifi reload\n"
    ))?;
    let radio = oer_tools_lab_router::router::quote(&router.radio)?;
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
                router.ssh.run(&removal(&names)?)?;
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

/// The script that removes `names`, wherever they still are: the next look
/// tells what stayed.
fn removal(names: &[String]) -> Result<String> {
    names.iter().try_fold(String::new(), |script, name| {
        let name = oer_tools_lab_router::router::quote(name)?;
        Ok(format!("{script}iw dev {name} del 2>/dev/null || true\n"))
    })
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

    #[test]
    fn the_end_state_needs_a_quiet_radio_and_restored_airtime_limits() {
        assert_eq!(end_state(Ok(()), Ok(())), Ok(()));
        let quiet = end_state(Err("loud".into()), Ok(())).unwrap_err();
        assert_eq!(quiet, "loud");
        let airtime = end_state(Ok(()), Err("stuck".into())).unwrap_err();
        assert!(airtime.contains("airtime") && airtime.contains("stuck"));
        let both = end_state(Err("loud".into()), Err("stuck".into())).unwrap_err();
        assert!(both.contains("loud") && both.contains("stuck"));
    }

    #[test]
    fn a_stray_interface_goes_by_its_quoted_name_and_a_gone_one_is_no_failure() {
        assert_eq!(
            removal(&["mon0".to_owned(), "phy0-sta0".to_owned()]).unwrap(),
            "iw dev mon0 del 2>/dev/null || true\niw dev phy0-sta0 del 2>/dev/null || true\n"
        );
        assert_eq!(
            removal(&["a b".to_owned()]).unwrap(),
            "iw dev 'a b' del 2>/dev/null || true\n"
        );
    }

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
