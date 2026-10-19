//! The A/B experiment a run belongs to, recorded in its manifest.
//!
//! `cargo hil ab` runs every repetition of an arm with [`EXPERIMENT_ENV`]
//! naming the experiment, the arm, the arm's variant and the round; the
//! runner copies it into the run manifest, so the variant a figure was
//! measured on, and the round it pairs with the other arm in, stay with the
//! run however long after the comparison it is read.

use std::path::PathBuf;

use oer_hil_schema::dependency::Dependency;
use serde::{Deserialize, Serialize};

use crate::Result;

/// Names the experiment a runner process runs for, as JSON.
pub const EXPERIMENT_ENV: &str = "OER_HIL_EXPERIMENT";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Experiment {
    pub id: String,
    pub arm: Arm,
    pub variant: Variant,
    pub round: Round,
}

/// One round of an experiment: both arms, one after the other, on one
/// layout seed.
///
/// Round 0 of a layout seed is the [`Phase::Preparation`]: it builds each
/// arm's images and warms the stand up, each arm under its own lease, and
/// no analysis pairs it. The measured rounds `1..=repetitions` replay those
/// images, both arms under one stand lease; the two arms' runs of one
/// measured round are a pair.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Round {
    /// The layout seed both arms' images were built with.
    pub layout_seed: u32,
    /// The round's position among the rounds of its layout seed: 0 for the
    /// preparation, `1..=repetitions` for the measured rounds.
    pub index: u32,
    /// What the round is for: preparation (index 0) or measurement.
    pub phase: Phase,
    /// Which arm ran first.
    pub order: Order,
    /// The seed the experiment drew every round's order from.
    pub order_seed: u64,
}

/// What an experiment round is for.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Phase {
    /// Round 0: build each arm's images and warm up; never paired.
    Preparation,
    /// A measured round: both arms under one lease, paired.
    Measurement,
}

impl Phase {
    /// The phase of round `index`.
    pub const fn of(index: u32) -> Self {
        if index == 0 {
            Self::Preparation
        } else {
            Self::Measurement
        }
    }
}

/// The order of the two arms in one round.
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Order {
    /// A, then B.
    Ab,
    /// B, then A.
    Ba,
}

impl Order {
    /// The arms in the order they run.
    pub const fn arms(self) -> [Arm; 2] {
        match self {
            Self::Ab => [Arm::A, Arm::B],
            Self::Ba => [Arm::B, Arm::A],
        }
    }

    /// The order whose first arm is `B` when `swapped`.
    pub const fn swapped(swapped: bool) -> Self {
        if swapped { Self::Ba } else { Self::Ab }
    }
}

impl std::fmt::Display for Order {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Ab => "AB",
            Self::Ba => "BA",
        })
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Arm {
    A,
    B,
}

impl std::fmt::Display for Arm {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::A => "A",
            Self::B => "B",
        })
    }
}

/// What an arm builds: a commit of the repository and the local checkouts
/// that replace its pinned dependencies.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Variant {
    /// The repository commit, resolved.
    pub commit: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub overrides: Vec<DependencyOverride>,
    /// Runtime features added to or removed from each image class's own.
    #[serde(
        default,
        skip_serializing_if = "oer_hil_schema::image::FeatureDelta::is_empty"
    )]
    pub features: oer_hil_schema::image::FeatureDelta,
}

/// A local checkout that replaces a pinned dependency, as the
/// `ESP_HAL_ROOT` root does.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DependencyOverride {
    pub dependency: Dependency,
    pub path: PathBuf,
    /// The checkout's commit when the experiment started.
    pub commit: String,
    /// Whether it had uncommitted changes; the source snapshot keeps them.
    pub dirty: bool,
}

impl Experiment {
    /// The experiment this process runs for, if [`EXPERIMENT_ENV`] names one.
    pub fn from_environment() -> Result<Option<Self>> {
        match std::env::var(EXPERIMENT_ENV) {
            Ok(json) => Ok(Some(serde_json::from_str(&json).map_err(|error| {
                format!("{EXPERIMENT_ENV} is not an experiment: {error}")
            })?)),
            Err(_) => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_experiment_round_trips_through_its_environment_form() {
        let experiment = Experiment {
            id: String::from("1790-1"),
            arm: Arm::B,
            variant: Variant {
                commit: String::from("abc"),
                overrides: vec![DependencyOverride {
                    dependency: Dependency::EspHal,
                    path: PathBuf::from("/src/esp-hal"),
                    commit: String::from("ea9385e"),
                    dirty: false,
                }],
                features: "+trace".parse().unwrap(),
            },
            round: Round {
                layout_seed: 1,
                index: 2,
                phase: Phase::Measurement,
                order: Order::Ba,
                order_seed: 7,
            },
        };
        let json = serde_json::to_string(&experiment).unwrap();
        assert!(json.contains(r#""arm":"b""#), "{json}");
        assert!(json.contains(r#""dependency":"esp-hal""#), "{json}");
        assert!(json.contains(r#""phase":"measurement""#), "{json}");
        assert_eq!(
            serde_json::from_str::<Experiment>(&json).unwrap(),
            experiment
        );
    }
}
