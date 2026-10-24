//! W3 host acceptance with a Linux station and independent router evidence.
//! Firmware counters and IRQ stack measurements are outside this run's scope.

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    os::unix::net::UnixDatagram,
    path::Path,
    process::{ExitCode, Stdio},
    sync::mpsc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use oer_lab_arbiter::{Arbiter, Request, claims::Claim, resource_lock::ResourceLock};
use oer_lab_router::{
    credentials,
    mac::Mac,
    router,
    ssh::{Remote, Ssh},
    w3::{
        self, Evidence, Ping, Station,
        stream::{Capture, Departure},
    },
};
use oer_os_process::{self as process, owned::Child};
use oer_tool_lab_fixture_install::{
    ArtifactRole, Provider, admit_system,
    network_client::{self, Lease},
};
use serde::{Deserialize, Serialize};

type Result<T> = oer_image_build::Result<T>;

fn net_command(operation: &str) -> std::process::Command {
    let mut command = process::command("sudo");
    command
        .args(["-n", "/usr/local/sbin/open-radio-net", operation])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

fn net_action(operation: &str, input: &[u8]) -> Result<()> {
    let mut child =
        Child::spawn_with_shutdown_grace(&mut net_command(operation), Duration::from_secs(20))?;
    child
        .stdin
        .take()
        .ok_or("fixture has no input")?
        .write_all(input)?;
    let output = child.wait_with_output_timeout(Some(Duration::from_secs(45)))?;
    if !output.status.success() {
        return Err(format!("linux-net {operation} failed").into());
    }
    Ok(())
}

struct Fixture(bool);
impl Fixture {
    fn close(&mut self) -> Result<()> {
        let stop = net_action("stop", &[]);
        let managed = net_action("managed", &[]);
        stop?;
        managed?;
        self.0 = false;
        Ok(())
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if self.0 && process::cleanup(|| self.close()).is_err() {
            eprintln!("fw: AX211 fixture cleanup failed");
        }
    }
}

struct Control {
    socket: UnixDatagram,
    _directory: tempfile::TempDir,
    connected: bool,
}
impl Control {
    fn start(root: &Path) -> Result<Self> {
        let directory = tempfile::Builder::new()
            .prefix("w3-")
            .tempdir_in(root.join("target"))?;
        let socket = UnixDatagram::bind(directory.path().join("control"))?;
        socket
            .connect("/run/open-radio-wpa-control/wlan0")
            .map_err(|_| "cannot connect to private station control socket")?;
        socket.set_read_timeout(Some(Duration::from_millis(200)))?;
        let mut control = Self {
            socket,
            _directory: directory,
            connected: false,
        };
        control.command("ATTACH")?;
        control.command("ENABLE_NETWORK 0")?;
        let end = Instant::now() + Duration::from_secs(30);
        while !control.connected {
            control.receive(end)?;
        }
        Ok(control)
    }
    fn receive(&mut self, deadline: Instant) -> Result<Option<String>> {
        loop {
            process::check_cancelled()?;
            if Instant::now() >= deadline {
                return Err("station control timed out".into());
            }
            let mut buffer = [0; 4096];
            match self.socket.recv(&mut buffer) {
                Ok(count) => {
                    let text =
                        std::str::from_utf8(buffer.get(..count).ok_or("invalid control length")?)
                            .map_err(|_| "invalid station control response")?;
                    if text.starts_with('<') {
                        self.connected |= text.contains("CTRL-EVENT-CONNECTED");
                        return Ok(None);
                    }
                    return Ok(Some(text.trim().to_owned()));
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                    ) => {}
                Err(_) => return Err("cannot receive private station control response".into()),
            }
        }
    }
    fn command(&mut self, command: &str) -> Result<()> {
        self.socket.send(command.as_bytes())?;
        let end = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(reply) = self.receive(end)? {
                if reply != "OK" {
                    return Err("private station control refused operation".into());
                }
                return Ok(());
            }
        }
    }
}

#[derive(Deserialize)]
#[serde(tag = "event", rename_all = "kebab-case", deny_unknown_fields)]
enum Event {
    Bound { lease: Lease },
    Lease { lease: Lease },
    Ping { ping: network_client::Ping },
    Disconnected,
}

struct Session {
    child: Child,
    events: mpsc::Receiver<std::result::Result<Event, &'static str>>,
}
impl Session {
    fn start() -> Result<Self> {
        let mut child = Child::spawn_with_shutdown_grace(
            &mut net_command("client-dhcp"),
            Duration::from_secs(20),
        )?;
        let output = child.take_stdout().ok_or("station session has no output")?;
        let (sender, events) = mpsc::sync_channel(8);
        std::thread::spawn(move || {
            for line in BufReader::new(output.take(8193)).lines().take(9) {
                let event = line
                    .map_err(|_| "cannot read station event")
                    .and_then(|line| {
                        serde_json::from_str(&line).map_err(|_| "invalid station event")
                    });
                if sender.send(event).is_err() {
                    break;
                }
            }
        });
        Ok(Self { child, events })
    }
    fn send(&mut self, command: &str) -> Result<()> {
        writeln!(
            self.child
                .stdin
                .as_mut()
                .ok_or("station session has no input")?,
            "{command}"
        )?;
        Ok(())
    }
    fn event(&self, within: Duration) -> Result<Event> {
        let end = Instant::now() + within;
        loop {
            process::check_cancelled()?;
            if Instant::now() >= end {
                return Err("station event timed out".into());
            }
            match self.events.recv_timeout(Duration::from_millis(200)) {
                Ok(event) => return Ok(event?),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(_) => return Err("station session closed before its observation".into()),
            }
        }
    }
}

#[derive(Serialize)]
struct Report {
    schema: u8,
    scope: &'static str,
    run_id: String,
    source_commit: String,
    source_dirty: bool,
    fixture_generation: String,
    router_profile_changes: Vec<w3::network::Setting>,
    router_service_actions: [&'static str; 5],
    phase: &'static str,
    observations: Option<Evidence>,
    station_lease: Option<Lease>,
    station_ping: Option<network_client::Ping>,
    checks: [bool; 5],
    workload_error: Option<String>,
    cleanup_error: Option<String>,
    passed: bool,
}

fn checks(e: &Evidence, lease: Lease, ping: network_client::Ping) -> [bool; 5] {
    let profile =
        e.schema == 1 && e.open && e.ht20 && e.he_disabled && e.before == Station::default();
    let leased = e
        .lease
        .is_some_and(|l| l.address == lease.address && l.remaining_seconds > 0)
        && lease.router == e.router_address
        && lease.server == e.router_address;
    [
        profile && e.joined.joined(),
        profile && leased,
        profile
            && leased
            && ping.sent == 100
            && ping.received == 100
            && ping.interval_ms == 1000
            && e.capture_complete
            && w3::echo_gate(&e.echoes, lease.address, lease.router),
        profile
            && leased
            && e.router_ping.passed()
            && e.capture_complete
            && w3::echo_gate(&e.echoes, lease.router, lease.address),
        profile && e.disconnect_event && e.after == Station::default(),
    ]
}

pub fn run(root: &Path) -> Result<ExitCode> {
    let _signals = process::install_signal_handlers()?;
    let stand = oer_lab_model::Stand::load(&oer_lab_model::path()?)?;
    let config = stand.router()?;
    let mut ssh = Ssh::new(config.ssh_target.expose())?;
    let router_key = oer_lab_router::ssh::host_key(&ssh.run(oer_lab_router::ssh::BOOT_ID)?)?;
    let radio_key = format!(
        "local-radio:{}",
        fs::canonicalize("/sys/class/net/wlan0/phy80211")?.display()
    );
    let mut request = Request::from_environment("cargo fw w3 ax211")?;
    request.claims = vec![
        Claim::exclusive("air"),
        Claim::exclusive(&router_key),
        Claim::exclusive(&radio_key),
        Claim::shared(oer_tool_lab_fixture_install::resource(Provider::LinuxNet)),
    ];
    eprintln!("fw: AX211 queueing for router, radio and exclusive air");
    let mut grant = Arbiter::open()?.acquire(&request)?;
    grant.supervise_self(false);
    let _router_resource = ResourceLock::acquire(&router_key)?;
    let _radio_resource = ResourceLock::acquire(&radio_key)?;
    let directory = oer_os_durable::xdg::path(oer_os_durable::xdg::Base::Runtime, "routers")?;
    let _router_lock = process::lock::FileLock::wait(
        &oer_lab_router::lock::router_path(&directory, config.ssh_target.expose()),
        process::lock::Mode::Exclusive,
        "fw: waiting for the router lock",
    )?;
    let software = admit_system(Provider::LinuxNet)?;
    // Verify the installed helper's exact contract before changing the radio.
    let helper = software.artifact(ArtifactRole::NetworkHelper)?;
    let output = process::output(
        process::command(helper).arg("capabilities"),
        Some(Duration::from_secs(5)),
    )?;
    if !output.status.success()
        || output.stdout != format!("{}\n", network_client::CONTRACT).as_bytes()
    {
        return Err("installed station fixture contract mismatch".into());
    }
    let run_id = format!(
        "ax211-{}-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        std::process::id()
    );
    let destination = root.join("target/fw/w3-ax211").join(&run_id);
    fs::create_dir_all(&destination)?;
    let source = process::output(
        process::command("git")
            .current_dir(root)
            .args(["rev-parse", "HEAD"]),
        Some(Duration::from_secs(5)),
    )?;
    if !source.status.success() {
        return Err("cannot identify acceptance source".into());
    }
    let dirty = process::output(
        process::command("git")
            .current_dir(root)
            .args(["status", "--porcelain"]),
        Some(Duration::from_secs(5)),
    )?;
    if !dirty.status.success() {
        return Err("cannot identify acceptance source state".into());
    }
    let mut report = Report {
        schema: 1,
        scope: "linux-net-host-acceptance",
        run_id,
        source_commit: String::from_utf8(source.stdout)?.trim().to_owned(),
        source_dirty: !dirty.stdout.is_empty(),
        fixture_generation: software
            .generation()
            .file_name()
            .ok_or("missing generation identity")?
            .to_string_lossy()
            .into_owned(),
        router_profile_changes: w3::network::changes(
            config.radio,
            config.ap_section,
            config.channel,
        ),
        router_service_actions: [
            "firewall reload before W3 activation",
            "network reload; ifup W3",
            "wireless commit; cycle selected radio",
            "firewall reload with active W3 device",
            "dnsmasq reload",
        ],
        phase: "profile",
        observations: None,
        station_lease: None,
        station_ping: None,
        checks: [false; 5],
        workload_error: None,
        cleanup_error: None,
        passed: false,
    };
    eprintln!("fw: AX211 run-id {}", report.run_id);
    let mut fixture = Fixture(true);
    let result = workload(root, &stand, &mut ssh, &mut report);
    report.workload_error = result.err().map(|e| e.to_string());
    report.cleanup_error = process::cleanup(|| fixture.close())
        .err()
        .map(|e| e.to_string());
    report.passed = report.workload_error.is_none()
        && report.cleanup_error.is_none()
        && report.checks.into_iter().all(|c| c);
    fs::write(
        destination.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    for (name, pass) in [
        "association",
        "lease",
        "station-ping",
        "router-ping",
        "departure",
    ]
    .into_iter()
    .zip(report.checks)
    {
        println!("fw: AX211 {name}: {}", if pass { "pass" } else { "fail" });
    }
    if let Some(error) = &report.workload_error {
        eprintln!("fw: AX211 workload: {error}");
    }
    if let Some(error) = &report.cleanup_error {
        eprintln!("fw: AX211 cleanup: {error}");
    }
    println!(
        "fw: AX211 report {}",
        destination.join("report.json").display()
    );
    Ok(ExitCode::from(u8::from(!report.passed)))
}

fn workload(
    root: &Path,
    stand: &oer_lab_model::Stand,
    ssh: &mut Ssh,
    report: &mut Report,
) -> Result<()> {
    let cfg = stand.router()?;
    let interface = cfg.wireless_interface.expose();
    let ssid = credentials::ssid("oer-w3")?;
    let profile = router::Profile::w3(
        cfg.radio,
        cfg.ap_section,
        interface,
        cfg.channel,
        ssid.expose(),
    )?;
    let applied = router::apply(ssh, &profile)?;
    w3::verify_open_ht20(ssh, cfg.radio, interface)?;
    report.phase = "station-setup";
    net_action(
        "client",
        &serde_json::to_vec(&serde_json::json!({
            "ssid_hex": hex::encode(ssid.expose()), "security": "open", "frequency_mhz": 2407 + u16::from(applied.channel) * 5,
            "phy": "ht", "ipv4": {"mode":"dhcp"}
        }))?,
    )?;
    let target = Mac::parse(fs::read_to_string("/sys/class/net/wlan0/address")?.trim())
        .ok_or("invalid station identity")?;
    let before = w3::station(ssh, interface, target)?;
    if before != Station::default() {
        return Err("AX211 was present before association".into());
    }
    let departure = Departure::start(ssh, target)?;
    let _control = Control::start(root)?;
    eprintln!("fw: AX211 associated; acquiring DHCP lease");
    report.phase = "dhcp";
    let mut session = Session::start()?;
    let Event::Bound { lease } = session.event(Duration::from_secs(35))? else {
        return Err("missing initial station lease".into());
    };
    report.station_lease = Some(lease);
    let joined = w3::station(ssh, interface, target)?;
    let router_lease = w3::lease(ssh, target)?;
    // The route source is the router's independently observed L3 endpoint,
    // including APs enslaved to a bridge with no address on the Wi-Fi netdev.
    let router_address = ssh
        .run(&format!(
            "set -e\nip -4 route get {} | awk '{{for(i=1;i<NF;i++) if($i==\"src\") print $(i+1)}}'",
            lease.address
        ))?
        .trim()
        .parse()
        .map_err(|_| "ambiguous router route source")?;
    report.observations = Some(Evidence {
        schema: 1,
        target: target.masked(),
        bssid: applied.bssid.masked(),
        channel: applied.channel,
        open: true,
        ht20: true,
        he_disabled: true,
        before,
        joined,
        lease: router_lease,
        router_address,
        router_ping: Ping::default(),
        echoes: vec![],
        capture_complete: false,
        disconnect_event: false,
        after: joined,
    });
    let capture = Capture::start(ssh, interface, lease.address, 100)?;
    report.phase = "ping";
    eprintln!("fw: AX211 lease observed; exchanging 100 echoes each way at 1 s");
    session.send("ping")?;
    let (station_ping, router_ping) = std::thread::scope(|scope| -> Result<_> {
        let reverse = scope.spawn(|| w3::ping(ssh, lease.address, 100));
        let station = session.event(Duration::from_secs(120));
        let router = reverse
            .join()
            .map_err(|_| "router ping observer panicked")?;
        let Event::Ping { ping } = station? else {
            return Err("missing station ping observation".into());
        };
        Ok((ping, router?))
    })?;
    report.station_ping = Some(station_ping);
    let e = report
        .observations
        .as_mut()
        .ok_or("missing router observations")?;
    e.router_ping = router_ping;
    let (echoes, complete) = capture.finish()?;
    e.echoes = echoes;
    e.capture_complete = complete;
    session.send("status")?;
    let Event::Lease { lease: after_lease } = session.event(Duration::from_secs(5))? else {
        return Err("missing final lease observation".into());
    };
    if (lease.address, lease.router, lease.prefix)
        != (after_lease.address, after_lease.router, after_lease.prefix)
    {
        return Err("station DHCP identity changed".into());
    }
    report.phase = "departure";
    session.send("disconnect")?;
    if !matches!(session.event(Duration::from_secs(20))?, Event::Disconnected)
        || !session
            .child
            .wait_timeout(Some(Duration::from_secs(20)))?
            .success()
    {
        return Err("station disconnect did not complete".into());
    }
    let end = Instant::now() + Duration::from_secs(10);
    loop {
        e.after = w3::station(ssh, interface, target)?;
        if !e.after.present || Instant::now() >= end {
            break;
        }
        process::sleep(Duration::from_millis(250))?;
    }
    e.disconnect_event = departure.finish()?;
    report.checks = checks(e, lease, station_ping);
    report.phase = "complete";
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn host_acceptance_requires_independent_echo_pairs_and_departure() {
        let mut e = crate::w3::tests::evidence();
        let lease = Lease {
            address: "192.0.2.8".parse().unwrap(),
            prefix: 24,
            router: e.router_address,
            server: e.router_address,
            obtained_at: 100,
            expires_at: 3700,
        };
        let ping = network_client::Ping {
            sent: 100,
            received: 100,
            interval_ms: 1000,
        };
        assert_eq!(checks(&e, lease, ping), [true; 5]);
        let mut external = lease;
        external.server = "192.0.2.2".parse().unwrap();
        assert!(!checks(&e, external, ping)[1]);
        e.echoes.remove(0);
        assert!(!checks(&e, lease, ping)[2]);
        e.disconnect_event = false;
        assert!(!checks(&e, lease, ping)[4]);
        e.before.present = true;
        assert_eq!(checks(&e, lease, ping), [false; 5]);
    }
}
