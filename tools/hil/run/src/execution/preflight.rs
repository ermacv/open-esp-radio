//! Run selection and attached-device preflight checks.

use std::{path::Path, time::Duration};

use oer_hil_scenario::ScenarioFamily as _;

use crate::{Result, fixture, scenario::Scenario};
use oer_hil_lab::config::LabConfig;
use oer_hil_link::SerialCapture;
use oer_hil_model::schema::image::ImageClass;
use oer_hil_protocol::DeviceImageKeys;
use oer_hil_runs::format::run::Failure;

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
    output: &Path,
    device: Option<&oer_lab_board::lock::DeviceAccess>,
) -> Result<()> {
    if selected.image() == ImageClass::BootSmoke {
        return Ok(());
    }
    let preflight = output.join("image-preflight");
    let error = match image_keys_of(lab, &preflight) {
        Ok(image_keys) => {
            return check_flashed_image_keys(selected, &image_keys);
        }
        Err(error) => error,
    };
    // A bootloader that resets in a loop keeps state an RTS reset and a
    // reflash leave: climb to the resets that clear it.
    let console = std::fs::read_to_string(preflight.join("uart.log")).unwrap_or_default();
    let Some(found) = oer_hil_lab::recovery::boot_loop(&console) else {
        return Err(error);
    };
    eprintln!(
        "hil: the bootloader resets in a loop ({}); escalating the reset",
        found.reset_line
    );
    let origin = oer_hil_runs::store::run_of(output).unwrap_or_default();
    let mut attempt = 0;
    let mut answered = None;
    let board = lab
        .dut_board()?
        .lease(device.ok_or("the run holds no lease of its device under test")?)?;
    let escalation =
        oer_hil_lab::recovery::escalate_boot_loop(&board, found, output, &origin, || {
            attempt += 1;
            let directory = output.join(format!("image-preflight-retry-{attempt}"));
            answered = image_keys_of(lab, &directory).ok();
            answered.is_some()
        })?;
    match answered {
        Some(image_keys) => {
            eprintln!(
                "hil: {:?} cleared the boot loop",
                escalation.ladder.steps.last().map(|step| step.step)
            );
            check_flashed_image_keys(selected, &image_keys)
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
                oer_lab_board::reset::LadderEnd::Loadable { .. } => {
                    "its ROM answers, so firmware can be loaded again"
                }
                _ => "the board is quarantined for a person",
            },
            oer_hil_lab::recovery::RESET_ESCALATION_FILE
        )
        .into()),
    }
}

/// Whether the device, reset, answers as the boot smoke recovery image: by
/// its console's pass line, as it speaks no protocol; why not otherwise.
pub(crate) fn answers_as_boot_smoke(
    lab: &LabConfig,
    directory: &Path,
) -> std::result::Result<(), String> {
    SerialCapture::start_with_reset(lab, directory)
        .and_then(|capture| {
            let passed = capture.wait_for_boot_smoke(Duration::from_secs(10));
            capture.finish_with(passed)
        })
        .map_err(|error| error.to_string())
}

/// Reset the device and ask its image for its image keys, capturing the
/// console into `directory`.
fn image_keys_of(lab: &LabConfig, directory: &Path) -> Result<DeviceImageKeys> {
    let capture = SerialCapture::start_with_reset(lab, directory)?;
    let image_keys = capture.request_image_keys(Duration::from_secs(10));
    capture.finish_with(image_keys)
}

/// Accept a flashed image only when it reports every required key and no
/// forbidden key of the scenario's family. What the image serves is its own
/// report; the host derives no expected set from the class's features, so an experiment's
/// image with a feature delta passes when it still serves the scenario.
fn check_flashed_image_keys(selected: &Scenario, image_keys: &DeviceImageKeys) -> Result<()> {
    image_keys
        .require_all(selected.family.required_keys())
        .and_then(|()| image_keys.forbid_all(selected.family.forbidden_keys()))
        .map_err(|mismatch| {
            format!(
                "flashed `{}` image does not serve scenario `{}`: {mismatch}",
                selected.image().id(),
                selected.id()
            )
            .into()
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_hil_protocol::{Key, Message, base, system};

    fn scenario(kind: &str) -> Scenario {
        Scenario::from_toml(
            &format!(
                "schema = 5\nid = \"t\"\ndescription = \"t\"\nrole = \"investigation\"\n\
                 repetitions = 1\n\n[system]\nkind = \"{kind}\"\n"
            ),
            Path::new("t.toml"),
        )
        .unwrap()
    }

    fn image(keys: &[Key]) -> DeviceImageKeys {
        DeviceImageKeys::of_keys(keys.iter().copied().chain([base::Hello::KEY]))
    }

    #[test]
    fn an_image_serving_the_family_s_keys_is_accepted_with_further_services() {
        let watchdog = scenario("watchdog");
        let keys = [
            system::WatchdogTest::KEY,
            system::WatchdogArmed::KEY,
            system::PanicHook::KEY,
            Key([7; 8]),
        ];
        check_flashed_image_keys(&watchdog, &image(&keys)).unwrap();
    }

    #[test]
    fn every_image_class_selects_only_its_system_workload() {
        use oer_hil_protocol::{health, lower_mac, phy, wifi};
        let panic = [system::InjectPanic::KEY, system::PanicInjected::KEY];
        let watchdog: Vec<Key> = panic
            .iter()
            .copied()
            .chain([
                system::WatchdogTest::KEY,
                system::WatchdogArmed::KEY,
                system::PanicHook::KEY,
            ])
            .collect();
        let usb: Vec<Key> = watchdog
            .iter()
            .copied()
            .chain([
                system::DisableUsb::KEY,
                system::UsbDisabled::KEY,
                system::UsbJtagOff::KEY,
            ])
            .collect();
        for class in ImageClass::ALL {
            let (keys, expected): (&[Key], Option<&str>) = match class {
                ImageClass::SystemWatchdog => (&watchdog, Some("watchdog")),
                ImageClass::SystemPanicReset => (&panic, Some("panic-reset")),
                ImageClass::DiagnosticUsbJtagOff => (&usb, Some("stand-recovery")),
                ImageClass::BootSmoke => (&[], None),
                ImageClass::WifiStation => (wifi::KEYS, None),
                ImageClass::DiagnosticPhy => (phy::KEYS, None),
                ImageClass::DiagnosticRadioHealth => (health::KEYS, None),
                ImageClass::DiagnosticLowerMac => (lower_mac::KEYS, None),
            };
            let accepted: Vec<&str> = ["watchdog", "panic-reset", "stand-recovery"]
                .into_iter()
                .filter(|kind| check_flashed_image_keys(&scenario(kind), &image(keys)).is_ok())
                .collect();
            assert_eq!(
                accepted,
                expected.into_iter().collect::<Vec<_>>(),
                "{class:?}"
            );
        }
    }

    #[test]
    fn properties_are_checked_independently_of_endpoints() {
        for (kind, keys, path) in [
            (
                "watchdog",
                vec![system::WatchdogTest::KEY, system::WatchdogArmed::KEY],
                system::PanicHook::PATH,
            ),
            (
                "panic-reset",
                vec![
                    system::InjectPanic::KEY,
                    system::PanicInjected::KEY,
                    system::PanicHook::KEY,
                ],
                system::PanicHook::PATH,
            ),
            (
                "watchdog",
                vec![
                    system::WatchdogTest::KEY,
                    system::WatchdogArmed::KEY,
                    system::PanicHook::KEY,
                    system::UsbJtagOff::KEY,
                ],
                system::UsbJtagOff::PATH,
            ),
            (
                "panic-reset",
                vec![
                    system::InjectPanic::KEY,
                    system::PanicInjected::KEY,
                    system::UsbJtagOff::KEY,
                ],
                system::UsbJtagOff::PATH,
            ),
            (
                "stand-recovery",
                vec![system::DisableUsb::KEY, system::UsbDisabled::KEY],
                system::UsbJtagOff::PATH,
            ),
        ] {
            let error = check_flashed_image_keys(&scenario(kind), &image(&keys))
                .unwrap_err()
                .to_string();
            assert!(error.contains(path), "{kind}: {error}");
        }
    }

    #[test]
    fn an_image_lacking_a_required_key_is_refused_by_its_path() {
        let recovery = scenario("stand-recovery");
        let keys = [system::WatchdogTest::KEY, system::WatchdogArmed::KEY];
        let error = check_flashed_image_keys(&recovery, &image(&keys))
            .unwrap_err()
            .to_string();
        assert!(error.contains(system::DisableUsb::PATH), "{error}");
        assert!(error.contains("diagnostic-usb-jtag-off"), "{error}");
    }
}
