//! The Linux fixture contract of the HIL stand: provisioning of the fixture
//! software and the requests and reports of its finite helpers.
//!
//! The privileged `open-radio-fixture-install` program and the fixed
//! launchers (`tools/lab/fixture-install`) install and admit the software; the
//! unprivileged runner plans and prepares through the same library, and
//! parses the requests and reports of `open-radio-bluetooth` and
//! `open-radio-probe` (`tools/lab/fixture-helpers`) with the same types. The programs
//! are packages without a library.
#![forbid(unsafe_code)]

mod admission;
pub mod bluetooth;
pub mod hostapd;
pub mod launcher;
pub mod local;
mod model;
pub mod network_client;
mod prepare;
#[cfg(target_os = "linux")]
pub mod probe;
mod transaction;
pub mod udhcpc;

pub use admission::{OperationalLease, admit_system};
pub use model::{
    Artifact, ArtifactRole, ArtifactState, Bundle, InstallPlan, InstallResult, InstallState,
    Provider, SourceIdentity,
};
pub use prepare::{build_plan, prepare};
pub use transaction::{ExistingInstallation, apply_system, inspect_system};

pub const INSTALLER_BINARY: &str = "open-radio-fixture-install";
pub const BUNDLE_SCHEMA: u32 = 3;
pub const RECEIPT_SCHEMA: u32 = 1;

/// The stand resource a lease claims `provider`'s installed software by.
pub fn resource(provider: Provider) -> String {
    format!("fixture-software:{}", provider.as_str())
}

pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[cfg(test)]
mod tests;
