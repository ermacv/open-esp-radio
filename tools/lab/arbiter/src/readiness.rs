//! Whether a stateful resource of the stand (a board, the router) can be
//! granted. Only the service changes it: a grant puts it in use, the end of
//! the lease starts its recovery, and only its driver's confirmation of an
//! end state its kind declares makes it ready again. The end of a lease alone
//! never does.
//!
//! Every grant and every recovery starts a new generation of the resource,
//! so a confirmation of an older one changes nothing.
//!
//! Whether a resource may be granted is apart from what it may put on the
//! air ([`Emission`]): a ready resource is in an end state its driver
//! confirmed, which for a board is one that cannot radiate and for the
//! router an access point in a profile the stand knows; a resource in use
//! radiates under its lease's air claims; one recovering, or unavailable
//! without a confirmation that it cannot radiate, may radiate unknown to
//! the stand and closes the air to everyone else.

use serde::{Deserialize, Serialize};

use crate::claims::{AIR, Claim, STAND, spectrum};

/// The resource name of the lab router.
pub const ROUTER: &str = "router";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "state", rename_all = "kebab-case", deny_unknown_fields)]
pub enum Readiness {
    /// Confirmed in an end state of its kind; for a board a quiet one.
    Ready { generation: u64 },
    /// Granted to lease `lease`. `marked` says why an operation of the
    /// lease may have left it in a state its recovery cannot confirm: the
    /// end of the lease then leaves it unavailable.
    InUse {
        lease: u64,
        generation: u64,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        marked: Option<String>,
    },
    /// Its driver brings it to an end state; `lease` is the lease that left
    /// it, none after a restart or a person's release.
    Recovering { lease: Option<u64>, generation: u64 },
    /// No end state was confirmed: nobody is granted it until a person
    /// returns it. `emission` is what its driver still confirmed of what it
    /// may put on the air.
    Unavailable {
        reason: String,
        emission: Emission,
        generation: u64,
    },
}

/// What a resource outside a lease may put on the air.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Emission {
    /// Its driver confirmed it cannot radiate, such as a board whose power
    /// is confirmed off.
    Quiet,
    /// Nothing is confirmed: it may radiate.
    Unknown,
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

    /// Whether what the resource may put on the air is known: confirmed
    /// in its end state, under a lease whose air claims order it, or
    /// confirmed unable to radiate.
    pub(crate) fn emission_known(&self) -> bool {
        match self {
            Self::Ready { .. } | Self::InUse { .. } => true,
            Self::Recovering { .. } => false,
            Self::Unavailable { emission, .. } => *emission == Emission::Quiet,
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
    Unavailable { reason: String, emission: Emission },
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
