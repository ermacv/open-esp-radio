//! Bounded initial-GTK investigation, distinct from W4 qualification.
use super::workload::{Station, StopEnd, authorized, ended, network, stop_on_error, stop_station};
use oer_hil_lab::{
    air::{AirCapture, station::Direction},
    config::StationFixtureConfig,
};
use oer_hil_scenario::{
    Result,
    workload::{context::Context, require_keys},
};
use oer_ieee80211_control::*;
use oer_lab_router::{
    credentials,
    mac::Mac,
    router,
    ssh::{CommandLog, CommandObservation, Ssh, Stderr, read_stderr},
    w4,
};
use oer_network_control::Ready;
use std::{path::Path, time::Duration};

pub(crate) struct Observer {
    child: oer_os_process::owned::Child,
    reader: Option<std::thread::JoinHandle<Result<String>>>,
    diagnostics: Option<std::thread::JoinHandle<Stderr>>,
    command: Option<CommandObservation>,
}
impl Observer {
    pub(crate) fn start(ssh: &Ssh, script: &str) -> Result<Self> {
        use std::io::Read;
        let mut child = ssh.start(script)?;
        let mut stdout = child.take_stdout().ok_or("observer stdout missing")?;
        let stderr = child.take_stderr().ok_or("observer stderr missing")?;
        let diagnostics = std::thread::spawn(move || read_stderr(stderr, |_| {}));
        let reader = std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let read = stdout.by_ref().take(131073).read_to_end(&mut bytes);
            read.map_err(|_| "observer read failed")?;
            if bytes.len() > 131072 {
                return Err("observer exceeded bound".into());
            }
            String::from_utf8(bytes).map_err(|_| "invalid observer encoding".into())
        });
        Ok(Self {
            child,
            reader: Some(reader),
            diagnostics: Some(diagnostics),
            command: Some(ssh.observe(script)),
        })
    }
    pub(crate) fn finish(mut self) -> Result<String> {
        self.child.kill()?;
        let status = self.child.try_wait()?;
        let diagnostics = self
            .diagnostics
            .take()
            .ok_or("observer stderr reader missing")?
            .join()
            .map_err(|_| "observer stderr reader failed")?;
        let failed = diagnostics.error.is_some();
        if let Some(command) = self.command.take() {
            command.finish(status, &diagnostics.text, diagnostics.error.as_deref());
        }
        if failed {
            return Err("observer stderr failed; see router command evidence".into());
        }
        self.reader
            .take()
            .ok_or("observer reader missing")?
            .join()
            .map_err(|_| "observer reader failed")?
    }
}
impl Drop for Observer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        if let Some(command) = self.command.take() {
            let diagnostics = self
                .diagnostics
                .take()
                .and_then(|reader| reader.join().ok());
            let (stderr, error) = diagnostics.map_or_else(
                || {
                    (
                        String::new(),
                        Some("observer stderr reader unavailable".to_owned()),
                    )
                },
                |diagnostics| (diagnostics.text, diagnostics.error),
            );
            command.finish(
                self.child.try_wait().ok().flatten(),
                &stderr,
                error
                    .as_deref()
                    .or(Some("observer stopped before completion")),
            );
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
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
    let ssid = credentials::ssid("oer-initial-gtk")?;
    let password = credentials::passphrase()?;
    let target = Mac::parse(&context.lab.dut.mac).ok_or("invalid station identity")?;
    let target_text = target.octets().map(|byte| format!("{byte:02x}")).join(":");
    let results = context.results.clone();
    let log = CommandLog::new(
        &[
            ssid.expose(),
            password.expose(),
            &config.ssh_target,
            &config.wireless_interface,
            &target_text,
        ],
        move |record| results.observe("initial-gtk-router-command", &record),
    );
    let mut ssh = Ssh::new(&config.ssh_target)?.with_log(log);
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
        let stopped = || -> Result<Option<StationServiceEnded>> {
            match stop_station(&Station { capture, start })? {
                StopEnd::Stopped(counters) => {
                    context.results.observe("initial-gtk-stopped", &counters);
                    Ok(None)
                }
                StopEnd::Ended(end) => {
                    context.results.observe("initial-gtk-service-ended", &end);
                    Ok(Some(end))
                }
            }
        };
        let work = match work {
            Ok(()) => match stopped()? {
                None => Ok(()),
                Some(end) => Err(ended(&end).into()),
            },
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
