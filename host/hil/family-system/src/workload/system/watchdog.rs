//! SoC reset evidence without a radio peer; distinct from DTM RF evidence.
//! The image's HIL panic hook is checked here too: an intentional panic
//! leaves its post-mortem fault and, on the ESP32-S31, the fatal machine
//! state for the next boot.
use crate::Result;
use oer_hil_protocol::{
    base::{BootEvidence, Fault, GetBootStatus, PanicFault, ResetReason},
    system::{InjectPanic, PanicInjected, WatchdogArmed, WatchdogTest, WatchdogTestMode as Mode},
};
use oer_hil_scenario::workload::{context::Context, require_keys};
use std::{path::Path, time::Duration};

pub fn run(output: &Path, context: &Context<'_>) -> Result<()> {
    context.results.claim(
        "soc-watchdog-reset-without-a-radio",
        &["phy-restoration-fault-injection", "rf-stop-bound"],
    );
    for mode in [
        Mode::Complete,
        Mode::BlockedPoll,
        Mode::Cancelled,
        Mode::LostCompletion,
        Mode::LateRestoration,
    ] {
        let directory = output.join(scope(mode));
        std::fs::create_dir_all(&directory)?;
        context.with_capture(&directory, |capture| {
            require_keys::<WatchdogTest>(capture)?;
            if mode != Mode::Complete {
                capture.expect_reboot(Duration::from_millis(500), Duration::from_secs(12))?;
            }
            let WatchdogArmed(armed) =
                capture.request(0, WatchdogTest(mode), Duration::from_secs(5))?;
            if armed != mode {
                return Err(format!("watchdog {mode:?} armed as {armed:?}").into());
            }
            let reboot = if mode == Mode::Complete {
                oer_os_process::sleep(Duration::from_secs(3))?;
                capture.request_image_keys(Duration::from_secs(5))?;
                None
            } else {
                Some(capture.wait_expected_reboot()?)
            };
            let fresh = capture.request(0, GetBootStatus, Duration::from_secs(5))?;
            if reboot.is_some() && fresh.reset_reason != ResetReason::MainWatchdog1 {
                return Err("expected autonomous MWDT1 reset, not software reset".into());
            }
            // The boot the watchdog reset left its record in retained memory:
            // its newest checkpoint is the arming this scenario requested.
            let checkpoints = match (&reboot, &fresh.post_mortem) {
                (None, _) => Vec::new(),
                (Some(_), None) => {
                    return Err("no post-mortem survived the MWDT1 reset".into());
                }
                (Some(_), Some(summary)) => capture.post_mortem_checkpoints(summary.checkpoints)?,
            };
            if reboot.is_some()
                && checkpoints
                    .last()
                    .is_none_or(|last| last.name != "watchdog.arm" || last.arg != mode as u32)
            {
                return Err(format!(
                    "the post-mortem after the MWDT1 reset does not end at this arming: {checkpoints:?}"
                )
                .into());
            }
            context.results.observe(
                scope(mode),
                &serde_json::json!({
                    "mode": mode, "budget_micros": 1_000_000,
                    "reboot": reboot, "fresh_boot": fresh, "post_mortem_checkpoints": checkpoints,
                }),
            );
            Ok(())
        })?;
    }
    panic_hook(output, context)
}

/// The agent source that raises the panic, as its record ends.
const PANIC_FILE: &str = "system/panic.rs";
const PANIC_MESSAGE: &str = "intentional HIL panic";
/// The fatal slot's report of the previous boot's panic on the ESP32-S31,
/// which prints it through the ROM before its protocol starts.
const FATAL_PANIC_LINE: &str = "OPEN_RADIO_HIL runtime=PANIC boot=previous hart=0 stage=0 ";

/// An intentional panic through the image's HIL panic hook: after the
/// software reset, the boot reports the post-mortem fault it recorded.
fn panic_hook(output: &Path, context: &Context<'_>) -> Result<()> {
    let fatal_slot = context.dut_board()?.chip() == "esp32s31";
    let directory = output.join("panic-hook");
    std::fs::create_dir_all(&directory)?;
    context.with_capture(&directory, |capture| {
        require_keys::<InjectPanic>(capture)?;
        capture.expect_reboot(Duration::from_millis(100), Duration::from_secs(15))?;
        let mark = capture.console_length();
        let PanicInjected = capture.request(0, InjectPanic, Duration::from_secs(5))?;
        let reboot = capture.wait_expected_reboot()?;
        let fresh = capture.request(0, GetBootStatus, Duration::from_secs(5))?;
        let fault = recorded_panic(&fresh)?;
        if fatal_slot && !capture.console_shows_since(mark, FATAL_PANIC_LINE) {
            return Err(
                "the console shows no fatal-slot report of the panic after the reset".into(),
            );
        }
        context.results.observe(
            "panic-hook",
            &serde_json::json!({
                "reboot": reboot, "fresh_boot": fresh, "fault": fault,
                "fatal_slot_report": fatal_slot,
            }),
        );
        Ok(())
    })
}

/// The injected panic as the post-mortem fault of `boot`: a software reset,
/// and the fault names the panic's source and message.
fn recorded_panic(boot: &BootEvidence) -> Result<&PanicFault> {
    if boot.reset_reason != ResetReason::Software {
        return Err(format!(
            "expected the panic entry's software reset, not {:?} (0x{:02x})",
            boot.reset_reason, boot.raw_reset_reason
        )
        .into());
    }
    match boot
        .post_mortem
        .as_ref()
        .and_then(|summary| summary.fault.as_ref())
    {
        Some(Fault::Panic(panic))
            if panic.file.ends_with(PANIC_FILE)
                && panic.line != 0
                && panic.message == PANIC_MESSAGE =>
        {
            Ok(panic)
        }
        _ => Err(format!(
            "the post-mortem does not hold the injected panic: {:?}",
            boot.post_mortem
        )
        .into()),
    }
}

fn scope(mode: Mode) -> &'static str {
    match mode {
        Mode::Complete => "complete",
        Mode::BlockedPoll => "blocked-poll",
        Mode::Cancelled => "cancelled",
        Mode::LostCompletion => "lost-completion",
        Mode::LateRestoration => "late-restoration",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_hil_protocol::base::PostMortemSummary;

    fn boot(reset_reason: ResetReason, fault: Option<Fault>) -> BootEvidence {
        BootEvidence {
            reset_reason,
            post_mortem: Some(PostMortemSummary {
                boot_count: 2,
                checkpoints: 0,
                fault,
            }),
            raw_reset_reason: 0,
            platform_panic: None,
        }
    }

    fn panic(file: &str, line: u32, message: &str) -> Option<Fault> {
        Some(Fault::Panic(PanicFault {
            file: file.try_into().unwrap(),
            line,
            message: message.try_into().unwrap(),
        }))
    }

    #[test]
    fn only_the_injected_panic_after_a_software_reset_is_recorded() {
        let injected = "agent/src/system/panic.rs";
        assert!(
            recorded_panic(&boot(
                ResetReason::Software,
                panic(injected, 38, PANIC_MESSAGE)
            ))
            .is_ok()
        );
        for wrong in [
            boot(
                ResetReason::MainWatchdog1,
                panic(injected, 38, PANIC_MESSAGE),
            ),
            boot(
                ResetReason::Software,
                panic("src/main.rs", 38, PANIC_MESSAGE),
            ),
            boot(ResetReason::Software, panic(injected, 0, PANIC_MESSAGE)),
            boot(ResetReason::Software, panic(injected, 38, "")),
            boot(ResetReason::Software, None),
        ] {
            assert!(recorded_panic(&wrong).is_err(), "{wrong:?}");
        }
    }
    #[test]
    fn capture_scopes_are_valid_and_distinct() {
        let modes = [
            Mode::Complete,
            Mode::BlockedPoll,
            Mode::Cancelled,
            Mode::LostCompletion,
            Mode::LateRestoration,
        ];
        let names: std::collections::BTreeSet<_> = modes.map(scope).into_iter().collect();
        assert_eq!(names.len(), modes.len());
        assert!(
            names
                .iter()
                .all(|s| s.bytes().all(|b| b.is_ascii_lowercase() || b == b'-'))
        );
    }
}
