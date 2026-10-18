//! Run selection and attached-device preflight checks.

use std::{path::Path, time::Duration};

use oer_tool_service_hil_scenario::ScenarioFamily as _;

use crate::{Result, fixture, scenario::Scenario};
use oer_tool_data_hil_protocol::DeviceImageKeys;
use oer_tool_data_hil_run_bundle_format::run::Failure;
use oer_tool_data_hil_schema::image::{FeatureDelta, ImageClass};
use oer_tool_service_hil_lab::config::LabConfig;
use oer_tool_service_hil_link::SerialCapture;

pub(crate) fn scenario_failure(lab: &LabConfig, selected: &Scenario) -> Option<Failure> {
    fixture::preflight::scenario_precondition(lab, selected).or_else(|| {
        fixture::preflight::check(lab, selected)
            .err()
            .map(|error| super::classify(&*error))
    })
}

pub(crate) fn validate_flashed_image(
    lab: &LabConfig,
    selected: &Scenario,
    features: &FeatureDelta,
    output: &Path,
    device: Option<&oer_tool_service_device_lock::DeviceAccess>,
) -> Result<()> {
    if selected.image() == ImageClass::BootSmoke {
        return Ok(());
    }
    let preflight = output.join("image-preflight");
    let error = match image_keys_of(lab, &preflight) {
        Ok(image_keys) => {
            return check_flashed_image_keys(lab.chip(), selected, features, &image_keys);
        }
        Err(error) => error,
    };
    // A bootloader that resets in a loop keeps state an RTS reset and a
    // reflash leave: climb to the resets that clear it.
    let console = std::fs::read_to_string(preflight.join("uart.log")).unwrap_or_default();
    let Some(found) = oer_tool_service_hil_lab::recovery::boot_loop(&console) else {
        return Err(error);
    };
    eprintln!(
        "hil: the bootloader resets in a loop ({}); escalating the reset",
        found.reset_line
    );
    let origin = oer_tool_service_hil_run_bundle::store::run_of(output).unwrap_or_default();
    let mut attempt = 0;
    let mut answered = None;
    let board = lab
        .dut_board()?
        .lease(device.ok_or("the run holds no lease of its device under test")?)?;
    let escalation = oer_tool_service_hil_lab::recovery::escalate_boot_loop(
        &board,
        found,
        output,
        &origin,
        || {
            attempt += 1;
            let directory = output.join(format!("image-preflight-retry-{attempt}"));
            answered = image_keys_of(lab, &directory).ok();
            answered.is_some()
        },
    )?;
    match answered {
        Some(image_keys) => {
            eprintln!(
                "hil: {:?} cleared the boot loop",
                escalation.ladder.steps.last().map(|step| step.step)
            );
            check_flashed_image_keys(lab.chip(), selected, features, &image_keys)
        }
        None => Err(format!(
            "{error}; the bootloader reset in a loop ({}) and {} did not clear it: {} (see {})",
            escalation.boot_loop.reset_line,
            escalation
                .ladder
                .steps
                .iter()
                .map(|step| format!("{:?}", step.step))
                .collect::<Vec<_>>()
                .join(", "),
            match escalation.ladder.end {
                oer_tool_service_devices::reset::LadderEnd::Loadable { .. } => {
                    "its ROM answers, so firmware can be loaded again"
                }
                _ => "the board is quarantined for a person",
            },
            oer_tool_service_hil_lab::recovery::RESET_ESCALATION_FILE
        )
        .into()),
    }
}

/// How the host recognizes an image of a class once it boots.
#[derive(Debug, Eq, PartialEq)]
enum Recognition {
    /// By the image keys its protocol reports.
    ImageKeys,
    /// The boot smoke image speaks no protocol: by its console's pass line.
    BootSmokeLine,
}

fn recognition(class: ImageClass) -> Recognition {
    if class == ImageClass::BootSmoke {
        Recognition::BootSmokeLine
    } else {
        Recognition::ImageKeys
    }
}

/// Whether the device, reset, answers as an image of `class`; why not
/// otherwise.
pub(crate) fn answers_as(
    lab: &LabConfig,
    class: ImageClass,
    directory: &Path,
) -> std::result::Result<(), String> {
    if recognition(class) == Recognition::BootSmokeLine {
        return SerialCapture::start_with_reset(lab, directory)
            .and_then(|capture| {
                let passed = capture.wait_for_boot_smoke(Duration::from_secs(10));
                capture.finish_with(passed)
            })
            .map_err(|error| error.to_string());
    }
    let image_keys = image_keys_of(lab, directory).map_err(|error| error.to_string())?;
    match oer_tool_data_hil_image_class::classify_flashed(lab.chip(), &image_keys) {
        Some(found) if found == class => Ok(()),
        Some(found) => Err(format!("it answers as {} instead", found.id())),
        None => Err(String::from("its image keys name no image class")),
    }
}

/// Reset the device and ask its image for its image keys, capturing the
/// console into `directory`.
fn image_keys_of(lab: &LabConfig, directory: &Path) -> Result<DeviceImageKeys> {
    let capture = SerialCapture::start_with_reset(lab, directory)?;
    let image_keys = capture.request_image_keys(Duration::from_secs(10));
    capture.finish_with(image_keys)
}

/// Accept a flashed image only when it is the scenario's class, built with
/// the run's experiment `features` delta when it has one, and reports every
/// role the scenario drives. An experiment's image is not its class's: it
/// reports the keys of the class's features with the delta applied.
fn check_flashed_image_keys(
    chip: &str,
    selected: &Scenario,
    features: &FeatureDelta,
    image_keys: &DeviceImageKeys,
) -> Result<()> {
    let expected = selected.image();
    let observed = if features.is_empty() {
        oer_tool_data_hil_image_class::classify_flashed(chip, image_keys).ok_or_else(|| {
            format!("the flashed image reports image keys no {chip} image class builds")
        })?
    } else {
        let keys = oer_tool_data_hil_image_class::image_keys_with(expected, chip, features)
            .ok_or_else(|| format!("{chip} builds no `{}` image", expected.id()))?;
        if !keys.same_keys(image_keys) {
            return Err(format!(
                "the flashed image does not report the image keys of `{}` with features `{features}`",
                expected.id()
            )
            .into());
        }
        expected
    };
    if observed != expected {
        return Err(format!(
            "scenario requires `{}` image but flashed target advertises the image keys of `{}`",
            expected.id(),
            observed.id()
        )
        .into());
    }
    if !selected.family.served_by(image_keys) {
        return Err(format!(
            "flashed `{}` image does not declare a role scenario `{}` drives",
            observed.id(),
            selected.id()
        )
        .into());
    }
    Ok(())
}

#[cfg(test)]
mod tests;
