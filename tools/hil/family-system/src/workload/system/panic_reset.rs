//! A host-requested reset through the staged runtime's retained record.
//! The next boot must report `HilRequested` with the acknowledged request's
//! identity; neither a panic record nor a watchdog reset satisfies it.
use crate::Result;
use oer_hil_protocol::{
    base::{BootEvidence, GetBootStatus, ResetReason},
    system::{InjectPanic, PanicInjected},
};
use oer_hil_scenario::workload::{context::Context, require_keys};
use std::{path::Path, time::Duration};

pub fn run(output: &Path, context: &Context<'_>) -> Result<()> {
    context.with_capture(output, |capture| {
        require_keys::<InjectPanic>(capture)?;
        // The reset follows the acknowledgement at once; the next Hello
        // comes after the ROM, the bootloader and both boot stages.
        capture.expect_reboot(Duration::from_millis(100), Duration::from_secs(15))?;
        let mark = capture.console_length();
        let (reply, response) = capture.call_answered(0, InjectPanic, Duration::from_secs(5))?;
        let PanicInjected = response
            .map_err(|reason| format!("device rejected the requested reset: {reason:?}"))?;
        let report = retained_report(reply.request_id, reply.message_sequence, reply.session_id);
        let reboot = capture.wait_expected_reboot()?;
        let fresh = capture.request(0, GetBootStatus, Duration::from_secs(5))?;
        if !capture.console_shows_boot_since(mark) {
            return Err("the console shows no ROM or bootloader start after the reset".into());
        }
        recorded_request(&fresh, capture.console_shows_since(mark, &report))?;
        context.results.observe(
            "requested-reset",
            &serde_json::json!({
                "reboot": reboot, "fresh_boot": fresh, "boot_console_after_reset": true,
                "retained_report": report,
            }),
        );
        Ok(())
    })
}

/// The existing staged-runtime slot reports its four observation words in
/// hexadecimal: request ID, acknowledgement sequence and session ID low/high.
fn retained_report(request: u32, sequence: u32, session: u64) -> String {
    format!(
        "previous fault HilRequested: {request:08x} {sequence:08x} {:08x} {:08x}",
        session as u32,
        (session >> 32) as u32,
    )
}

fn recorded_request(boot: &BootEvidence, report_seen: bool) -> Result<()> {
    if boot.reset_reason != ResetReason::Software {
        return Err(format!(
            "expected the host-requested software reset, not {:?} (0x{:02x})",
            boot.reset_reason, boot.raw_reset_reason
        )
        .into());
    }
    if !report_seen {
        return Err(
            "the next boot did not report HilRequested for the acknowledged request".into(),
        );
    }
    if boot.platform_panic.is_some() {
        return Err("the requested reset left an unexpected platform panic record".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use oer_hil_protocol::base::PlatformPanic;

    fn boot(reset_reason: ResetReason) -> BootEvidence {
        BootEvidence {
            reset_reason,
            raw_reset_reason: 0,
            post_mortem: None,
            platform_panic: None,
        }
    }

    #[test]
    fn requested_reset_requires_its_retained_report_and_software_reset() {
        assert!(recorded_request(&boot(ResetReason::Software), true).is_ok());
        assert!(recorded_request(&boot(ResetReason::Software), false).is_err());
        for cause in [ResetReason::MainWatchdog1, ResetReason::PowerOn] {
            assert!(recorded_request(&boot(cause), true).is_err());
        }
        let mut panic = boot(ResetReason::Software);
        panic.platform_panic = Some(PlatformPanic {
            hart: 0,
            in_interrupt: false,
            interrupted_pc: None,
            file: "system/panic.rs".try_into().unwrap(),
            line: 37,
            column: 5,
        });
        assert!(recorded_request(&panic, true).is_err());
    }

    #[test]
    fn retained_report_correlates_the_request_acknowledgement_and_session() {
        assert_eq!(
            retained_report(0x1234, u32::MAX, 0x89ab_cdef_0123_4567),
            "previous fault HilRequested: 00001234 ffffffff 01234567 89abcdef",
        );
        let expected = retained_report(7, 9, 0);
        for other in [
            retained_report(8, 9, 0),
            retained_report(7, 10, 0),
            retained_report(7, 9, 1),
            retained_report(7, 9, 1 << 32),
            "previous fault HilConsole: 00000007 00000009 00000000 00000000".to_owned(),
        ] {
            assert!(!other.contains(&expected));
        }
    }
}
