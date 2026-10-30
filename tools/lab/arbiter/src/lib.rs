//! The scheduling of the single physical stand, for the LAB service that
//! owns it (`oer-lab-service`).
//!
//! Every request claims the boards, fixtures and air it needs. Requests wait
//! in one queue, ordered by their owners' balances; leases on disjoint
//! resources run in parallel ([`scheduler`]). A stateful resource is granted
//! only once its driver confirmed an end state ([`readiness`]). The
//! service's state, the lease history and the board journal live in one
//! directory ([`Arbiter`], the stand model's `paths::arbiter`).
#![forbid(unsafe_code)]

pub mod claims;
pub mod journal;
pub mod owner;

pub mod balance;
mod estimate;
pub mod health;
pub mod history;

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

pub type Result<T> = oer_os_process::Result<T>;
