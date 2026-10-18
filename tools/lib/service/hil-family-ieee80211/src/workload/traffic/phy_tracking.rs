//! The shared PHY's periodic tracking around one traffic session.
//!
//! A suspended arm stops the device's tracking timer before the session and
//! resumes it afterwards; both arms record the tick counts across the
//! session, so traffic with and without tracking compares on one firmware.

use oer_hil_link::SerialCapture;
use oer_hil_protocol::{phy::PhyTrackingCommand, phy::PhyTrackingEvidence};
use serde::Serialize;

use crate::Result;

/// The tracking state when the session began.
pub struct PhyTrackingArm {
    suspended: bool,
    before: PhyTrackingEvidence,
}

#[derive(Serialize)]
struct Report {
    suspended: bool,
    before: PhyTrackingEvidence,
    after: PhyTrackingEvidence,
    tracked_during: u32,
    not_due_during: u32,
    skipped_during: u32,
}

/// Suspend the timer when `suspended`, and record the counts.
pub fn begin(capture: &SerialCapture, suspended: bool) -> Result<PhyTrackingArm> {
    let command = if suspended {
        PhyTrackingCommand::Suspend
    } else {
        PhyTrackingCommand::Status
    };
    let before = capture
        .request(
            0,
            oer_hil_protocol::phy::ControlTracking(command),
            std::time::Duration::from_secs(2),
        )
        .map(|state| state.0)?;
    if before.running == suspended {
        return Err(format!("PHY tracking did not reach the requested state: {before:?}").into());
    }
    Ok(PhyTrackingArm { suspended, before })
}

/// Record the counts across the session, resume a suspended timer and
/// write the report.
pub fn finish(
    capture: &SerialCapture,
    results: &oer_hil_workload::results::Results,
    arm: PhyTrackingArm,
) -> Result<()> {
    let after = capture
        .request(
            0,
            oer_hil_protocol::phy::ControlTracking(if arm.suspended {
                PhyTrackingCommand::Resume
            } else {
                PhyTrackingCommand::Status
            }),
            std::time::Duration::from_secs(2),
        )
        .map(|state| state.0)?;
    let report = Report {
        suspended: arm.suspended,
        before: arm.before,
        after,
        tracked_during: after.tracked.wrapping_sub(arm.before.tracked),
        not_due_during: after.not_due.wrapping_sub(arm.before.not_due),
        skipped_during: after.skipped.wrapping_sub(arm.before.skipped),
    };
    results.observe("phy-tracking", &report);
    // A tick already running when the suspension arrived still finishes.
    if arm.suspended && report.tracked_during > 1 {
        return Err("PHY tracking ran while the experiment suspended it".into());
    }
    Ok(())
}
