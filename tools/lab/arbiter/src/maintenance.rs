//! Boards an owner took out of service.
//!
//! A board under maintenance serves only the owner who took it out: every
//! other request that claims it, or the whole stand, waits (a tool that acts
//! on one board is told at once). Maintenance never stops a lease already
//! held; it keeps later ones away. The entries live in the service's state.
//! A board no driver could bring back is not under maintenance but
//! unavailable (`crate::readiness`).

use serde::{Deserialize, Serialize};

use crate::claims::Claim;

/// The `mac` of a maintenance entry that takes the whole stand out of
/// service.
pub const STAND_SERVICE: &str = "stand";

/// One board, or the stand, out of service.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Maintenance {
    /// The board's MAC, or [`STAND_SERVICE`].
    pub mac: String,
    /// The only owner whose requests may claim the board.
    pub owner: String,
    pub reason: String,
    pub since_unix: u64,
    /// Fields a newer build wrote, kept when this build rewrites the record.
    #[serde(flatten)]
    pub unknown: crate::Unknown,
}

/// Why `owner`'s request for `claims` may not be served while `entries`
/// keep boards out of service.
pub fn refusal(entries: &[Maintenance], owner: &str, claims: &[Claim]) -> Option<String> {
    let whole = claims
        .iter()
        .any(|claim| claim.resource == crate::claims::STAND);
    entries
        .iter()
        .filter(|entry| entry.owner != owner)
        .find(|entry| {
            whole
                || entry.mac == STAND_SERVICE
                || claims
                    .iter()
                    .any(|claim| claim.resource == Claim::board(&entry.mac).resource)
        })
        .map(|entry| {
            if entry.mac == STAND_SERVICE {
                format!(
                    "the stand is under maintenance by {}: {}; it serves only its maintainer \
                     until `cargo stand devices release --stand`",
                    entry.owner, entry.reason
                )
            } else {
                format!(
                    "board {} is under maintenance by {}: {}; it serves only its maintainer \
                     until `cargo stand devices release`",
                    entry.mac, entry.owner, entry.reason
                )
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(mac: &str, owner: &str) -> Maintenance {
        Maintenance {
            mac: mac.into(),
            owner: owner.into(),
            reason: "rework".into(),
            since_unix: 1,
            unknown: Default::default(),
        }
    }

    #[test]
    fn a_board_under_maintenance_serves_only_its_maintainer() {
        let entries = [entry("02:00:00:00:00:01", "ann")];
        let claims = [Claim::board("02:00:00:00:00:01")];
        assert!(refusal(&entries, "ann", &claims).is_none());
        assert!(
            refusal(&entries, "bob", &claims)
                .unwrap()
                .contains("maintenance by ann")
        );
        assert!(refusal(&entries, "bob", &[Claim::board("02:00:00:00:00:02")]).is_none());
        assert!(refusal(&entries, "bob", &[Claim::stand()]).is_some());
    }

    #[test]
    fn a_whole_stand_maintenance_refuses_every_other_owner() {
        let entries = [entry(STAND_SERVICE, "ann")];
        assert!(refusal(&entries, "bob", &[Claim::board("02:00:00:00:00:02")]).is_some());
        assert!(refusal(&entries, "ann", &[Claim::stand()]).is_none());
    }
}
