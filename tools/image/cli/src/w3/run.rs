//! AX211 verification of the same W3 router profile and observation path.

use std::{
    io::{BufRead, BufReader, Read, Write},
    path::Path,
    process::{ExitCode, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

use oer_image_build::Result;
use oer_lab_arbiter::{
    claims::{AIR, Claim},
    resource_lock::ResourceLock,
};
use oer_lab_router::{
    credentials, lock,
    mac::Mac,
    router,
    ssh::{Remote, Ssh},
    w3::{
        self, Evidence, Ping, Station,
        stream::{Capture, Departure},
    },
};
use oer_os_process::owned::Child;
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LaptopLease {
    address: std::net::Ipv4Addr,
    router: std::net::Ipv4Addr,
    prefix: u8,
}

struct Laptop {
    child: Child,
    input: std::process::ChildStdin,
    output: mpsc::Receiver<std::result::Result<String, &'static str>>,
}
impl Laptop {
    fn start(root: &Path, ssid: &str, channel: u8) -> Result<Self> {
        let mut cmd = oer_os_process::command("python3");
        cmd.arg(root.join("tools/lab/router/stand/w3-ax211.py"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        let mut child = Child::spawn_with_shutdown_grace(&mut cmd, Duration::from_secs(20))?
            .with_timeout(Duration::from_secs(240));
        let input = child.stdin.take().ok_or("laptop helper has no stdin")?;
        let mut stream = BufReader::new(child.take_stdout().ok_or("laptop helper has no stdout")?);
        let (sender, output) = mpsc::channel();
        std::thread::spawn(move || {
            // The protocol contains exactly three replies: lease, ping and
            // cleanup. Bound each reply before allocating or parsing it.
            for _ in 0..3 {
                let mut line = String::new();
                let result = match stream.by_ref().take(1025).read_line(&mut line) {
                    Ok(0) => Err("laptop helper closed its output"),
                    Ok(_) if line.len() > 1024 => Err("oversized laptop observation"),
                    Ok(_) => Ok(line),
                    Err(_) => Err("cannot read laptop observation"),
                };
                let failed = result.is_err();
                if sender.send(result).is_err() || failed {
                    break;
                }
            }
        });
        let mut laptop = Self {
            child,
            input,
            output,
        };
        writeln!(
            laptop.input,
            "{}",
            serde_json::json!({"ssid":ssid,"channel":channel})
        )?;
        Ok(laptop)
    }
    fn read<T: serde::de::DeserializeOwned>(&mut self) -> Result<T> {
        let deadline = Instant::now() + Duration::from_secs(125);
        loop {
            oer_os_process::check_cancelled()?;
            match self.output.recv_timeout(Duration::from_millis(250)) {
                Ok(line) => {
                    return serde_json::from_str(&line?)
                        .map_err(|_| "invalid laptop observation".into());
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("laptop observer closed".into());
                }
                Err(mpsc::RecvTimeoutError::Timeout) if Instant::now() >= deadline => {
                    return Err("laptop observation timed out".into());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
    fn command(&mut self, command: &str) -> Result<()> {
        writeln!(self.input, "{command}")?;
        Ok(())
    }
    fn disconnect(mut self) -> Result<()> {
        self.command("disconnect")?;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Done {
            disconnected: bool,
        }
        if !self.read::<Done>()?.disconnected || !self.child.wait()?.success() {
            return Err("laptop disconnect/cleanup failed".into());
        }
        Ok(())
    }
}

pub fn laptop(root: &Path) -> Result<ExitCode> {
    let stand = oer_lab_model::Stand::load(&oer_lab_model::path()?)?;
    let cfg = stand.router()?;
    let ingress = stand
        .station_fixture
        .ingress_interface
        .as_ref()
        .ok_or("W3 needs the router ingress interface")?
        .expose();
    let target = Mac::parse(std::fs::read_to_string("/sys/class/net/wlan0/address")?.trim())
        .ok_or("invalid laptop station identity")?;
    let mut ssh = Ssh::new(cfg.ssh_target.expose())?;
    let host_key = oer_lab_router::ssh::host_key(&ssh.run(oer_lab_router::ssh::BOOT_ID)?)?;
    let phy = std::fs::canonicalize("/sys/class/net/wlan0/phy80211")?;
    let local_key = format!("local-radio:{}", phy.display());
    let mut request = oer_lab_arbiter::Request::from_environment("cargo fw w3 laptop")?;
    request.claims = vec![
        Claim::exclusive(AIR),
        Claim::exclusive(&host_key),
        Claim::exclusive(&local_key),
    ];
    let mut grant = oer_lab_arbiter::Arbiter::open()?.acquire(&request)?;
    grant.supervise_self(false);
    let _remote_resource = ResourceLock::acquire(&host_key)?;
    let _local_resource = ResourceLock::acquire(&local_key)?;
    let lock_dir = oer_os_durable::xdg::path(oer_os_durable::xdg::Base::Runtime, "routers")?;
    let _router = loop {
        match lock::router(&lock_dir, cfg.ssh_target.expose()) {
            Ok(lock) => break lock,
            Err(error) if error.to_string().contains("busy") => {
                eprintln!("fw: w3 waiting for the router lock");
                oer_os_process::check_cancelled()?;
                std::thread::sleep(Duration::from_secs(1));
            }
            Err(error) => return Err(error),
        }
    };
    let run = root.join("target/fw/runs").join(format!(
        "{}-w3-ax211",
        crate::command::timestamp(std::time::SystemTime::now())
    ));
    std::fs::create_dir(&run)?;
    let ssid = credentials::ssid("oer-w3")?;
    let profile = router::Profile::w3(
        cfg.radio,
        cfg.ap_section,
        cfg.wireless_interface.expose(),
        cfg.channel,
        ssid.expose(),
    )?;
    let applied = router::apply(&mut ssh, &profile)?;
    w3::verify_open_ht20(&mut ssh, cfg.radio, cfg.wireless_interface.expose())?;
    eprintln!(
        "fw: w3 open HT20 profile applied, HE disabled, channel {}",
        applied.channel
    );
    let before = w3::station(&mut ssh, cfg.wireless_interface.expose(), target)?;
    if before.present {
        return Err("laptop already associated before W3 run".into());
    }
    let mut station = Laptop::start(root, ssid.expose(), applied.channel)?;
    let laptop_lease: LaptopLease = station.read()?;
    let joined = w3::station(&mut ssh, cfg.wireless_interface.expose(), target)?;
    let lease = w3::lease(&mut ssh, target)?;
    let router_address = w3::router_address(&mut ssh, ingress)?;
    let lease_matches = lease
        .is_some_and(|l| l.address == laptop_lease.address && l.remaining_seconds > 0)
        && laptop_lease.router == router_address
        && (1..=32).contains(&laptop_lease.prefix);
    eprintln!(
        "fw: w3 router association={}, DHCP match={}",
        joined.joined(),
        lease_matches
    );
    let departure = Departure::start(&ssh, target)?;
    let capture = Capture::start(&ssh, ingress, laptop_lease.address)?;
    eprintln!("fw: w3 router capture ready; ping 100 x 1s in each direction");
    let ping_ssh = Ssh::new(cfg.ssh_target.expose())?;
    let address = laptop_lease.address;
    let (target_ping, router_ping) = std::thread::scope(|scope| -> Result<(Ping, Ping)> {
        let ping = scope.spawn(|| w3::ping(&ping_ssh, address));
        station.command("ping")?;
        let target_ping = station.read()?;
        let router_ping = ping.join().map_err(|_| "router ping worker panicked")??;
        Ok((target_ping, router_ping))
    })?;
    let (echoes, capture_complete) = capture.finish()?;
    station.disconnect()?;
    let after = wait_departure(&mut ssh, cfg.wireless_interface.expose(), target)?;
    // hostapd's log notification can follow the nl80211 station deletion.
    ssh.pause(Duration::from_secs(1));
    let disconnect_event = departure.finish()?;
    let evidence = Evidence {
        schema: 1,
        target: target.masked(),
        bssid: applied.bssid.masked(),
        channel: applied.channel,
        open: true,
        ht20: true,
        he_disabled: true,
        before,
        joined,
        lease,
        router_address,
        router_ping,
        echoes,
        capture_complete,
        disconnect_event,
        after,
    };
    std::fs::write(
        run.join("router.json"),
        serde_json::to_vec_pretty(&evidence)?,
    )?;
    std::fs::write(
        run.join("station.json"),
        serde_json::to_vec_pretty(&target_ping)?,
    )?;
    let gates = [
        joined.joined(),
        lease_matches,
        target_ping.passed()
            && capture_complete
            && w3::echo_gate(&evidence.echoes, laptop_lease.address, router_address),
        router_ping.passed()
            && capture_complete
            && w3::echo_gate(&evidence.echoes, router_address, laptop_lease.address),
        disconnect_event && !after.present,
    ];
    for (name, pass) in [
        "a association",
        "b DHCP",
        "c station-to-router",
        "d router-to-station",
        "departure",
    ]
    .into_iter()
    .zip(gates)
    {
        println!(
            "fw: w3 AX211 {name}: {}",
            if pass { "pass" } else { "fail" }
        );
    }
    println!(
        "fw: w3 AX211 run {}",
        run.file_name().ok_or("missing run id")?.to_string_lossy()
    );
    Ok(ExitCode::from(if gates.into_iter().all(|p| p) {
        0
    } else {
        1
    }))
}

fn wait_departure(remote: &mut dyn Remote, interface: &str, target: Mac) -> Result<Station> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let state = w3::station(remote, interface, target)?;
        if !state.present || Instant::now() >= deadline {
            return Ok(state);
        }
        remote.pause(Duration::from_millis(250));
    }
}
