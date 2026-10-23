//! Cold full/save/reset/partial exercise through the production R1 owner.
use crate::{
    Result,
    cache::{self, Link, StartRequest},
};
use oer_hil_lab::calibration_cache::{Provenance, Store};
use oer_hil_protocol::phy::*;
use oer_hil_scenario::workload::context::Context;
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub(crate) fn run(boots: u16, output: &Path, context: &Context<'_>) -> Result<()> {
    let image = context.images()?.scenario_image();
    let summary: oer_image_build::sizes::Summary =
        serde_json::from_slice(&oer_hil_runs::format::archived::read_archived(
            &image.directory.join(oer_image_build::sizes::FILE),
        )?)?;
    if summary.provenance.dirty {
        return Err("cache provenance requires a clean producing image".into());
    }
    let provenance = Provenance {
        image_commit: summary.provenance.commit,
        application_sha256: oer_os_durable::sha256_file(&image.application())?,
        run_id: oer_hil_runs::store::run_of(output)
            .ok_or("cache exercise is outside a run bundle")?,
        captured_unix_seconds: 0,
    };
    let store = Store::host()?;
    let mut previous_boot = None;
    for pair in 1..=boots {
        for source in [CalibrationSource::Full, CalibrationSource::HostCache] {
            let name = format!(
                "pair-{pair:02}-{}",
                if source == CalibrationSource::Full {
                    "full"
                } else {
                    "partial"
                }
            );
            let directory = output.join(&name);
            context.with_capture(&directory, |capture| {
                let keys = capture.request_image_keys(Duration::from_secs(3))?;
                if !keys.has::<HostCalibrationCache>() {
                    return Err("R1 image lacks host cache capability".into());
                }
                let boot = capture.latest_boot_id().ok_or("R1 boot has no identity")?;
                if previous_boot == Some(boot) {
                    return Err("cache exercise did not reset into a new boot".into());
                }
                previous_boot = Some(boot);
                let began = Instant::now();
                let link = Link::new(capture);
                let (key, started) = cache::start(
                    &link,
                    &store,
                    StartRequest {
                        boot_id: boot,
                        session_id: 0,
                        chip: context.lab.chip(),
                        board: oer_lab_board::DeviceId::parse(&context.lab.dut.mac)?,
                        capture: PrepareCapture {
                            channel: 1,
                            radio: CaptureRadio::Wifi,
                        },
                        source,
                    },
                )?;
                context.results.observe(format!("{name}-boot"), &boot);
                context.results.observe(format!("{name}-start"), &started);
                context.results.observe(
                    format!("{name}-host-start-us"),
                    &began.elapsed().as_micros(),
                );
                let CalibrationStartState::Ready {
                    decision,
                    capture: ready,
                    ..
                } = started.status.state
                else {
                    return Err("R1 did not become ready".into());
                };
                if ready.epoch != 1
                    || ready.lifecycle != CaptureLifecycle::Cold
                    || ready.channel != 1
                    || ready.radio != CaptureRadio::Wifi
                {
                    return Err("cache exercise reached the wrong capture point".into());
                }
                if source == CalibrationSource::Full {
                    if decision != CalibrationCacheDecision::FullRequested
                        || ready.path != CalibrationPath::FullForCache
                        || !ready.full_calibration_performed
                    {
                        return Err("explicit full calibration changed its native contract".into());
                    }
                    let mut provenance = provenance.clone();
                    provenance.captured_unix_seconds = oer_os_durable::unix_seconds();
                    let path = cache::save(
                        &link,
                        0,
                        &store,
                        &key,
                        &started,
                        provenance,
                        &directory.join("full-snapshot.json"),
                    )?;
                    context.results.observe(format!("{name}-saved"), &path);
                } else {
                    if decision != CalibrationCacheDecision::CandidateSupplied
                        || ready.path != CalibrationPath::PartialFromCache
                        || ready.full_calibration_performed
                    {
                        return Err(
                            "saved full record did not select native cold partial registration"
                                .into(),
                        );
                    }
                    let before = oer_os_durable::sha256_file(&store.path(&key))?;
                    // Warm RF wake remains retained; it cannot reload or promote a cache.
                    let restarted = capture.request_boot(
                        boot,
                        0,
                        RestartCaptureRadio { epoch: ready.epoch },
                        Duration::from_secs(30),
                    )?;
                    if !restarted.rf_closed
                        || !restarted.rf_woken
                        || restarted.ready.epoch <= ready.epoch
                        || restarted.ready.lifecycle != CaptureLifecycle::Restart
                        || restarted.ready.full_calibration_performed
                        || restarted.ready.path != CalibrationPath::PartialFromCache
                    {
                        return Err(
                            "partial boot did not retain the existing RF-wake lifecycle".into()
                        );
                    }
                    if oer_os_durable::sha256_file(&store.path(&key))? != before {
                        return Err("partial/warm boot replaced last-full".into());
                    }
                    context
                        .results
                        .observe(format!("{name}-restart"), &restarted);
                }
                Ok(())
            })?;
        }
    }
    // Missing storage and no host response are separate cold fallback cases.
    for timeout in [false, true] {
        let name = if timeout {
            "no-response"
        } else {
            "missing-file"
        };
        context.with_capture(&output.join(name), |capture| {
            capture.request_image_keys(Duration::from_secs(3))?;
            let boot = capture.latest_boot_id().ok_or("R1 boot has no identity")?;
            if previous_boot == Some(boot) {
                return Err("negative case reused the previous boot".into());
            }
            previous_boot = Some(boot);
            let status = if timeout {
                let started = capture.request_boot(
                    boot,
                    0,
                    StartCalibration {
                        capture: PrepareCapture {
                            channel: 1,
                            radio: CaptureRadio::Wifi,
                        },
                        source: CalibrationSource::HostCache,
                    },
                    Duration::from_secs(3),
                )?;
                if !matches!(started.state, CalibrationStartState::AwaitingCache(_)) {
                    return Err("no-response case did not open the cache window".into());
                }
                let deadline = Instant::now() + Duration::from_secs(120);
                loop {
                    let remaining = deadline
                        .checked_duration_since(Instant::now())
                        .filter(|remaining| !remaining.is_zero())
                        .ok_or("no-response case did not reach a terminal status")?;
                    let status = capture.request_boot(
                        boot,
                        0,
                        GetCalibrationStartStatus {
                            ticket: started.ticket,
                        },
                        remaining,
                    )?;
                    if status.ticket != started.ticket {
                        return Err("negative case received a stale ticket".into());
                    }
                    match status.state {
                        CalibrationStartState::Ready { .. } => break status,
                        CalibrationStartState::Failed { .. } => {
                            return Err("fallback registration failed".into());
                        }
                        _ => {}
                    }
                }
            } else {
                let directory = tempfile::tempdir()?;
                let store = Store::at(directory.path().into());
                cache::start(
                    &Link::new(capture),
                    &store,
                    StartRequest {
                        boot_id: boot,
                        session_id: 0,
                        chip: context.lab.chip(),
                        board: oer_lab_board::DeviceId::parse(&context.lab.dut.mac)?,
                        capture: PrepareCapture {
                            channel: 1,
                            radio: CaptureRadio::Wifi,
                        },
                        source: CalibrationSource::HostCache,
                    },
                )?
                .1
                .status
            };
            context.results.observe(format!("{name}-status"), &status);
            let CalibrationStartState::Ready {
                decision,
                capture: ready,
                ..
            } = status.state
            else {
                return Err("fallback did not become ready".into());
            };
            let expected = if timeout {
                CalibrationCacheDecision::TimedOut
            } else {
                CalibrationCacheDecision::NoCache(CalibrationCacheMiss::Missing)
            };
            if decision != expected
                || ready.path != CalibrationPath::FullForCache
                || !ready.full_calibration_performed
            {
                return Err(
                    "cache miss/timeout did not select explicit native full calibration".into(),
                );
            }
            Ok(())
        })?;
    }
    context.results.claim(
        "cold full/save/reset/partial and bounded missing/no-response paths observed",
        &[
            "vendor NONE+NVS equivalence",
            "RF qualification",
            "w-image cache integration",
        ],
    );
    Ok(())
}
