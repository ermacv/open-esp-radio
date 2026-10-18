//! The per-boot loop of a workload: a fresh boot of the board under test
//! per step, each boot's typed evidence an observation.

use std::path::Path;

use oer_hil_link::SerialCapture;
use serde::Serialize;

use crate::{Result, context::Context};

/// The directory of boot `boot` below `output`: `boot-NNN`.
pub fn boot_directory(output: &Path, boot: u8) -> std::path::PathBuf {
    output.join(format!("boot-{boot:03}"))
}

/// Run `observe` on `boots` fresh boots of the board under test, each in
/// its own capture in `boot-NNN` below `output`; record what each boot
/// returns as the observation `boot-NNN`, then let `accept` judge it. The
/// first failure, of a boot or of its acceptance, ends the loop; the
/// observations recorded before it stay in the repetition's results.
pub fn for_each_boot<T: Serialize>(
    context: &Context<'_>,
    output: &Path,
    boots: u8,
    mut observe: impl FnMut(u8, &SerialCapture) -> Result<T>,
    mut accept: impl FnMut(u8, &T) -> Result<()>,
) -> Result<Vec<T>> {
    std::fs::create_dir_all(output)?;
    let mut observed = Vec::with_capacity(usize::from(boots));
    for boot in 1..=boots {
        let directory = boot_directory(output, boot);
        let value = context.with_capture(&directory, |capture| observe(boot, capture))?;
        context.results.observe(format!("boot-{boot:03}"), &value);
        accept(boot, &value)?;
        observed.push(value);
    }
    Ok(observed)
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, time::Duration};

    use oer_hil_link::test_support::{Output, capture_with_commands_at, healthy_target};

    use super::*;

    #[test]
    fn each_boot_is_observed_until_the_first_rejection() {
        let output = Output::new();
        let lab = oer_hil_lab::config::LabConfig::for_test();
        let targets = RefCell::new(Vec::new());
        let context = Context::new(&lab, &output.0).with_opener(|path| {
            let (capture, input, commands) = capture_with_commands_at(path);
            targets
                .borrow_mut()
                .push(healthy_target(input, commands, 7));
            Ok(capture)
        });
        let workload = output.0.join("workload");
        let error = for_each_boot(
            &context,
            &workload,
            3,
            |boot, capture| {
                capture
                    .wait_for_message_after(0, Duration::from_secs(2), |_| true)?
                    .ok_or("no hello")?;
                Ok(u32::from(boot) * 10)
            },
            |boot, value| {
                if boot == 2 {
                    Err(format!("boot {boot} observed {value}").into())
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert_eq!(error.to_string(), "boot 2 observed 20");
        assert!(boot_directory(&workload, 2).join("uart.bin").is_file());
        assert!(!boot_directory(&workload, 3).exists());
        context.finish().unwrap();
        let written: oer_hil_run_bundle_format::run::Observations = serde_json::from_slice(
            &std::fs::read(
                output
                    .0
                    .join(oer_hil_run_bundle_format::run::OBSERVATIONS_FILE),
            )
            .unwrap(),
        )
        .unwrap();
        let observed: Vec<_> = written
            .observations
            .iter()
            .map(|observation| (observation.name.as_str(), observation.value.clone()))
            .collect();
        assert_eq!(
            observed,
            [
                ("boot-001", serde_json::json!(10)),
                ("boot-002", serde_json::json!(20))
            ]
        );
        assert!(
            context
                .measurements
                .snapshot()
                .iter()
                .any(|measurement| measurement.name == "target.workload.boot-002.capture.events")
        );
    }
}
