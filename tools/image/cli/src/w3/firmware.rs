//! Build/run the W3 image under exclusive board, router and air ownership.
use crate::{image, w1};
use oer_lab_arbiter::{Arbiter, Request, claims::Claim, resource_lock::ResourceLock};
use oer_lab_board::{
    lock::DeviceId,
    stand::{Board, Via},
};
use oer_lab_router::{
    credentials,
    mac::Mac,
    router,
    ssh::{Remote, Ssh},
    w3::{
        self, Evidence, Ping, Station,
        stream::{Capture, Departure, DepartureOutcome},
    },
};
use oer_os_process as process;
use serde::Serialize;
use std::{
    fs,
    io::Write,
    path::Path,
    process::ExitCode,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
type Result<T> = oer_image_build::Result<T>;

#[derive(Serialize)]
struct RunReport {
    schema: u8,
    scope: &'static str,
    run_id: String,
    source_commit: String,
    source_dirty: bool,
    phase: &'static str,
    observations: Option<Evidence>,
    load: Option<w3::load::Report>,
    loss_accounting: Option<Vec<super::loss::Row>>,
    gates: Option<[bool; 5]>,
    workload_error: Option<String>,
    passed: bool,
}
pub fn run(
    root: &Path,
    selector: &str,
    timeout: Duration,
    load: Option<w3::load::Plan>,
) -> Result<ExitCode> {
    let _signals = process::install_signal_handlers()?;
    let stand = oer_lab_model::Stand::load(&oer_lab_model::path()?)?;
    let selected = stand.board(selector).ok_or("unknown W3 board")?;
    if selected.chip != image::CHIP {
        return Err("W3 requires ESP32-S31".into());
    }
    let config = stand.router()?;
    let mut ssh = Ssh::new(config.ssh_target.expose())?;
    let router_key = oer_lab_router::ssh::host_key(&ssh.run(oer_lab_router::ssh::BOOT_ID)?)?;
    let target = Mac::parse(&selected.mac).ok_or("invalid W3 station MAC")?;
    let run_id = format!(
        "w3-{}-{}",
        SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs(),
        std::process::id()
    );
    let directory = root.join("target/fw/w3-runs").join(&run_id);
    fs::create_dir_all(&directory)?;
    let mut request = Request::from_environment("cargo fw run w3")?;
    request.run = Some(run_id.clone());
    request.claims = vec![
        Claim::board(&selected.mac),
        Claim::exclusive("air"),
        Claim::exclusive(&router_key),
    ];
    eprintln!("fw: W3 run-id {run_id}; queueing for board, router and exclusive air");
    let mut grant = Arbiter::open()?.acquire(&request)?;
    grant.supervise_self(false);
    let _router_resource = ResourceLock::acquire(&router_key)?;
    let locks = oer_os_durable::xdg::path(oer_os_durable::xdg::Base::Runtime, "routers")?;
    let _router_lock = process::lock::FileLock::wait(
        &oer_lab_router::lock::router_path(&locks, config.ssh_target.expose()),
        process::lock::Mode::Exclusive,
        "fw: waiting for the W3 router lock",
    )?;
    let id = DeviceId::parse(&selected.mac)?;
    let access = grant.device(&id).ok_or("W3 lease lacks board access")?;
    let board = Board::attached(root, &stand, selector)?.lease(access)?;
    let source = process::output(
        process::command("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(root),
        Some(Duration::from_secs(5)),
    )?;
    let dirty = process::output(
        process::command("git")
            .args(["status", "--porcelain"])
            .current_dir(root),
        Some(Duration::from_secs(5)),
    )?;
    if !source.status.success() || !dirty.status.success() {
        return Err("cannot record W3 source identity".into());
    }
    let mut report = RunReport {
        schema: 1,
        scope: "s31-firmware-acceptance",
        run_id,
        source_commit: String::from_utf8(source.stdout)?.trim().to_owned(),
        source_dirty: !dirty.stdout.is_empty(),
        phase: "profile",
        observations: None,
        load: None,
        loss_accounting: None,
        gates: None,
        workload_error: None,
        passed: false,
    };
    let result = (|| -> Result<()> {
        let source = load.map(|_| w3::load::Source::prepare(&ssh)).transpose()?;
        let interface = config.wireless_interface.expose();
        let ssid = credentials::ssid("oer-w3")?;
        let profile = router::Profile::w3(
            config.radio,
            config.ap_section,
            interface,
            config.channel,
            ssid.expose(),
        )?;
        let applied = router::apply(&mut ssh, &profile)?;
        w3::verify_open_ht20(&mut ssh, config.radio, interface)?;
        let before = w3::station(&mut ssh, interface, target)?;
        if before != Station::default() {
            return Err("W3 target present before association".into());
        }
        let build = image::Stand::W3(w1::Config {
            channel: applied.channel,
            bssid: applied.bssid,
            ssid: w1::Ssid::new(ssid.expose().as_bytes())?,
            country: applied.country,
        });
        report.phase = "build";
        let bundle = image::build(
            root,
            super::IMAGE,
            Some(&build),
            false,
            oer_image_checks::Enforced::Only([oer_image_checks::Check::Interrupts].into()),
            None,
        )?;
        if load.is_some() {
            let summary: oer_image_build::sizes::Summary = serde_json::from_slice(&fs::read(
                bundle.directory.join(oer_image_build::sizes::FILE),
            )?)?;
            let runtime = summary
                .elfs
                .get("runtime")
                .ok_or("missing runtime size observations")?;
            let bound = runtime
                .bounds
                .get("CPU0 task stack")
                .ok_or("missing W3 P1 bound")?;
            load_stack_admission(bound.bytes, bound.complete, bound.holes.len())?;
            eprintln!(
                "fw: W3 load preflash P1={} limit=42192 unresolved={}",
                bound.bytes,
                bound.holes.len()
            );
        }
        // Start the bounded subscription after compilation, while the old
        // image is still selected. It must cover flash, the workload and
        // departure, without spending its lifetime on the image build.
        let departure = Departure::start(&ssh, target)?;
        report.phase = "flash";
        board.flash(
            &oer_lab_board::image::Store::open()?,
            &bundle,
            super::IMAGE,
            Via::Usb,
            "cargo fw run w3",
        )?;
        let lines = board.reset_into_console()?.lines();
        report.phase = "association-dhcp";
        let console = std::thread::scope(|scope| -> Result<String> {
            let log_path = directory.join("console.txt");
            let raw_path = directory.join("console.raw.txt");
            let console = scope.spawn(move || -> Result<String> {
                let end = Instant::now() + timeout;
                let mut text = String::new();
                let mut log = fs::File::create(log_path)?;
                let mut raw_log = fs::File::create(raw_path)?;
                let mut diagnostics = crate::diagnostics::Decoder::default();
                while Instant::now() < end {
                    process::check_cancelled()?;
                    match lines.recv_timeout(Duration::from_millis(200)) {
                        Ok(line) if !line.is_empty() => {
                            writeln!(raw_log, "{}", String::from_utf8_lossy(&line))?;
                            // The report is typed notes: the parser reads the
                            // text the host renders from them.
                            let Some(line) = diagnostics.line(&line)? else {
                                continue;
                            };
                            if line.is_empty() {
                                continue;
                            }
                            if text.len() + line.len() > 256 * 1024 {
                                return Err("W3 console exceeded bound".into());
                            }
                            writeln!(log, "{line}")?;
                            println!("{line}");
                            text.push_str(&line);
                            text.push('\n');
                            if line.starts_with("OER-RESULT: ") {
                                return Ok(text);
                            }
                        }
                        Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            return Err("W3 console closed".into());
                        }
                    }
                }
                Err("W3 firmware result timed out".into())
            });
            let end = Instant::now() + Duration::from_secs(65);
            let (joined, lease) = loop {
                let joined = w3::station(&mut ssh, interface, target)?;
                if joined.joined()
                    && let Some(lease) = w3::lease(&mut ssh, target)?
                {
                    break (joined, lease);
                }
                if console.is_finished() {
                    return Err(
                        "W3 firmware stopped before router association/DHCP evidence".into(),
                    );
                }
                if Instant::now() >= end {
                    return Err("W3 router association/DHCP evidence timed out".into());
                }
                process::sleep(Duration::from_millis(100))?;
            };
            let router_address=ssh.run(&format!("set -e\nip -4 route get {} | awk '{{for(i=1;i<NF;i++) if($i==\"src\") print $(i+1)}}'",lease.address))?.trim().parse().map_err(|_|"ambiguous W3 router route source")?;
            let capture = match &source {
                Some(source) => source.capture(interface, lease.address)?,
                None => Capture::start(&ssh, interface, lease.address, 100)?,
            };
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
                lease: Some(lease),
                router_address,
                router_ping: Ping::default(),
                echoes: vec![],
                capture_complete: false,
                disconnect_event: false,
                after: joined,
            });
            report.phase = "ping";
            eprintln!("fw: W3 lease observed; router ping and capture started");
            let ping = if let (Some(source), Some(plan)) = (&source, load) {
                report.load = Some(source.run(lease.address, plan)?);
                let flood = report.load.as_ref().ok_or("missing W3 load report")?.flood;
                Ping {
                    sent: flood.sent,
                    received: flood.received,
                    interval_ms: plan.interval_ms,
                }
            } else {
                w3::ping(&mut ssh, lease.address, 100)?
            };
            let text = console.join().map_err(|_| "W3 console reader panicked")??;
            let (echoes, complete) = capture.finish()?;
            let evidence = report
                .observations
                .as_mut()
                .ok_or("missing W3 observations")?;
            evidence.router_ping = ping;
            evidence.echoes = echoes;
            evidence.capture_complete = complete;
            if let Some(load) = &mut report.load {
                load.observe(&evidence.echoes, lease.address, router_address, complete)?;
            }
            Ok(text)
        })?;
        report.phase = "departure";
        let evidence = report
            .observations
            .as_mut()
            .ok_or("missing W3 observations")?;
        let end = Instant::now() + Duration::from_secs(10);
        loop {
            evidence.after = w3::station(&mut ssh, interface, target)?;
            if !evidence.after.present || Instant::now() >= end {
                break;
            }
            process::sleep(Duration::from_millis(100))?;
        }
        evidence.disconnect_event = departure.finish()? == DepartureOutcome::Unique;
        fs::write(
            directory.join("observations.json"),
            serde_json::to_vec_pretty(evidence)?,
        )?;
        let parsed = super::parse(&console).map_err(|_| "malformed W3 firmware report")?;
        if let Some(load) = &report.load {
            if !evidence.capture_complete {
                return Err("incomplete AP load capture".into());
            }
            report.loss_accounting = Some(super::loss::account(
                &console,
                load.ap_requests,
                load.ap_replies,
            )?);
            let gates = [
                parsed.authenticated && parsed.associated && evidence.joined.joined(),
                parsed.acquired
                    && evidence.lease.is_some_and(|lease| {
                        lease.address == parsed.address && lease.remaining_seconds > 0
                    })
                    && parsed.router == evidence.router_address,
                parsed.ping.passed(),
                load.passed(),
                parsed.deauth_sent
                    && parsed.tx_timeout == 0
                    && parsed.queue_collision == 0
                    && evidence.disconnect_event
                    && evidence.after == Station::default(),
            ];
            // The load recipe has its own peer identities and acceptance; the
            // standard version-1 report and its claims are retained verbatim.
            report.passed = gates[0] && gates[1] && gates[3] && gates[4];
            report.gates = Some(gates);
        } else {
            let checked = super::check(&parsed, evidence)
                .map_err(|_| "W3 report contradicts router observations")?;
            report.gates = Some(checked.gates);
            report.passed = checked.passed();
        }
        if let Some(source) = source {
            source.finish()?;
        }
        report.phase = "complete";
        Ok(())
    })();
    report.workload_error = result.err().map(|error| error.to_string());
    fs::write(
        directory.join("report.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    if let Some(gates) = report.gates {
        for (id, pass) in ['a', 'b', 'c', 'd', 'f'].into_iter().zip(gates) {
            println!("fw: W3 gate {id}: {}", if pass { "pass" } else { "fail" });
        }
    }
    if let Some(error) = report.workload_error {
        eprintln!("fw: W3 {}: {error}", report.phase);
    }
    println!("fw: W3 report {}", directory.join("report.json").display());
    Ok(ExitCode::from(u8::from(!report.passed)))
}

// The image owner's limit in firmware/apps/w3/README.md. Memory growth is a report.
fn load_stack_admission(bytes: u64, complete: bool, unresolved: usize) -> Result<()> {
    if !complete || unresolved != 0 || bytes > 42_192 {
        return Err(
            "W3 load requires a complete P1 bound <=42192 and zero unresolved sites".into(),
        );
    }
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn load_preflash_enforces_the_owner_bound_and_requires_resolved_paths() {
        assert!(load_stack_admission(42192, true, 0).is_ok());
        assert!(load_stack_admission(42193, true, 0).is_err());
        assert!(load_stack_admission(38768, false, 0).is_err());
        assert!(load_stack_admission(38768, true, 1).is_err());
    }
}
