//! The vocabulary of a sealed HIL run: its state, outcomes, failure kinds and
//! gated measurements.
//!
//! The runner writes these values into run bundles; every reader (repository
//! tools, retention, the evaluator) parses them into these types instead of
//! comparing strings, so a renamed or new value fails to parse rather than
//! silently falling through a string comparison.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunState {
    Running,
    Completed,
    Interrupted,
}

impl RunState {
    /// The serialized name.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Interrupted => "interrupted",
        }
    }

    /// A completed or interrupted bundle is sealed and can no longer change.
    pub const fn is_sealed(self) -> bool {
        !matches!(self, Self::Running)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    Passed,
    Failed,
    Broken,
    Skipped,
    Blocked,
    Interrupted,
    /// The board stayed unreachable or needed hardware recovery too often,
    /// and the stand quarantined it; not a verdict on the code under test.
    BoardQuarantined,
}

impl Outcome {
    /// The serialized name.
    pub const fn id(self) -> &'static str {
        match self {
            Self::Passed => "passed",
            Self::Failed => "failed",
            Self::Broken => "broken",
            Self::Skipped => "skipped",
            Self::Blocked => "blocked",
            Self::Interrupted => "interrupted",
            Self::BoardQuarantined => "board-quarantined",
        }
    }

    pub const fn is_passed(self) -> bool {
        matches!(self, Self::Passed)
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum FailureKind {
    Scenario,
    Precondition,
    ImageBuild,
    ImageFlash,
    Infrastructure,
    /// The target's hang watchdog found a stalled executor and reset it.
    Hang,
    /// The target reset for a reason the runner did not cause.
    UnexpectedReset,
}

impl FailureKind {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Scenario => "scenario",
            Self::Precondition => "precondition",
            Self::ImageBuild => "image-build",
            Self::ImageFlash => "image-flash",
            Self::Infrastructure => "infrastructure",
            Self::Hang => "hang",
            Self::UnexpectedReset => "unexpected-reset",
        }
    }
}

/// One step of a run, as `events.jsonl` records it in order.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RunEventKind {
    RunStarted,
    RunFinished,
    RunInterrupted,
    SourceSnapshotBound,
    PlanResolved,
    ImageBuildStarted,
    ImageBuildFinished,
    ImageBuildFailed,
    ImageFlashStarted,
    ImageFlashFinished,
    ImageFlashFailed,
    ImageReplayImportStarted,
    ImageReplayImportFinished,
    ImageReplayImportFailed,
    LabProvenanceCaptured,
    StandLeaseRequested,
    StandLeaseGranted,
    StandLeaseYielded,
    ScenarioStarted,
    ScenarioFinished,
    ScenarioBlocked,
    ThenStarted,
    ThenSucceeded,
    ThenFailed,
}

impl RunEventKind {
    pub const fn id(self) -> &'static str {
        match self {
            Self::RunStarted => "run-started",
            Self::RunFinished => "run-finished",
            Self::RunInterrupted => "run-interrupted",
            Self::SourceSnapshotBound => "source-snapshot-bound",
            Self::PlanResolved => "plan-resolved",
            Self::ImageBuildStarted => "image-build-started",
            Self::ImageBuildFinished => "image-build-finished",
            Self::ImageBuildFailed => "image-build-failed",
            Self::ImageFlashStarted => "image-flash-started",
            Self::ImageFlashFinished => "image-flash-finished",
            Self::ImageFlashFailed => "image-flash-failed",
            Self::ImageReplayImportStarted => "image-replay-import-started",
            Self::ImageReplayImportFinished => "image-replay-import-finished",
            Self::ImageReplayImportFailed => "image-replay-import-failed",
            Self::LabProvenanceCaptured => "lab-provenance-captured",
            Self::StandLeaseRequested => "stand-lease-requested",
            Self::StandLeaseGranted => "stand-lease-granted",
            Self::StandLeaseYielded => "stand-lease-yielded",
            Self::ScenarioStarted => "scenario-started",
            Self::ScenarioFinished => "scenario-finished",
            Self::ScenarioBlocked => "scenario-blocked",
            Self::ThenStarted => "then-started",
            Self::ThenSucceeded => "then-succeeded",
            Self::ThenFailed => "then-failed",
        }
    }
}

/// One line of a run's `events.jsonl`.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct RunEvent {
    pub timestamp_unix_millis: u64,
    pub kind: RunEventKind,
    pub scenario: Option<String>,
    /// The image class's identifier.
    pub image: Option<String>,
    pub outcome: Option<Outcome>,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MeasurementUnit {
    Count,
    Bytes,
    BitsPerSecond,
    Microseconds,
    BasisPoints,
}

impl MeasurementUnit {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Count => "count",
            Self::Bytes => "bytes",
            Self::BitsPerSecond => "bit/s",
            Self::Microseconds => "us",
            Self::BasisPoints => "bp",
        }
    }
}

/// Which way a measured quantity improves, as its producer declares it.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Better {
    Higher,
    Lower,
}

impl Better {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Higher => "higher",
            Self::Lower => "lower",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Comparison {
    AtLeast,
    AtMost,
    Exactly,
}

impl Comparison {
    /// The direction a threshold of this comparison prefers: `None` for
    /// `exactly`, a correctness check rather than a figure of merit.
    pub const fn better(self) -> Option<Better> {
        match self {
            Self::AtLeast => Some(Better::Higher),
            Self::AtMost => Some(Better::Lower),
            Self::Exactly => None,
        }
    }

    pub const fn symbol(self) -> &'static str {
        match self {
            Self::AtLeast => "&gt;=",
            Self::AtMost => "&lt;=",
            Self::Exactly => "=",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub struct Threshold {
    pub comparison: Comparison,
    pub value: u64,
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum MeasurementVerdict {
    Passed,
    Failed,
}

macro_rules! display_by_id {
    ($($type:ty),*) => {
        $(impl core::fmt::Display for $type {
            fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                formatter.write_str(self.id())
            }
        })*
    };
}

display_by_id!(RunState, Outcome, FailureKind, MeasurementUnit, Better);

impl MeasurementVerdict {
    pub const fn is_passed(self) -> bool {
        matches!(self, Self::Passed)
    }
}

impl core::fmt::Display for Threshold {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let comparison = match self.comparison {
            Comparison::AtLeast => ">=",
            Comparison::AtMost => "<=",
            Comparison::Exactly => "=",
        };
        write!(formatter, "{comparison} {}", self.value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_are_the_serialized_names() {
        for state in [
            RunState::Running,
            RunState::Completed,
            RunState::Interrupted,
        ] {
            assert_eq!(serde_json::to_value(state).unwrap(), state.id());
        }
        for outcome in [
            Outcome::Passed,
            Outcome::Failed,
            Outcome::Broken,
            Outcome::Skipped,
            Outcome::Blocked,
            Outcome::Interrupted,
        ] {
            assert_eq!(serde_json::to_value(outcome).unwrap(), outcome.to_string());
        }
        assert!(serde_json::from_value::<Outcome>("sealed".into()).is_err());
    }
}
