//! What a scenario needs from the stand: boards by role and capability,
//! and fixtures.

use core::fmt;

use serde::{Deserialize, Serialize};

use crate::{ChipId, FixtureName};

/// What a board does in a scenario.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum BoardRole {
    /// The device under test.
    Dut,
    /// A peer: a reference board, an access-point client, a BLE central.
    Peer,
    /// An air observer.
    Observer,
}

/// The low-power clock source a board has.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum LpClockSource {
    /// The internal RC oscillator, on every board.
    RcSlow,
    /// A 32 kHz crystal. ESP32-S31-Function-CoreBoard-1 has none (,
    /// decision 7).
    Xtal32k,
}

/// A property of a board a scenario may require.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case", tag = "capability", content = "value")]
pub enum BoardCapability {
    LpClock(LpClockSource),
    /// An antenna connector for conducted RF.
    Ufl,
}

/// One board a scenario needs.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct BoardNeed {
    pub role: BoardRole,
    pub chip: ChipId,
    pub capabilities: Vec<BoardCapability>,
}

/// One board the stand offers.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct BoardOffer {
    pub chip: ChipId,
    pub roles: Vec<BoardRole>,
    pub capabilities: Vec<BoardCapability>,
}

/// What the stand offers a plan.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct StandOffer {
    pub boards: Vec<BoardOffer>,
    pub fixtures: Vec<FixtureName>,
}

/// Everything a scenario needs from the stand.
#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub struct Resources {
    pub boards: Vec<BoardNeed>,
    pub fixtures: Vec<FixtureName>,
}

/// Why the stand cannot run a scenario, found at plan time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Unmet {
    /// No board left for the need; `missing` is what the board of this
    /// chip and role that lacks the fewest of the need's capabilities
    /// lacks. It is empty when no board takes the role at all, or when a
    /// fitting board exists but other needs of the scenario hold it.
    Board {
        role: BoardRole,
        chip: ChipId,
        missing: Vec<BoardCapability>,
    },
    Fixture(FixtureName),
}

impl fmt::Display for Unmet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Board {
                role,
                chip,
                missing,
            } if missing.is_empty() => {
                write!(f, "no free {chip} board can take the {role:?} role")
            }
            Self::Board {
                role,
                chip,
                missing,
            } => write!(f, "no {chip} board for the {role:?} role has {missing:?}"),
            Self::Fixture(name) => write!(f, "the stand has no fixture `{name}`"),
        }
    }
}

fn takes(need: &BoardNeed, board: &BoardOffer) -> bool {
    board.chip == need.chip && board.roles.contains(&need.role)
}

fn fits(need: &BoardNeed, board: &BoardOffer) -> bool {
    takes(need, board)
        && need
            .capabilities
            .iter()
            .all(|c| board.capabilities.contains(c))
}

/// Kuhn's augmenting path for `need`; `owner[board]` is the need holding
/// it. A failed search leaves `owner` unchanged.
fn augment(
    needs: &[BoardNeed],
    boards: &[BoardOffer],
    need: usize,
    seen: &mut [bool],
    owner: &mut [Option<usize>],
) -> bool {
    for board in 0..boards.len() {
        if seen[board] || !fits(&needs[need], &boards[board]) {
            continue;
        }
        seen[board] = true;
        if owner[board].is_none_or(|other| augment(needs, boards, other, seen, owner)) {
            owner[board] = Some(need);
            return true;
        }
    }
    false
}

impl Resources {
    /// What the stand cannot provide. Needed boards are matched to distinct
    /// offered boards by a maximum bipartite matching, so a board with a
    /// rare capability is not spent on a need that does not require it.
    /// Empty when the stand can run the scenario.
    pub fn unmet(&self, stand: &StandOffer) -> Vec<Unmet> {
        let mut owner = vec![None; stand.boards.len()];
        let mut unmet = Vec::new();
        for (index, need) in self.boards.iter().enumerate() {
            let mut seen = vec![false; stand.boards.len()];
            if augment(&self.boards, &stand.boards, index, &mut seen, &mut owner) {
                continue;
            }
            let missing = stand
                .boards
                .iter()
                .filter(|board| takes(need, board))
                .map(|board| {
                    need.capabilities
                        .iter()
                        .filter(|c| !board.capabilities.contains(c))
                        .copied()
                        .collect::<Vec<_>>()
                })
                .min_by_key(Vec::len)
                .unwrap_or_default();
            unmet.push(Unmet::Board {
                role: need.role,
                chip: need.chip.clone(),
                missing,
            });
        }
        for fixture in &self.fixtures {
            if !stand.fixtures.contains(fixture) {
                unmet.push(Unmet::Fixture(fixture.clone()));
            }
        }
        unmet
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::name;

    fn s31(capabilities: Vec<BoardCapability>) -> BoardOffer {
        BoardOffer {
            chip: name!(ChipId, "esp32s31"),
            roles: vec![BoardRole::Dut, BoardRole::Peer],
            capabilities,
        }
    }

    fn need(role: BoardRole, capabilities: Vec<BoardCapability>) -> BoardNeed {
        BoardNeed {
            role,
            chip: name!(ChipId, "esp32s31"),
            capabilities,
        }
    }

    fn stand(boards: Vec<BoardOffer>, fixtures: Vec<FixtureName>) -> StandOffer {
        StandOffer { boards, fixtures }
    }

    #[test]
    fn a_crystal_scenario_is_rejected_on_a_board_without_one() {
        let crystal = BoardCapability::LpClock(LpClockSource::Xtal32k);
        let needs = Resources {
            boards: vec![need(BoardRole::Dut, vec![crystal])],
            ..Resources::default()
        };
        let offer = stand(
            vec![s31(vec![BoardCapability::LpClock(LpClockSource::RcSlow)])],
            Vec::new(),
        );
        assert_eq!(
            needs.unmet(&offer),
            vec![Unmet::Board {
                role: BoardRole::Dut,
                chip: name!(ChipId, "esp32s31"),
                missing: vec![crystal]
            }]
        );
    }

    #[test]
    fn a_rare_capability_is_kept_for_the_need_that_requires_it() {
        let needs = Resources {
            boards: vec![
                need(BoardRole::Dut, Vec::new()),
                need(BoardRole::Peer, vec![BoardCapability::Ufl]),
            ],
            ..Resources::default()
        };
        let offer = stand(
            vec![s31(vec![BoardCapability::Ufl]), s31(Vec::new())],
            Vec::new(),
        );
        assert!(needs.unmet(&offer).is_empty());
    }

    #[test]
    fn an_unmet_need_names_what_the_closest_board_lacks() {
        let crystal = BoardCapability::LpClock(LpClockSource::Xtal32k);
        let needs = Resources {
            boards: vec![need(BoardRole::Dut, vec![BoardCapability::Ufl, crystal])],
            ..Resources::default()
        };
        let offer = stand(
            vec![s31(Vec::new()), s31(vec![BoardCapability::Ufl])],
            Vec::new(),
        );
        assert_eq!(
            needs.unmet(&offer),
            vec![Unmet::Board {
                role: BoardRole::Dut,
                chip: name!(ChipId, "esp32s31"),
                missing: vec![crystal]
            }]
        );
    }

    #[test]
    fn each_needed_board_takes_a_distinct_offer_and_fixtures_are_named() {
        let hostapd = name!(FixtureName, "hostapd");
        let needs = Resources {
            boards: vec![
                need(BoardRole::Dut, Vec::new()),
                need(BoardRole::Peer, Vec::new()),
            ],
            fixtures: vec![hostapd.clone()],
        };
        assert_eq!(
            needs
                .unmet(&stand(vec![s31(Vec::new())], vec![hostapd.clone()]))
                .len(),
            1
        );
        let two = vec![s31(Vec::new()), s31(Vec::new())];
        assert!(
            needs
                .unmet(&stand(two.clone(), vec![hostapd.clone()]))
                .is_empty()
        );
        assert_eq!(
            needs.unmet(&stand(two, Vec::new())),
            vec![Unmet::Fixture(hostapd)]
        );
    }
}
