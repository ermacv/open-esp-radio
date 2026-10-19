//! Versioned Linux fixture provisioning shared by the runner and root apply owner.
//!
//! The privileged `open-radio-fixture-install` binary and the fixed launchers
//! link only this package, so its dependency closure is the root-executed
//! surface. The runner uses the same library for planning and preparation.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

mod admission;
pub mod bluetooth_contract;
pub mod launcher;
mod model;
mod prepare;
mod transaction;

pub use admission::{OperationalLease, admit_system};
pub use model::{
    Artifact, ArtifactRole, ArtifactState, Bundle, InstallPlan, InstallResult, InstallState,
    Provider, SourceIdentity,
};
pub use prepare::{build_plan, prepare};
pub use transaction::apply_system;

pub const INSTALLER_BINARY: &str = "open-radio-fixture-install";
pub const BUNDLE_SCHEMA: u32 = 2;
pub const RECEIPT_SCHEMA: u32 = 1;

/// The stand resource a lease claims `provider`'s installed software by.
pub fn resource(provider: Provider) -> String {
    format!("fixture-software:{}", provider.as_str())
}

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[cfg(test)]
mod tests;
