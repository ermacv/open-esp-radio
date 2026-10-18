//! Shared PHY diagnostic control, independent of the selected radio protocol.
use postcard_schema::Schema;
use serde::{Deserialize, Serialize};
/// Destructive injection into real PHY maintenance, not the idle SoC test.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum PhyFaultMode {
    BlockedPoll,
    LostCompletion,
    Restoration,
    Cancelled,
}

/// A one-shot attempt cannot be disarmed or replaced before reboot.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum PhyFaultCommand {
    Arm(PhyFaultMode),
    Status,
    Release,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum PhyFaultPhase {
    Idle,
    Armed,
    Reached,
    Released,
    Cancelled,
}

/// Reset cause is reported independently of the fault's checkpoint phase.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct PhyFaultEvidence {
    pub phase: PhyFaultPhase,
    pub reset_reason: crate::base::ResetReason,
}

/// Control of the shared PHY's periodic tracking timer for one experiment.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub enum PhyTrackingCommand {
    /// Run the periodic tracking timer (the boot default).
    Resume,
    /// Stop the timer between ticks; a started tick finishes first.
    Suspend,
    /// Report the state and the tick counts.
    Status,
}

/// The tracking timer's state and its tick results since boot.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize, Schema)]
pub struct PhyTrackingEvidence {
    /// Whether the periodic timer runs.
    pub running: bool,
    /// Ticks that ran due tracking to completion.
    pub tracked: u32,
    /// Ticks with no tracking due.
    pub not_due: u32,
    /// Ticks the domain could not serve (not registered, RF closed, or
    /// poisoned).
    pub skipped: u32,
}
