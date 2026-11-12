#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]
#![allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods,
    clippy::panic,
    clippy::expect_used,
    clippy::unwrap_used,
    clippy::unreachable
)]
//! The shared conformance suite of the IEEE 802.11 lower-MAC port.
//!
//! - [`data_port`]: the cases every data port passes, called as its actor
//!   calls it.
//! - [`access_point`]: the access-point role of a port, beside the
//!   station's on the one physical owner.
//! - [`arena`]: intake, publication and settlement on a port's two TX
//!   arenas.
//! - [`group_keys`]: the acceptance of a GTK mutation's receive cut, from a
//!   backend fixture's actual observations.
//! - [`edca`]: distinct EDCA tables a fixture installs.
//!
//! A backend's tests implement the harness of each suite; observations come
//! from actual submitted and returned owners, never from the expected
//! identities.

/// A port call's own answer; an item it could not keep, or a submission
/// it did not accept, is the case's violation `unkept` (the cases' sinks
/// take what the port gives).
pub(crate) fn answered<R, E, V>(answer: Result<R, E>, unkept: V) -> Result<R, V> {
    answer.map_err(|_| unkept)
}

pub mod access_point;
pub mod arena;
pub mod data_port;
pub mod edca;
pub mod group_keys;
pub mod pull;

#[cfg(test)]
extern crate alloc;
#[cfg(test)]
extern crate std;

/// Where a fixture obtains the requested outcomes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Environment {
    /// A host model can inject every declared fault and ingress stage.
    Model,
    /// A physical backend uses real air and hardware observations.
    Hardware,
}

/// Why a case requires the host model rather than a physical air exchange.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ModelOnlyReason {
    /// Fail-stop/quarantine is deliberately injected into the backend.
    PoisonedBackend,
    /// DMA, interrupt and software ingress stages are injected independently.
    DelayedIngress,
    /// A BlockAck that misses chosen MPDUs requires an injected answer.
    PartialBlockAck,
    /// A protection exchange without a CTS requires an injected answer.
    ProtectionFailure,
}

/// A case's physical execution requirements, independent of its fixture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Execution {
    /// A physical fixture can use real frames and cooperative backend progress.
    Hardware,
    /// The case requires an explicitly injected model condition.
    ModelOnly(ModelOnlyReason),
}

/// Explicit accounting of executed cases and model-only exclusions.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Report {
    /// Cases whose observations passed their shared verifier.
    pub passed: u16,
    /// Cases explicitly excluded from a physical-backend run.
    pub model_only: u16,
}
impl Report {
    pub(crate) fn includes(&mut self, environment: Environment, execution: Execution) -> bool {
        if environment == Environment::Hardware && matches!(execution, Execution::ModelOnly(_)) {
            self.model_only = self.model_only.saturating_add(1);
            false
        } else {
            true
        }
    }
    pub(crate) fn passed(&mut self) {
        self.passed = self.passed.saturating_add(1);
    }
}
