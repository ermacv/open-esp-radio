//! VIF parameter isolation and head-selected contention on shared AC queues.
//! Fixtures inject all-ones entropy and capture actual publication draws.

use super::{Environment, Execution, ModelOnlyReason, Report};
use crate::{EdcaAcParameters, EdcaParameterSet, VifRole};
use oer_ieee80211_wire::{
    extensions::wmm::{WmmAcParameters, WmmParameterSet},
    qos::WmmAccessCategory,
};

/// Workload applied to a shared AC queue.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Scenario {
    /// Publish without installing a BSS parameter set.
    Defaults,
    /// Alternate station, AP and station frames under distinct tables.
    AlternatingVifs,
    /// Change the queued AP frame's set while the station head is published.
    QueuedUpdate,
    /// Fail the station head once before the AP and next station frame.
    Retry,
}

/// One workload on one of the four AC queues.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Case {
    /// Workload to exercise.
    pub scenario: Scenario,
    /// Hardware AC queue used by all three frames.
    pub category: WmmAccessCategory,
}
impl Case {
    /// Every workload on every AC, in suite order.
    pub const ALL: [Self; 16] = {
        use Scenario::*;
        use WmmAccessCategory::*;
        [
            Self {
                scenario: Defaults,
                category: BestEffort,
            },
            Self {
                scenario: Defaults,
                category: Background,
            },
            Self {
                scenario: Defaults,
                category: Video,
            },
            Self {
                scenario: Defaults,
                category: Voice,
            },
            Self {
                scenario: AlternatingVifs,
                category: BestEffort,
            },
            Self {
                scenario: AlternatingVifs,
                category: Background,
            },
            Self {
                scenario: AlternatingVifs,
                category: Video,
            },
            Self {
                scenario: AlternatingVifs,
                category: Voice,
            },
            Self {
                scenario: QueuedUpdate,
                category: BestEffort,
            },
            Self {
                scenario: QueuedUpdate,
                category: Background,
            },
            Self {
                scenario: QueuedUpdate,
                category: Video,
            },
            Self {
                scenario: QueuedUpdate,
                category: Voice,
            },
            Self {
                scenario: Retry,
                category: BestEffort,
            },
            Self {
                scenario: Retry,
                category: Background,
            },
            Self {
                scenario: Retry,
                category: Video,
            },
            Self {
                scenario: Retry,
                category: Voice,
            },
        ]
    };

    /// Whether the stimulus can be supplied by a physical fixture.
    pub const fn execution(self) -> Execution {
        match self.scenario {
            Scenario::QueuedUpdate => Execution::ModelOnly(ModelOnlyReason::HeldContention),
            Scenario::Retry => Execution::ModelOnly(ModelOnlyReason::Collision),
            _ => Execution::Hardware,
        }
    }
}

/// Distinct complete tables for fixture input; records are in ACI order.
pub const fn parameters(role: VifRole, updated: bool) -> EdcaParameterSet {
    let (aifsn, minimum, maximum, txop, lifetime) = match (role, updated) {
        (VifRole::Station, _) => (3, 4, 6, 94, 500),
        (VifRole::AccessPoint, false) => (7, 2, 3, 47, 200),
        (VifRole::AccessPoint, true) => (5, 1, 2, 350, 100),
    };
    EdcaParameterSet::new(
        WmmParameterSet::new(
            0,
            false,
            [WmmAcParameters {
                admission_control_mandatory: false,
                aifsn,
                ecw_min: minimum,
                ecw_max: maximum,
                txop_limit_units_32_us: txop,
            }; 4],
        ),
        [lifetime; 4],
    )
}

/// Parameters and backoff observed at a publication boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Draw {
    /// VIF of the published head frame.
    pub role: VifRole,
    /// AC queue whose head published.
    pub category: WmmAccessCategory,
    /// Complete record selected by the publication.
    pub parameters: EdcaAcParameters,
    /// Current contention window, including retry expansion.
    pub window_slots: u16,
    /// Backoff drawn with the injected all-ones entropy.
    pub backoff_slots: u16,
}

/// Captured publications, installed records and returned owners.
pub struct Trace<D> {
    /// Observed draws, not copies of the requested records.
    pub draws: D,
    /// Installed tables read after all three operations have completed.
    pub installed: [EdcaParameterSet; 2],
    /// Ticket identities captured before submitting station, AP, station.
    pub admitted: [u8; 3],
    /// Ticket identities read from the actual terminals, in FIFO order.
    pub completed: [u8; 3],
    /// Draws while only the station head was allowed to publish.
    pub held_draws: Option<usize>,
}

/// Backend-specific admission, entropy injection and draw capture.
pub trait Harness {
    /// Allocator-free or allocating storage of actual observations.
    type Draws: AsRef<[Draw]>;
    /// Run three queued operations and capture their actual publications.
    fn exercise(&mut self, case: Case) -> impl core::future::Future<Output = Trace<Self::Draws>>;
}

/// A mismatch between the workload and its actual observations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Violation {
    /// A held queue published or the number of draws changed.
    Publication,
    /// A VIF table or a head-selected record changed.
    VifParameters,
    /// The draw or retry window differs from the selected record.
    Contention,
    /// The returned owners do not follow queue admission order.
    Fifo,
}

/// Check actual observations against the shared workload.
pub fn verify<D: AsRef<[Draw]>>(case: Case, trace: Trace<D>) -> Result<(), Violation> {
    if trace.completed != trace.admitted {
        return Err(Violation::Fifo);
    }
    let defaults = case.scenario == Scenario::Defaults;
    let updated = case.scenario == Scenario::QueuedUpdate;
    let retry = case.scenario == Scenario::Retry;
    let tables = if defaults {
        [EdcaParameterSet::STANDARD_DEFAULT; 2]
    } else {
        [
            parameters(VifRole::Station, false),
            parameters(VifRole::AccessPoint, updated),
        ]
    };
    if trace.installed != tables {
        return Err(Violation::VifParameters);
    }
    if trace.draws.as_ref().len() != if retry { 4 } else { 3 }
        || trace.held_draws != updated.then_some(1)
    {
        return Err(Violation::Publication);
    }
    for (index, draw) in trace.draws.as_ref().iter().enumerate() {
        let role = if index == if retry { 2 } else { 1 } {
            VifRole::AccessPoint
        } else {
            VifRole::Station
        };
        let set = match role {
            VifRole::Station => tables[0],
            VifRole::AccessPoint => tables[1],
        };
        let record = set.access_category(case.category);
        if draw.role != role || draw.category != case.category || draw.parameters != record {
            return Err(Violation::VifParameters);
        }
        let exponent = if retry && index == 1 {
            record.wmm.ecw_min.saturating_add(1).min(record.wmm.ecw_max)
        } else {
            record.wmm.ecw_min
        };
        let window = (1_u16 << exponent).saturating_sub(1);
        if draw.window_slots != window || draw.backoff_slots != window {
            return Err(Violation::Contention);
        }
    }
    Ok(())
}

/// Run all applicable workloads and verify their captured draws.
pub async fn check<H: Harness>(
    harness: &mut H,
    environment: Environment,
) -> Result<Report, (Case, Violation)> {
    let mut report = Report::default();
    for case in Case::ALL {
        if !report.includes(environment, case.execution()) {
            continue;
        }
        verify(case, harness.exercise(case).await).map_err(|why| (case, why))?;
        report.passed();
    }
    Ok(report)
}
