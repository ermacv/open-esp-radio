//! Bounded initial-GTK investigation, distinct from W4 qualification.
use super::workload::{authorized, network, stop_on_error};
use oer_hil_lab::{
    air::{AirCapture, station::Direction},
    config::StationFixtureConfig,
};
use oer_hil_protocol::{network::Ready, wifi::*};
use oer_hil_scenario::{
    Result,
    workload::{context::Context, require_keys},
};
use oer_lab_router::{credentials, mac::Mac, router, ssh::Ssh, w4};
use std::{path::Path, time::Duration};

pub(crate) struct Observer {
    child: oer_os_process::owned::Child,
    reader: Option<std::thread::JoinHandle<Result<String>>>,
}
impl Observer {
    pub(crate) fn start(ssh: &Ssh, script: &str) -> Result<Self> {
        use std::io::Read;
        let mut child = ssh.start(script)?;
        let mut stdout = child.take_stdout().ok_or("observer stdout missing")?;
        let mut stderr = child.take_stderr().ok_or("observer stderr missing")?;
        let reader = std::thread::spawn(move || {
            let drain = std::thread::spawn(move || {
                let _ = std::io::copy(&mut stderr.by_ref().take(8192), &mut std::io::sink());
            });
            let mut bytes = Vec::new();
            let read = stdout.by_ref().take(131073).read_to_end(&mut bytes);
            let _ = drain.join();
            read.map_err(|_| "observer read failed")?;
            if bytes.len() > 131072 {
                return Err("observer exceeded bound".into());
            }
            String::from_utf8(bytes).map_err(|_| "invalid observer encoding".into())
        });
        Ok(Self {
            child,
            reader: Some(reader),
        })
    }
    pub(crate) fn finish(mut self) -> Result<String> {
        self.child.kill()?;
        self.reader
            .take()
            .ok_or("observer reader missing")?
            .join()
            .map_err(|_| "observer reader failed")?
    }
}
/// Project only known hostapd events. A fresh subscription excludes old runs.
pub(crate) fn hostapd(ssh: &Ssh, target: Mac) -> Result<Observer> {
    let mac = target.octets().map(|b| format!("{b:02x}")).join(":");
    let pipeline = format!(
        "logread -f -l 1 | awk -v target={} 'NR>1 && index(tolower($0),target) {{label=\"\"; if(/pairwise key handshake completed/)label=\"pairwise-completed\"; else if(/group key handshake completed/)label=\"group-completed\"; else if(/group key handshake failed/)label=\"group-failed\"; else if(/AP-STA-CONNECTED/)label=\"authorized\"; else if(/AP-STA-DISCONNECTED/)label=\"disconnected\"; if(label!=\"\"){{print substr($0,1,24),label;fflush()}}}}'",
        router::quote(&mac)?
    );
    Observer::start(
        ssh,
        &format!("exec timeout 240 sh -c {}", router::quote(&pipeline)?),
    )
}
#[derive(Debug, serde::Serialize)]
pub(crate) struct Dhcp {
    pub broadcast: bool,
    pub kind: &'static str,
}
pub(crate) fn dhcp(text: &str) -> Vec<Dhcp> {
    let mut broadcast = false;
    text.lines()
        .filter_map(|line| {
            if line.contains("ethertype IPv4") {
                broadcast = line.contains(" > ff:ff:ff:ff:ff:ff,");
            }
            let kind = if line.contains("DHCP-Message") && line.contains(": Offer") {
                "offer"
            } else if line.contains("DHCP-Message") && line.contains(": ACK") {
                "ack"
            } else {
                return None;
            };
            Some(Dhcp { broadcast, kind })
        })
        .collect()
}

pub fn run(output: &Path, context: &Context<'_>) -> Result<()> {
    let StationFixtureConfig::OpenWrt(config) = &context.lab.station_fixture else {
        return Err("managed OpenWrt required".into());
    };
    let directory = oer_os_durable::xdg::path(oer_os_durable::xdg::Base::Runtime, "routers")?;
    let _router = oer_lab_router::lock::router(&directory, &config.ssh_target)?;
    let mut ssh = Ssh::new(&config.ssh_target)?;
    let ssid = credentials::ssid("oer-initial-gtk")?;
    let password = credentials::passphrase()?;
    let profile = router::Profile::initial_gtk_diagnostic(
        &config.radio,
        &config.ap_section,
        &config.wireless_interface,
        config.channel,
        ssid.expose(),
        password.expose(),
    )?;
    let applied = router::apply(&mut ssh, &profile)?;
    w4::verify_runtime(&mut ssh, &config.radio, &config.wireless_interface)?;
    let interval = w4::group_rekey_seconds(&mut ssh, &config.radio, &config.wireless_interface)?;
    context
        .results
        .observe("initial-gtk-rekey-seconds", &interval);
    if interval != 0 {
        return Err("running diagnostic BSS did not disable group rekey".into());
    }
    drop(profile);
    let target = Mac::parse(&context.lab.dut.mac).ok_or("invalid station identity")?;
    // First prove the observer's interface, frequency and BSS filter without
    // starting the DUT. An empty projection stops this investigation here.
    let check = AirCapture::start_in_run(applied.channel)?;
    oer_os_process::sleep(Duration::from_secs(2))?;
    let (check, drops, stats) = check.finish_station(target.octets(), applied.bssid.octets())?;
    context
        .results
        .observe("initial-gtk-observer-health", &stats);
    context
        .results
        .observe("initial-gtk-observer-check", &check);
    if drops > 0
        || !check.iter().any(|f| {
            f.direction == Direction::GroupFromAp
                && f.subtype == 8
                && f.frequency_mhz == 2407 + u16::from(applied.channel) * 5
        })
    {
        return Err("observer has no valid AP beacons on the selected channel; stop before station diagnosis".into());
    }
    let log = hostapd(&ssh, target)?;
    let packets = Observer::start(
        &ssh,
        &format!(
            "exec timeout 100 tcpdump -n -e -tt -vvv -l -i {} 'udp port 67 or 68'",
            router::quote(&config.wireless_interface)?
        ),
    )?;
    let air = AirCapture::start_in_run(applied.channel)?;
    let result = context.with_capture(output, |capture| {
        require_keys::<StartStation>(capture)?;
        require_keys::<StopStation>(capture)?;
        let start = capture.command(
            StartStation(StationStart {
                credentials: NetworkCredentials::try_new(
                    ssid.expose().as_bytes(),
                    password.expose().as_bytes(),
                )?,
                power_save: StationPowerSave::Off,
            }),
            Duration::from_secs(10),
        )?;
        let work: Result<_> = (|| {
            let connected = authorized(capture, start, 0)?;
            let info = network(capture, start, &connected)?;
            context.results.observe("initial-gtk-network", &info);
            Ok(())
        })();
        let counters = capture.request(0, GetStationCounters, Duration::from_secs(10));
        if let Ok(counters) = counters {
            context.results.observe("initial-gtk-counters", &counters);
        }
        let stopped = || -> Result<()> {
            let command = capture.command(StopStation, Duration::from_secs(10))?;
            let stopped: StationStopped = capture.wait_command(command, Duration::from_secs(10))?;
            context.results.observe("initial-gtk-stopped", &stopped.0);
            Ok(())
        };
        let work = match work {
            Ok(()) => stopped(),
            Err(e) => stop_on_error::<()>(Err(e), stopped),
        };
        // The canonical protocol stream retains link, key and receive-drop
        // topics with their boot/epoch identity, never a device verdict.
        let ready = capture.messages_since(start.first_event(), |messages| {
            messages.iter().filter_map(|m| m.decode::<Ready>()).count()
        })?;
        context.results.observe("initial-gtk-ready-count", &ready);
        work
    });
    let (frames, drops, stats) = air.finish_station(target.octets(), applied.bssid.octets())?;
    context.results.observe("initial-gtk-air", &frames);
    context.results.observe("initial-gtk-air-health", &stats);
    context.results.observe("initial-gtk-air-drops", &drops);
    context
        .results
        .observe("initial-gtk-hostapd", &log.finish()?);
    context
        .results
        .observe("initial-gtk-dhcp", &dhcp(&packets.finish()?));
    result?;
    if frames.is_empty() || drops > 0 {
        return Err("invalid initial GTK air evidence".into());
    }
    context.results.claim(
        "initial-gtk-diagnostic",
        &["connected GTK replacement", "W4 qualification"],
    );
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dhcp_records_preserve_broadcast_without_payload_or_identity() {
        let rows = dhcp(
            "1.000000 02:00:00:00:00:01 > ff:ff:ff:ff:ff:ff, ethertype IPv4\n DHCP-Message (53), length 1: Offer\n2.000000 02:00:00:00:00:01 > 02:00:00:00:00:02, ethertype IPv4\n DHCP-Message (53), length 1: ACK\nsecret-key",
        );
        assert_eq!(rows.len(), 2);
        assert!(rows[0].broadcast);
        assert!(!rows[1].broadcast);
        assert!(
            !serde_json::to_string(&rows)
                .expect("evidence")
                .contains("02:00")
        );
    }
}
