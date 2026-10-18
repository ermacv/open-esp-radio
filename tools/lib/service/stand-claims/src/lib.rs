//! What a stand lease claims: boards, fixture resources, the air and the
//! frequency ranges of its radio use ([`spectrum`]), each shared or
//! exclusive; when two sets of claims conflict and when one covers another.
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};

pub mod spectrum;

/// Claims everything: a lease that names no resources.
pub const STAND: &str = "stand";
/// The shared radio environment. RF-sensitive work claims it exclusively;
/// other radio work claims it shared.
pub const AIR: &str = "air";

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Mode {
    Exclusive,
    Shared,
}

/// One resource a lease holds.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Claim {
    pub resource: String,
    pub mode: Mode,
}

impl Claim {
    pub fn exclusive(resource: impl Into<String>) -> Self {
        Self {
            resource: resource.into(),
            mode: Mode::Exclusive,
        }
    }

    pub fn shared(resource: impl Into<String>) -> Self {
        Self {
            resource: resource.into(),
            mode: Mode::Shared,
        }
    }

    /// A board, identified by its MAC.
    pub fn board(identity: &str) -> Self {
        Self::exclusive(format!("board:{identity}"))
    }

    pub fn stand() -> Self {
        Self::exclusive(STAND)
    }
}

impl std::fmt::Display for Claim {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if let Some(range) = crate::spectrum::Spectrum::parse(&self.resource) {
            return range.fmt(f);
        }
        match self.mode {
            Mode::Exclusive => f.write_str(&self.resource),
            Mode::Shared => write!(f, "{} (shared)", self.resource),
        }
    }
}

/// Claims without duplicates, an exclusive claim absorbing a shared one;
/// no claims means the whole stand.
pub fn normalize(claims: &[Claim]) -> Vec<Claim> {
    if claims.is_empty() || claims.iter().any(|claim| claim.resource == STAND) {
        return vec![Claim::stand()];
    }
    let mut normalized: Vec<Claim> = Vec::new();
    for claim in claims {
        match normalized
            .iter_mut()
            .find(|known| known.resource == claim.resource)
        {
            Some(known) if claim.mode == Mode::Exclusive => known.mode = Mode::Exclusive,
            Some(_) => {}
            None => normalized.push(claim.clone()),
        }
    }
    normalized.sort_by(|a, b| a.resource.cmp(&b.resource));
    normalized
}

/// Whether two sets of claims cannot be held at once. When both carry
/// frequency ranges, the ranges order their radio use and the coarse air
/// claim is left to arbiters without ranges.
pub fn conflict(a: &[Claim], b: &[Claim]) -> bool {
    use crate::spectrum::{is_range, ranges};
    let whole = |claims: &[Claim]| claims.iter().any(|claim| claim.resource == STAND);
    if whole(a) || whole(b) {
        return true;
    }
    let (a_ranges, b_ranges) = (ranges(a), ranges(b));
    let by_ranges = !a_ranges.is_empty() && !b_ranges.is_empty();
    if by_ranges
        && a_ranges
            .iter()
            .any(|x| b_ranges.iter().any(|y| x.conflicts(y)))
    {
        return true;
    }
    a.iter().any(|x| {
        b.iter().any(|y| {
            x.resource == y.resource
                && !is_range(&x.resource)
                && !(by_ranges && x.resource == AIR)
                && (x.mode == Mode::Exclusive || y.mode == Mode::Exclusive)
        })
    })
}

/// Whether a lease holding `outer` already holds everything in `inner`.
pub fn covers(outer: &[Claim], inner: &[Claim]) -> bool {
    use crate::spectrum::Spectrum;
    let held_ranges = crate::spectrum::ranges(outer);
    outer.iter().any(|claim| claim.resource == STAND)
        || inner.iter().all(|wanted| {
            if let Some(range) = Spectrum::parse(&wanted.resource) {
                // An exclusive coarse air claim holds every range.
                return held_ranges.iter().any(|held| held.covers(&range))
                    || outer
                        .iter()
                        .any(|held| held.resource == AIR && held.mode == Mode::Exclusive);
            }
            wanted.resource != STAND
                && outer.iter().any(|held| {
                    held.resource == wanted.resource
                        && (held.mode == Mode::Exclusive || wanted.mode == Mode::Shared)
                })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claims(list: &[(&str, Mode)]) -> Vec<Claim> {
        list.iter()
            .map(|(resource, mode)| Claim {
                resource: (*resource).into(),
                mode: *mode,
            })
            .collect()
    }

    #[test]
    fn claims_conflict_on_exclusive_use_and_on_the_whole_stand() {
        use Mode::{Exclusive, Shared};
        let dut = claims(&[("board:chip-a", Exclusive), (AIR, Shared)]);
        let peer = claims(&[("board:chip-b", Exclusive), (AIR, Shared)]);
        let rf = claims(&[("board:chip-b", Exclusive), (AIR, Exclusive)]);
        assert!(!conflict(&dut, &peer), "different boards share the air");
        assert!(
            conflict(&dut, &rf),
            "exclusive air excludes other radio work"
        );
        assert!(conflict(&dut, &dut), "one board has one holder");
        assert!(conflict(&[Claim::stand()], &peer));
    }

    #[test]
    fn ranges_order_radio_use_between_new_leases_and_the_air_claim_for_old_ones() {
        use crate::spectrum::{BAND_2G4, BAND_5G, Emits, Need, Spectrum};
        use Mode::{Exclusive, Shared};
        let with = |board: &str, range: Spectrum| {
            let mut claims = vec![Claim::board(board)];
            claims.extend(range.claims());
            normalize(&claims)
        };
        let calibration = with("a", Spectrum::new(BAND_2G4, Need::Strict, Emits::Normal));
        let five = with("b", Spectrum::new(BAND_5G, Need::Tolerant, Emits::Normal));
        let peer = with("d", Spectrum::ieee802154(15, Need::Tolerant, Emits::Normal));
        assert!(
            !conflict(&calibration, &five),
            "disjoint bands run together"
        );
        assert!(
            conflict(&calibration, &peer),
            "a strict band excludes a transmitter in it"
        );
        assert!(!conflict(&five, &peer));
        // A lease from a checkout without ranges claims only the air.
        let old_shared = claims(&[("board:c", Exclusive), (AIR, Shared)]);
        let old_exclusive = claims(&[("board:c", Exclusive), (AIR, Exclusive)]);
        assert!(
            conflict(&calibration, &old_shared),
            "strict holds the air exclusively"
        );
        assert!(!conflict(&five, &old_shared));
        assert!(conflict(&five, &old_exclusive));
        assert!(!conflict(
            &claims(&[("x", Shared)]),
            &claims(&[("y", Exclusive)])
        ));
    }

    #[test]
    fn normalization_merges_modes_and_defaults_to_the_stand() {
        use Mode::{Exclusive, Shared};
        assert_eq!(normalize(&[]), [Claim::stand()]);
        assert_eq!(
            normalize(&claims(&[
                (AIR, Shared),
                ("board:a", Exclusive),
                (AIR, Exclusive)
            ])),
            claims(&[(AIR, Exclusive), ("board:a", Exclusive)])
        );
    }

    #[test]
    fn a_lease_covers_what_it_holds_at_least_as_strongly() {
        use Mode::{Exclusive, Shared};
        let held = claims(&[("board:a", Exclusive), (AIR, Shared)]);
        assert!(covers(&held, &claims(&[("board:a", Exclusive)])));
        assert!(covers(&held, &claims(&[(AIR, Shared)])));
        assert!(!covers(&held, &claims(&[(AIR, Exclusive)])));
        assert!(!covers(&held, &[Claim::stand()]));
        assert!(covers(&[Claim::stand()], &held));
    }
}
