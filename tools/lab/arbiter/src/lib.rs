//! The scheduling of the single physical stand, for the LAB service that
//! owns it (`oer-tools-lab-service`).
//!
//! Every request claims the boards, fixtures and air it needs. Requests wait
//! in one queue, ordered by their owners' balances; leases on disjoint
//! resources run in parallel ([`scheduler`]). A stateful resource is granted
//! only once its driver confirmed an end state ([`readiness`]). The
//! service's state, the lease history and the board journal live in one
//! directory ([`Arbiter`], the stand model's `paths::arbiter`).
#![expect(
    clippy::cast_possible_truncation,
    reason = "host tool: the rule covers the code an image reaches"
)]
#![expect(
    clippy::cast_possible_wrap,
    reason = "host tool: the rule covers the code an image reaches"
)]
#![expect(
    clippy::let_underscore_must_use,
    reason = "host tool: the rule covers the code an image reaches"
)]
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]
#![expect(
    clippy::arithmetic_side_effects,
    reason = "host tool: the ban covers the code an image reaches"
)]

pub mod claims;
pub mod journal;
pub mod owner;

pub mod balance;
mod estimate;
pub mod health;
pub mod history;
pub mod import;

pub mod maintenance;
pub mod notify;
pub mod preempt;
pub mod process;
mod queue;
pub mod readiness;
pub mod scheduler;
pub mod state;
mod status;
mod store;
mod unknown;

pub use balance::HARD_LIMIT;
pub use estimate::{DEFAULT_ESTIMATE, EstimateSource, estimate, format_duration, parse_duration};
pub use history::{GrantReason, LeaseOutcome, LeaseRecord, OwnerBalance};
pub use maintenance::{Maintenance, STAND_SERVICE};
pub use state::Priority;
pub use status::{HolderStatus, QueuedStatus, Status, describe_claims};
pub use store::Arbiter;
pub use unknown::Unknown;

pub type Result<T> = oer_tools_os_process::Result<T>;
