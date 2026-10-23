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
    let provenance =
        producing_provenance(output, &oer_os_durable::sha256_file(&image.application())?)?;
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
                let mut epoch = ready.epoch;
                let measured: Result<()> = (|| {
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
                            return Err(
                                "explicit full calibration changed its native contract".into()
                            );
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
                        epoch = restarted.ready.epoch;
                        if !restarted.rf_closed
                            || !restarted.rf_woken
                            || restarted.ready.epoch <= ready.epoch
                            || restarted.ready.lifecycle != CaptureLifecycle::Restart
                            || restarted.ready.full_calibration_performed
                            || restarted.ready.path != CalibrationPath::PartialFromCache
                        {
                            return Err(
                                "partial boot did not retain the existing RF-wake lifecycle".into(),
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
                })();
                let closed = close(context, capture, boot, epoch, &name);
                measured?;
                closed?;
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
            let closed = close(context, capture, boot, ready.epoch, name);
            if decision != expected
                || ready.path != CalibrationPath::FullForCache
                || !ready.full_calibration_performed
            {
                return Err(
                    "cache miss/timeout did not select explicit native full calibration".into(),
                );
            }
            closed?;
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

fn close(
    context: &Context<'_>,
    capture: &oer_hil_link::SerialCapture,
    boot: u64,
    epoch: u32,
    name: &str,
) -> Result<()> {
    let stopped = capture.request_boot(boot, 0, StopCapture { epoch }, Duration::from_secs(30))?;
    context.results.observe(format!("{name}-stopped"), &stopped);
    if stopped.epoch != epoch || !stopped.rf_closed {
        return Err("cache exercise did not close RF".into());
    }
    Ok(())
}

fn producing_provenance(output: &Path, application_sha256: &str) -> Result<Provenance> {
    use oer_hil_model::schema::image::ImageClass;
    use oer_hil_runs::format::read::{MANIFEST, RunBundle};
    let directory = output
        .ancestors()
        .find(|directory| directory.join(MANIFEST).is_file())
        .ok_or("cache exercise is outside a run bundle")?;
    let run = RunBundle::open(directory)?.ok_or("cache run manifest disappeared")?;
    let firmware = run
        .manifest()
        .firmware
        .iter()
        .find(|firmware| firmware.image == ImageClass::DiagnosticPhy)
        .ok_or("run has no producing R1 firmware record")?;
    if firmware.repository.dirty || firmware.application_sha256 != application_sha256 {
        return Err(
            "cache producing firmware is dirty or does not match the flashed application".into(),
        );
    }
    Ok(Provenance {
        image_commit: firmware.repository.commit.clone(),
        application_sha256: firmware.application_sha256.clone(),
        run_id: run.id().into(),
        captured_unix_seconds: 0,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn provenance_comes_from_the_runs_actual_firmware_not_flash_directory_reports_or_observer_head()
    {
        let directory = tempfile::tempdir().expect("valid private run fixture");
        let mut manifest = oer_hil_runs::run::test_support::manifest();
        manifest.run_id = "cache-run".into();
        manifest.repository.commit = "3".repeat(40);
        manifest.firmware = vec![serde_json::from_value(serde_json::json!({
            "image":"diagnostic-phy", "repository":{"commit":"1".repeat(40),"dirty":false},
            "application_path":"firmware/diagnostic-phy/application.bin", "application_size_bytes":1,
            "application_sha256":"2".repeat(64), "runtime_elf_sha256":"5".repeat(64)
        })).expect("valid private run fixture")];
        let path = directory.path().join(oer_hil_runs::format::read::MANIFEST);
        oer_os_durable::atomic_json(&path, &manifest).expect("valid private run fixture");
        let output = directory.path().join("scenarios/cache/repetition-001");
        let provenance =
            producing_provenance(&output, &"2".repeat(64)).expect("valid private run fixture");
        assert_eq!(provenance.image_commit, "1".repeat(40));
        assert_eq!(provenance.run_id, "cache-run");
        assert!(producing_provenance(&output, &"4".repeat(64)).is_err());
        manifest.firmware[0].repository.dirty = true;
        oer_os_durable::atomic_json(&path, &manifest).expect("valid private run fixture");
        assert!(producing_provenance(&output, &"2".repeat(64)).is_err());
    }

    #[test]
    fn provenance_requires_a_run_and_its_r1_firmware_record() {
        let directory = tempfile::tempdir().expect("valid private run fixture");
        assert!(producing_provenance(directory.path(), &"2".repeat(64)).is_err());
        let mut manifest = oer_hil_runs::run::test_support::manifest();
        manifest.firmware.clear();
        oer_os_durable::atomic_json(
            &directory.path().join(oer_hil_runs::format::read::MANIFEST),
            &manifest,
        )
        .expect("valid private run fixture");
        assert!(producing_provenance(directory.path(), &"2".repeat(64)).is_err());
    }
}
