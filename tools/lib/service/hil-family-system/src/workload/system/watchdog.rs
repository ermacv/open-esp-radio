//! SoC reset evidence without a radio peer; distinct from DTM RF evidence.
use crate::Result;
use oer_hil_protocol::{
    base::{GetBootStatus, ResetReason},
    system::{WatchdogArmed, WatchdogTest, WatchdogTestMode as Mode},
};
use oer_hil_workload::{context::Context, require_keys};
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
                oer_process::sleep(Duration::from_secs(3))?;
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
    Ok(())
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
