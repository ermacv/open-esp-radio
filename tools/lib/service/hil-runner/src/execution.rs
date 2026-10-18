//! Dispatch from validated scenario workloads to their concrete host owners.

use std::path::Path;

use crate::Result;
use crate::scenario::Scenario;
use oer_hil_run_bundle_format::run::Failure;
use oer_hil_run_bundle_format::run::FailureKind;
use oer_hil_run_bundle_format::run::Outcome;

pub(crate) mod doctor;
pub(crate) mod firmware;
pub(crate) mod fixture_check;
pub(crate) mod interrupt_stack;
pub(crate) mod orchestration;
pub(crate) mod preflight;
#[cfg(test)]
mod tests;
pub(crate) use oer_hil_workload::failure::classify;

#[derive(Default)]
pub(crate) struct ExecutionEvidence {
    pub(crate) measurements: Vec<oer_hil_run_bundle_format::run::Measurement>,
    pub(crate) failure: Option<Failure>,
    pub(crate) interrupted: bool,
    /// The stand quarantined the board after this repetition.
    pub(crate) quarantined: bool,
}

impl ExecutionEvidence {
    pub(crate) fn outcome(&self) -> Outcome {
        if self.interrupted {
            return Outcome::Interrupted;
        }
        if self.quarantined {
            return Outcome::BoardQuarantined;
        }
        match self.failure.as_ref().map(|failure| failure.kind) {
            None => Outcome::Passed,
            Some(FailureKind::Infrastructure) => Outcome::Broken,
            Some(_) => Outcome::Failed,
        }
    }
}

pub(crate) fn execute_workload(
    lab: &oer_hil_lab::config::LabConfig,
    selected: &Scenario,
    output: &Path,
    fixtures: &oer_hil_workload::fixture::Fixtures,
    images: Option<&dyn oer_hil_workload::context::BoardImages>,
    device: Option<&oer_device_lock::DeviceAccess>,
) -> Result<ExecutionEvidence> {
    // The board's MAC outlives its port name, which a reset can change.
    let mac = lab.dut.mac.as_str();
    let context = oer_hil_workload::context::Context::new(lab, output)
        .with_profile(selected.header.profile)
        .with_images(images)
        .with_device(device);
    let result = selected.family.run(output, &context, fixtures);
    // The workload's typed observations, whatever its outcome.
    let result = match (result, context.finish()) {
        (Ok(()), written) => written,
        (Err(error), Ok(())) => Err(error),
        (Err(error), Err(written)) => {
            eprintln!("hil: cannot write the repetition's observations: {written}");
            Err(error)
        }
    };
    let elf = runtime_elf(output, selected.image().id());
    // Read only when a failure needs the ELF to name code addresses.
    let elf_bytes = std::cell::LazyCell::new(|| {
        elf.as_deref()
            .and_then(|elf| oer_hil_run_bundle_format::archived::read_archived(elf).ok())
    });
    // A failure may be the target ending: ask it how, without resetting it;
    // a target that does not answer climbs the recovery ladder.
    let failed = result
        .as_ref()
        .err()
        .is_some_and(|error| !oer_process::is_cancelled(&**error));
    let mut post_mortem = failed
        .then(|| {
            device.and_then(|device| {
                oer_hil_lab::post_mortem::inspect(
                    &lab.dut.serial,
                    device,
                    output,
                    elf_bytes.as_deref(),
                )
            })
        })
        .flatten();
    // A target that does not answer is read through its JTAG before any
    // reset erases where it stopped.
    if let Some(device) = device.filter(|_| failed && post_mortem.is_none()) {
        oer_hil_lab::post_mortem::jtag_snapshot_through_stand_openocd(
            lab.chip(),
            device,
            output,
            elf_bytes.as_deref(),
        );
    }
    let recovery = (failed && post_mortem.is_none())
        .then(|| {
            let origin = oer_hil_run_bundle::store::run_of(output).unwrap_or_default();
            let leased = device
                .ok_or_else(|| "the run holds no lease of it".into())
                .and_then(|device| lab.dut_board()?.lease(device));
            match leased {
                Ok(board) => {
                    oer_hil_lab::recovery::recover(&board, output, elf_bytes.as_deref(), &origin)
                }
                Err(error) => {
                    eprintln!("hil: the board {mac} cannot be recovered: {error}");
                    Ok(None)
                }
            }
        })
        .transpose()?
        .flatten();
    if let Some(oer_hil_lab::recovery::Recovery::Recovered { finding, .. }) = &recovery {
        post_mortem = Some((**finding).clone());
    }
    if let Some(oer_hil_lab::recovery::Recovery::BootedSilent { .. }) = &recovery {
        oer_hil_lab::recovery::mark_image_silent(selected.image().id());
    }
    let mut evidence = ExecutionEvidence {
        quarantined: recovery
            .as_ref()
            .is_some_and(oer_hil_lab::recovery::Recovery::quarantined),
        measurements: context.measurements.snapshot(),
        interrupted: result
            .as_ref()
            .err()
            .is_some_and(|error| oer_process::is_cancelled(&**error)),
        failure: result.err().map(|error| classify(&*error)),
    };
    check_interrupt_stacks(lab.chip(), elf.as_deref(), &mut evidence);
    if let Some(finding) = post_mortem {
        let _ = oer_durable::atomic_json(
            &output.join("post-mortem/post-mortem.json"),
            &serde_json::json!({
                "schema": 1,
                "boot": finding.boot,
                "checkpoints": finding.checkpoints,
                "failure": finding.failure,
                "workload_failure": evidence.failure,
            }),
        );
        if let Some(failure) = finding.failure {
            // The workload's own error stays in the recorded report above.
            evidence.failure = Some(failure);
        }
    }
    if let Some(recovery) = &recovery {
        let failure = evidence.failure.get_or_insert_with(|| {
            Failure::new(FailureKind::Infrastructure, "the target stopped answering")
        });
        failure.message = format!("{}; {}", failure.message, recovery.describe());
    }
    if oer_process::cancellation_requested() {
        evidence.interrupted = true;
        evidence.failure.get_or_insert_with(|| {
            Failure::new(FailureKind::Infrastructure, "run cancelled by signal")
        });
    }
    Ok(evidence)
}

/// Hold each hart's observed interrupt-stack use of a repetition to the
/// image's static bound: a watermark above it fails the repetition, as does
/// an analysis that cannot run; a `partial + ?` hart is only observed. A
/// chip whose profile names no interrupt contract has no bound to hold.
fn check_interrupt_stacks(chip: &str, elf: Option<&Path>, evidence: &mut ExecutionEvidence) {
    let bounded = oer_chip_profile::Profile::load(&oer_process::built_root(), chip)
        .is_ok_and(|profile| profile.interrupts.is_some());
    if !bounded {
        return;
    }
    let peaks = interrupt_stack::observed_peaks(&evidence.measurements);
    if peaks.is_empty() {
        return;
    }
    let evaluated = elf
        .ok_or_else(|| "the run archived no runtime ELF to bound the interrupt stacks".to_owned())
        .and_then(|elf| interrupt_stack::bounds(chip, elf))
        .and_then(|bounds| interrupt_stack::evaluate(&peaks, &bounds));
    match evaluated {
        Ok(measurements) => {
            let exceeded: Vec<String> = measurements
                .iter()
                .filter(|measurement| {
                    measurement.verdict
                        == Some(oer_hil_run_bundle_format::run::MeasurementVerdict::Failed)
                })
                .map(|measurement| {
                    format!(
                        "{} {} > static bound {}",
                        measurement.name,
                        measurement.value,
                        measurement.threshold.map_or(0, |threshold| threshold.value)
                    )
                })
                .collect();
            evidence.measurements.extend(measurements);
            if !exceeded.is_empty() {
                evidence.failure.get_or_insert_with(|| {
                    Failure::new(
                        FailureKind::Scenario,
                        format!(
                            "observed interrupt-stack use exceeds the static bound: {}",
                            exceeded.join(", ")
                        ),
                    )
                });
            }
        }
        Err(error) => {
            evidence
                .failure
                .get_or_insert_with(|| Failure::new(FailureKind::Infrastructure, error));
        }
    }
}

/// The runtime ELF the run archived for `image`, found from a repetition's
/// output directory inside the run bundle; its bytes are read through
/// `oer_hil_run_bundle_format::archived::read_archived`.
fn runtime_elf(output: &Path, image: &str) -> Option<std::path::PathBuf> {
    output
        .ancestors()
        .map(|directory| directory.join("firmware").join(image).join("runtime.elf"))
        .find(|elf| oer_hil_run_bundle_format::archived::stored_path(elf).is_file())
}
