//! One fixed-condition vendor/source example, never a calibration-quality gate.
use crate::{
    Result,
    calibration_capture::{space::Space, vendor},
    comparison::{Comparison, HealthRegister},
};
use oer_hil_model::health::{Pages, Snapshot};
use oer_hil_protocol::{
    health::*,
    phy::{CaptureRadio, PrepareCapture},
};
use oer_hil_scenario::workload::context::Context;
use oer_image_build::esp_idf::catalog;
use std::{
    io::{Read, Write},
    path::Path,
    time::{Duration, Instant},
};
const REPLY: Duration = Duration::from_secs(5);

pub(crate) fn run<C: Comparison>(
    config: BeginHealth,
    output: &Path,
    context: &Context<'_>,
) -> Result<()> {
    let root = oer_os_process::built_root();
    let entries = catalog::entries(&root)?;
    let entry = catalog::entry(&entries, "vendor-calibration")?;
    if entry.chip != C::CHIP || entry.hold.is_some() {
        return Err("health reference is held or targets another chip".into());
    }
    let build = catalog::build(&root, &entry.image)?;
    let bundle = catalog::bundle(&root, entry, &build, &output.join("vendor-image"))?;
    let registers = C::health_registers(&root)?;
    if registers.is_empty() {
        return Err("chip has no source-qualified health registers".into());
    }
    let images = context.images()?;
    let source_sha = oer_os_durable::sha256_file(&images.scenario_image().application())?;
    images.flash(&bundle, &entry.image)?;
    let vendor_result = (|| -> Result<serde_json::Value> {
        let (console, mut serial) = super::calibration::vendor_boot(&context.lab.dut.serial)?;
        std::fs::write(output.join("vendor-boot.log"), &console)?;
        if vendor::home_channel(&console)? != config.channel {
            return Err("vendor channel differs from fixed health channel".into());
        }
        let clock = Instant::now();
        let before = words(
            &mut serial,
            &registers,
            &output.join("vendor-before.log"),
            clock,
        )?;
        // One bounded observation, with no reset, counter clear or reconfiguration.
        // Console reads provide the wait while detecting an unexpected new boot.
        let start = Instant::now();
        let mut during = String::new();
        let mut bytes = [0; 256];
        while start.elapsed() < Duration::from_secs(1) {
            match serial.read(&mut bytes) {
                Ok(n) => during.push_str(&String::from_utf8_lossy(&bytes[..n])),
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => return Err(e.into()),
            }
            if vendor::reports(&during) != 0 {
                return Err("vendor reset during health window".into());
            }
        }
        std::fs::write(output.join("vendor-window.log"), &during)?;
        let after = words(
            &mut serial,
            &registers,
            &output.join("vendor-after.log"),
            clock,
        )?;
        Ok(serde_json::json!({ "before": before, "after": after,
            "timebase": "host monotonic brackets per cut; no device acquisition timestamps",
            "unavailable": ["temperature acquisition age", "tracking branch timeline", "RF deafness edges", "intermediate DC/IQ/PWDET events", "per-frame RSSI and rate"] }))
    })();
    // Restore even when the reference rejected/reset/hung. Never mix boots in a report.
    let restored = images.restore();
    let vendor = vendor_result?;
    restored?;
    let native = context.with_capture(&output.join("native"), |capture| {
        capture.discover(Duration::from_secs(15))?;
        let boot = capture
            .latest_boot_id()
            .ok_or("health discovery omitted boot identity")?;
        let started = capture.request_boot(boot, 0, config, REPLY)?;
        if started.boot != boot {
            return Err("health boot identity differs from discovery".into());
        }
        let mut identity = HealthIdentity {
            observation: started.observation,
            epoch: 0,
        };
        let work =
            (|| -> Result<HealthWindow> {
                let ready = super::full(context, capture, PrepareCapture {
                    channel: config.channel,
                    radio: CaptureRadio::Wifi,
                })?;
                identity.epoch = ready.epoch;
                if ready.channel != config.channel || ready.radio != CaptureRadio::Wifi {
                    return Err("native capture changed fixed channel/radio".into());
                }
                Ok(capture.request_boot(
                    boot,
                    0,
                    RunHealthWindow(identity),
                    Duration::from_secs(30),
                )?)
            })();
        // A failed calibration/window still reaches the protocol's frozen failure evidence.
        let finished =
            capture.request_boot(boot, 0, FinishHealth(identity), Duration::from_secs(30));
        let finished = finished?;
        context.results.observe("health-finished", &finished);
        macro_rules! pages {
            ($field:ident, $request:ident, $limit:ident) => {{
                let mut pages = Pages::new(identity, finished.counts.$field);
                while let Some(request) = pages.next($limit) {
                    let page = capture.request_boot(boot, 0, $request(request), REPLY)?;
                    pages.append(page.identity, page.first, page.total, page.records, request)?;
                }
                pages.finish()?
            }};
        }
        let counters = pages!(counters, ReadHealthCounters, COUNTER_PAGE);
        let signals = pages!(signals, ReadHealthSignals, SIGNAL_PAGE);
        let tracking = pages!(tracking, ReadHealthTracking, DETAIL_PAGE);
        let measurements = pages!(measurements, ReadHealthMeasurements, DETAIL_PAGE);
        let window = match work {
            Ok(window) => window,
            Err(error) => {
                let partial = serde_json::json!({ "verdict": "INCOMPLETE", "failure": error.to_string(),
                    "started": started, "finished": finished, "counters": counters, "signals": signals,
                    "tracking": tracking, "measurements": measurements });
                std::fs::write(output.join("native-incomplete.json"), serde_json::to_vec_pretty(&partial)?)?;
                context.results.observe("radio-health-incomplete", &partial);
                capture.request_boot(boot, 0, ReleaseHealth(identity), REPLY)?;
                return Err(error);
            }
        };
        let snapshot = Snapshot::assemble(config, started, window, finished, counters, signals, tracking, measurements)?;
        let released = capture.request_boot(boot, 0, ReleaseHealth(identity), REPLY)?;
        if released.0 != identity {
            return Err("health release identity changed".into());
        }
        Ok(snapshot)
    })?;
    let report = serde_json::json!({ "schema": 1, "configured": config,
        "source_application_sha256": source_sha, "vendor_application_sha256": build.application_sha256,
        "vendor_idf_revision": build.idf_revision, "vendor": vendor, "source": native,
        "interpretation": "diagnostic example; counter cuts are non-atomic, deltas modulo, multiple wraps indeterminate; no new CCA/AGC/NF policy",
        "source_unavailable": ["ten-bit NF/ambient RSSI bank: read side effects unknown", "Celsius conversion", "unmeasured post-correction residuals"] });
    std::fs::write(
        output.join("health.json"),
        serde_json::to_vec_pretty(&report)?,
    )?;
    context.results.observe("radio-health", &report);
    if !native.complete {
        return Err("radio health INCOMPLETE: retained failure/overflow/cleanup evidence".into());
    }
    if !native
        .signals
        .iter()
        .any(|s| matches!(s.value, SignalValue::Received { peer, .. } if peer == config.peer))
    {
        return Err("radio health INCOMPLETE: no RX descriptor from fixed peer".into());
    }
    Ok(())
}

/// Only the chip's source-qualified direct words enter this reader. No discovery
/// resets or recovery occur: any new boot or timeout invalidates the whole cut.
fn words(
    serial: &mut (impl Read + Write),
    registers: &[HealthRegister],
    log: &Path,
    clock: Instant,
) -> Result<serde_json::Value> {
    let first_micros = clock.elapsed().as_micros();
    let mut console = String::new();
    let mut values = Vec::new();
    let result = (|| -> Result<()> {
        for register in registers {
            let from = console.len();
            serial.write_all(Space::Mmio.request(register.address).as_bytes())?;
            let deadline = Instant::now() + REPLY;
            let raw = loop {
                if vendor::reports(&console[from..]) != 0 {
                    return Err("vendor reset in measured health cut".into());
                }
                if let Some(raw) = Space::Mmio
                    .replies(&console[from..])?
                    .get(&register.address)
                {
                    break *raw;
                }
                if Instant::now() >= deadline {
                    return Err(format!("vendor health read timed out at {}", register.name).into());
                }
                let mut bytes = [0; 256];
                match serial.read(&mut bytes) {
                    Ok(n) => console.push_str(&String::from_utf8_lossy(&bytes[..n])),
                    Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(e) => return Err(e.into()),
                }
            };
            let fields: Vec<_> = register.fields.iter().map(|f| serde_json::json!({
                "name": f.name, "width": f.width, "raw": f.value(raw),
                "semantics": if f.name == "CFO_ACCUMULATOR" { "signed accumulator; native decoder scales by 40, not Hz" }
                    else if register.name.contains("NOISE_FLOOR") { "sixteenths dB code, not an event counter" }
                    else { "modulo event counter; multiple wraps indeterminate" }
            })).collect();
            values.push(serde_json::json!({ "name": register.name, "raw": raw, "fields": fields }));
        }
        Ok(())
    })();
    std::fs::write(log, &console)?;
    result?;
    Ok(
        serde_json::json!({ "host_first_micros": first_micros, "host_last_micros": clock.elapsed().as_micros(), "atomic": false, "words": values }),
    )
}
