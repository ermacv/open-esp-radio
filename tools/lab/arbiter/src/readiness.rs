//! Whether a stateful resource of the stand (a board, the router) can be
//! granted. Only the service changes it: a grant puts it in use, the end of
//! the lease starts its recovery, and only its driver's confirmation of an
//! end state its kind declares makes it ready again. The end of a lease alone
//! never does.
//!
//! Every grant and every recovery starts a new generation of the resource,
//! so a confirmation of an older one changes nothing.

use serde::{Deserialize, Serialize};

use crate::claims::{AIR, Claim, STAND, spectrum};

/// The resource name of the lab router.
pub const ROUTER: &str = "router";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Readiness {
    /// Confirmed in an end state of its kind; for a board a quiet one.
    Ready { generation: u64 },
    /// Granted to lease `lease`.
    InUse { lease: u64, generation: u64 },
    /// Its driver brings it to an end state; `lease` is the lease that left
    /// it, none after a restart or a person's release.
    Recovering { lease: Option<u64>, generation: u64 },
    /// No end state was confirmed: nobody is granted it until a person
    /// returns it. `quiet` when its driver still confirmed it cannot radiate,
    /// such as a board whose power is confirmed off.
    Unavailable {
        reason: String,
        quiet: bool,
        generation: u64,
    },
}

impl Readiness {
    pub fn generation(&self) -> u64 {
        match self {
            Self::Ready { generation }
            | Self::InUse { generation, .. }
            | Self::Recovering { generation, .. }
            | Self::Unavailable { generation, .. } => *generation,
        }
    }

    /// Whether the resource cannot radiate outside the leases the queue
    /// already orders: a lease's own radio use is ordered by its air claims.
    pub(crate) fn quiet(&self) -> bool {
        match self {
            Self::Ready { .. } | Self::InUse { .. } => true,
            Self::Recovering { .. } => false,
            Self::Unavailable { quiet, .. } => *quiet,
        }
    }
}

/// A recovery the service runs: bring `resource` to an end state and
/// confirm it with this `generation`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Recovery {
    pub resource: String,
    pub generation: u64,
}

/// What a driver confirmed at the end of a recovery.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Confirmation {
    Ready,
    Unavailable { reason: String, quiet: bool },
}

/// Whether `resource` is a board's.
pub(crate) fn is_board(resource: &str) -> bool {
    resource.starts_with("board:")
}

/// The stateful resources `claims` hold, out of the stand's `stand`.
pub(crate) fn held<'a>(claims: &[Claim], stand: &'a [String]) -> Vec<&'a str> {
    if claims.iter().any(|claim| claim.resource == STAND) {
        return stand.iter().map(String::as_str).collect();
    }
    stand
        .iter()
        .filter(|resource| claims.iter().any(|claim| claim.resource == **resource))
        .map(String::as_str)
        .collect()
}

/// Whether `claims` use the air: the air itself, a frequency range or the
/// whole stand.
pub(crate) fn uses_air(claims: &[Claim]) -> bool {
    claims.iter().any(|claim| {
        claim.resource == AIR || claim.resource == STAND || spectrum::is_range(&claim.resource)
    })
}

#[cfg(test)]
mod tests;
