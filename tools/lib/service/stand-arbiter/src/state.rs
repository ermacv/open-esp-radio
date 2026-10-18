//! The shared queue and the lease holders.

use serde::{Deserialize, Serialize};

use std::collections::BTreeMap;

use oer_stand_claims::Claim;

use crate::{balance::Balance, process::ProcessIdentity};

/// Schema 3 serves requests by owner balance instead of arrival and budget.
pub(crate) const STATE_SCHEMA: u32 = 3;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct State {
    pub(crate) schema: u32,
    pub(crate) next_id: u64,
    pub(crate) queue: Vec<Ticket>,
    pub(crate) holders: Vec<Holder>,
    /// Every recently active owner's balance; see [`crate::balance`].
    #[serde(default)]
    pub(crate) balances: BTreeMap<String, Balance>,
    /// When the balances were last advanced; 0 before the first settlement.
    #[serde(default)]
    pub(crate) settled_unix_ms: u64,
    /// Fields a newer build wrote, kept when this build rewrites the record.
    #[serde(flatten)]
    pub(crate) unknown: crate::Unknown,
}

impl Default for State {
    fn default() -> Self {
        Self {
            schema: STATE_SCHEMA,
            next_id: 1,
            queue: Vec::new(),
            holders: Vec::new(),
            balances: BTreeMap::new(),
            settled_unix_ms: 0,
            unknown: crate::Unknown::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Ticket {
    pub(crate) id: u64,
    pub(crate) owner: String,
    pub(crate) work: String,
    /// Expected duration from earlier leases, for waiting-time estimates
    /// only; never a limit.
    #[serde(default)]
    pub(crate) estimate_secs: u64,
    pub(crate) process: ProcessIdentity,
    pub(crate) enqueued_unix: u64,
    pub(crate) claims: Vec<Claim>,
    /// Maintenance goes before every ordinary request.
    #[serde(default, skip_serializing_if = "Priority::is_ordinary")]
    pub(crate) priority: Priority,
    /// The `cargo hil` job whose process, or its runner, made the request
    /// ([`crate::jobs::JOB_KEY`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) job: Option<String>,
    /// The HIL run the lease executes, when a runner requested it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) run: Option<String>,
    /// Fields a newer build wrote, kept when this build rewrites the record.
    #[serde(flatten)]
    pub(crate) unknown: crate::Unknown,
}

/// How a request is ordered against the others.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Priority {
    /// Served by its owner's balance.
    #[default]
    Ordinary,
    /// Stand maintenance, such as a fixture software installation: served
    /// before every ordinary request, and it preempts the holders it
    /// conflicts with.
    Maintenance,
}

impl Priority {
    fn is_ordinary(&self) -> bool {
        *self == Self::Ordinary
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub(crate) struct Holder {
    pub(crate) ticket: Ticket,
    /// Exported to commands inside the lease, which then join it.
    pub(crate) token: String,
    pub(crate) granted_unix: u64,
    /// The balances the grant was decided by.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reason: Option<crate::history::GrantReason>,
    /// Set once `cargo stand preempt` stopped the lease; it is charged no
    /// longer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) preempted: Option<crate::preempt::Preemption>,
    /// Fields a newer build wrote, kept when this build rewrites the record.
    #[serde(flatten)]
    pub(crate) unknown: crate::Unknown,
}

/// Why a state file of another schema cannot be read. The arbiter reads one
/// schema: a newer state needs a newer checkout, and an older one is never
/// converted, so the operator drains or resets it.
pub(crate) fn unreadable(path: &std::path::Path, schema: &serde_json::Value) -> String {
    let path = path.display();
    match schema.as_u64() {
        Some(schema) if schema > u64::from(STATE_SCHEMA) => format!(
            "HIL arbiter state {path} has schema {schema}; this checkout reads schema \
             {STATE_SCHEMA}. Update the checkout"
        ),
        _ => format!(
            "HIL arbiter state {path} has schema {schema}; this checkout reads only schema \
             {STATE_SCHEMA} and does not convert older states. Let the checkouts that hold or \
             wait for the stand finish, then remove {path} to reset the queue"
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_state_of_another_schema_names_its_remedy() {
        let path = std::path::Path::new("/stand/state.json");
        let older = unreadable(path, &serde_json::json!(1));
        assert!(older.contains("does not convert"), "{older}");
        assert!(older.contains("remove /stand/state.json"), "{older}");
        assert!(unreadable(path, &serde_json::Value::Null).contains("does not convert"));
        let newer = unreadable(path, &serde_json::json!(STATE_SCHEMA + 1));
        assert!(newer.contains("Update the checkout"), "{newer}");
    }
}
