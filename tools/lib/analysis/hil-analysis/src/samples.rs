//! The one aggregation of a run's measurements and the one comparison of
//! two sets of values.
//!
//! [`samples`] groups a suite's typed measurements by [`MetricId`], one
//! value per admitted repetition that measured it; every analysis reads
//! figures through it: `runs compare` and `history`, performance reports
//! and baselines, and A/B comparisons. Values of one scenario and name
//! whose unit, semantics version or improvement direction differ are never
//! combined: [`samples`] fails on them. [`compare`] judges two sets of
//! values with Welch's unequal-variance interval (the exact Student-t
//! quantile of `oer-stats`) and a practical tolerance, [`compare_paired`]
//! the differences of paired rounds, and [`change`] a set against a
//! reviewed baseline.
//!
//! Which repetitions enter the statistics is the declared
//! [`REPETITION_POLICY`]: a repetition that ran to its verdict, passed or
//! failed, measured what it measured, and a failed gate is itself the
//! observation a performance comparison must see; a repetition that broke,
//! was interrupted, quarantined its board, was blocked or skipped did not
//! finish its workload, so its values are partial and only counted as
//! excluded.

use std::collections::BTreeMap;

use oer_hil_run_bundle_format::experiment::Arm;
use oer_hil_run_bundle_format::run::MetricId;
use oer_hil_run_bundle_format::run::Outcome;
use oer_hil_run_bundle_format::run::SuiteResult;
use oer_hil_run_bundle_format::run::Threshold;
use serde::{Deserialize, Serialize};

pub use oer_hil_run_bundle_format::run::Better;

use crate::Result;

/// Which repetitions' values enter the statistics.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RepetitionPolicy {
    /// Repetitions that reached a verdict: `passed` or `failed`.
    Concluded,
}

impl RepetitionPolicy {
    pub const fn admits(self, outcome: Outcome) -> bool {
        match self {
            Self::Concluded => matches!(outcome, Outcome::Passed | Outcome::Failed),
        }
    }
}

/// The policy every analysis applies.
pub const REPETITION_POLICY: RepetitionPolicy = RepetitionPolicy::Concluded;

/// The gate of a performance figure: the direction an `at-least` or
/// `at-most` threshold prefers and its value. An `exactly` threshold is a
/// correctness check, not a performance figure, and gives no gate.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct Gate {
    pub better: Better,
    pub threshold: f64,
}

impl Gate {
    pub fn of(threshold: &Threshold) -> Option<Self> {
        Some(Self {
            better: threshold.comparison.better()?,
            threshold: threshold.value as f64,
        })
    }
}

/// The values of one metric of one scenario in one run, one per admitted
/// repetition that measured it, in repetition order.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Sample {
    pub metric: MetricId,
    /// The gate every admitted repetition measured it against; `None` for
    /// an ungated figure.
    pub gate: Option<Gate>,
    pub values: Vec<f64>,
    /// Repetitions that measured it but [`REPETITION_POLICY`] excluded.
    pub excluded: usize,
}

impl Sample {
    pub fn better(&self) -> Option<Better> {
        self.metric.better
    }

    pub fn spread(&self) -> Option<Spread> {
        Spread::of(&self.values)
    }
}

/// Every metric of every scenario of `suite`, ordered by scenario and name;
/// an error when one scenario reports a name under two metric identities
/// or two gates.
pub fn samples(suite: &SuiteResult) -> Result<Vec<Sample>> {
    let mut samples: BTreeMap<(&str, &str), Sample> = BTreeMap::new();
    for scenario in &suite.scenarios {
        for repetition in &scenario.repetitions {
            let admitted = REPETITION_POLICY.admits(repetition.outcome);
            for measurement in &repetition.measurements {
                let metric = measurement.metric(&scenario.scenario);
                let gate = measurement.threshold.as_ref().and_then(Gate::of);
                let sample = samples
                    .entry((&scenario.scenario, &measurement.name))
                    .or_insert_with(|| Sample {
                        metric: metric.clone(),
                        gate,
                        values: Vec::new(),
                        excluded: 0,
                    });
                if let Some(reason) = sample.metric.incompatibility(&metric) {
                    return Err(format!(
                        "repetition {} reports {} as another metric than earlier repetitions: \
                         {reason}",
                        repetition.repetition, sample.metric
                    )
                    .into());
                }
                if sample.gate != gate {
                    return Err(format!(
                        "repetition {} gates {} differently from earlier repetitions",
                        repetition.repetition, sample.metric
                    )
                    .into());
                }
                if admitted {
                    sample.values.push(measurement.value as f64);
                } else {
                    sample.excluded += 1;
                }
            }
        }
    }
    Ok(samples.into_values().collect())
}

/// Mean and sample standard deviation.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct Spread {
    pub mean: f64,
    pub deviation: f64,
    pub count: usize,
}

impl Spread {
    pub fn of(values: &[f64]) -> Option<Self> {
        if values.is_empty() {
            return None;
        }
        let count = values.len();
        let mean = values.iter().sum::<f64>() / count as f64;
        let deviation = if count > 1 {
            (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (count - 1) as f64).sqrt()
        } else {
            0.0
        };
        Some(Self {
            mean,
            deviation,
            count,
        })
    }
}

/// What a comparison of two sets of values concludes.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Verdict {
    /// The 95 % confidence interval of the difference excludes zero and the
    /// difference is at least 2 % of A's mean.
    Significant { better: Arm },
    /// As [`Self::Significant`], for a measurement without a gate: nothing
    /// says which direction is better, so only the higher side is named.
    Changed { higher: Arm },
    /// The interval includes zero, or the difference is too small to matter.
    WithinNoise,
    /// Fewer than [`MINIMUM_REPETITIONS`] values on a side, or pairs.
    InsufficientRepetitions,
}

impl std::fmt::Display for Verdict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Significant { better } => write!(f, "significant, {better} better"),
            Self::Changed { higher } => write!(f, "significant change, {higher} higher (ungated)"),
            Self::WithinNoise => f.write_str("within noise"),
            Self::InsufficientRepetitions => f.write_str("insufficient repetitions"),
        }
    }
}

/// The fewest values per side (or pairs) from which a comparison is judged.
pub const MINIMUM_REPETITIONS: usize = 3;

/// How a comparison's interval was estimated.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind")]
pub enum Design {
    /// Independent samples: Welch's unequal-variance interval at its
    /// fractional degrees of freedom (`None` when both sides are constant
    /// and the interval is empty).
    Independent { freedom: Option<f64> },
    /// Paired rounds: the one-sample interval of the per-pair differences
    /// B − A, with `pairs − 1` degrees of freedom.
    Paired { pairs: usize },
}

/// A comparison of one measurement: both sides, the difference B − A with
/// its 95 % confidence interval, how that interval was estimated, and the
/// verdict.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
pub struct Compared {
    pub a: Spread,
    pub b: Spread,
    pub difference: f64,
    /// Half-width of the 95 % confidence interval of `difference`.
    pub interval: f64,
    pub design: Design,
    pub verdict: Verdict,
}

/// Confidence of every interval [`compare`] and [`compare_paired`] give.
pub const CONFIDENCE: f64 = 0.95;

/// Relative tolerance below which a change is noise even for perfectly
/// repeatable values.
const MINIMUM_TOLERANCE: f64 = 0.02;

/// Half-width of the [`CONFIDENCE`] interval with `freedom` degrees of
/// freedom and `standard_error`. A zero standard error gives an empty
/// interval; degrees of freedom the quantile cannot take (never positive
/// here, since every caller has two or more values with a spread) give an
/// interval that spans everything, so nothing is judged significant.
fn half_width(standard_error: f64, freedom: f64) -> f64 {
    if standard_error == 0.0 {
        0.0
    } else {
        oer_stats::student_t_critical(CONFIDENCE, freedom)
            .map_or(f64::INFINITY, |critical| critical * standard_error)
    }
}

/// Compare the values of sides A and B of one measurement whose better
/// direction is `better` (`None` for an ungated measurement), with Welch's
/// unequal-variance t interval: the values of the two sides are independent
/// samples.
pub fn compare(better: Option<Better>, a: &[f64], b: &[f64]) -> Option<Compared> {
    let (spread_a, spread_b) = (Spread::of(a)?, Spread::of(b)?);
    let difference = spread_b.mean - spread_a.mean;
    let (variance_a, variance_b) = (
        spread_a.deviation.powi(2) / spread_a.count as f64,
        spread_b.deviation.powi(2) / spread_b.count as f64,
    );
    let standard_error = (variance_a + variance_b).sqrt();
    let freedom = (standard_error != 0.0).then(|| {
        (variance_a + variance_b).powi(2)
            / (variance_a.powi(2) / (spread_a.count as f64 - 1.0).max(1.0)
                + variance_b.powi(2) / (spread_b.count as f64 - 1.0).max(1.0))
    });
    let interval = freedom.map_or(0.0, |freedom| half_width(standard_error, freedom));
    let enough = spread_a.count >= MINIMUM_REPETITIONS && spread_b.count >= MINIMUM_REPETITIONS;
    Some(Compared {
        a: spread_a,
        b: spread_b,
        difference,
        interval,
        design: Design::Independent { freedom },
        verdict: verdict(better, enough, difference, interval, spread_a.mean),
    })
}

/// Compare the paired values `(a, b)` of one measurement, one pair per
/// round that ran both arms under the same conditions, by the interval of
/// the differences B − A; `None` without a pair.
pub fn compare_paired(better: Option<Better>, pairs: &[(f64, f64)]) -> Option<Compared> {
    let a = pairs.iter().map(|(a, _)| *a).collect::<Vec<_>>();
    let b = pairs.iter().map(|(_, b)| *b).collect::<Vec<_>>();
    let differences = pairs.iter().map(|(a, b)| b - a).collect::<Vec<_>>();
    let (spread_a, spread_b, spread_d) =
        (Spread::of(&a)?, Spread::of(&b)?, Spread::of(&differences)?);
    let standard_error = spread_d.deviation / (spread_d.count as f64).sqrt();
    let interval = if spread_d.count > 1 {
        half_width(standard_error, spread_d.count as f64 - 1.0)
    } else {
        f64::INFINITY
    };
    let enough = spread_d.count >= MINIMUM_REPETITIONS;
    Some(Compared {
        a: spread_a,
        b: spread_b,
        difference: spread_d.mean,
        interval,
        design: Design::Paired {
            pairs: spread_d.count,
        },
        verdict: verdict(better, enough, spread_d.mean, interval, spread_a.mean),
    })
}

/// The verdict on a difference B − A with the half-width `interval` of its
/// confidence interval, relative to A's mean `reference`.
fn verdict(
    better: Option<Better>,
    enough: bool,
    difference: f64,
    interval: f64,
    reference: f64,
) -> Verdict {
    if !enough {
        return Verdict::InsufficientRepetitions;
    }
    if difference.abs() <= interval || difference.abs() < MINIMUM_TOLERANCE * reference.abs() {
        return Verdict::WithinNoise;
    }
    let arm = |b_wins: bool| if b_wins { Arm::B } else { Arm::A };
    match better {
        Some(Better::Higher) => Verdict::Significant {
            better: arm(difference > 0.0),
        },
        Some(Better::Lower) => Verdict::Significant {
            better: arm(difference < 0.0),
        },
        None => Verdict::Changed {
            higher: arm(difference > 0.0),
        },
    }
}

/// How a figure compares with its reviewed baseline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Change {
    Improved,
    Within,
    Regressed,
}

/// Compare `current` with a baseline `reference` of a figure that `better`
/// prefers: a move in the worse direction by more than twice the baseline's
/// deviation and 2 % of its mean is a regression; the same margin in the
/// better direction is an improvement.
pub fn change(reference: &Spread, better: Better, current: &Spread) -> Change {
    let margin = (2.0 * reference.deviation).max(MINIMUM_TOLERANCE * reference.mean.abs());
    let delta = current.mean - reference.mean;
    let worse = match better {
        Better::Higher => -delta,
        Better::Lower => delta,
    };
    if worse > margin {
        Change::Regressed
    } else if -worse > margin {
        Change::Improved
    } else {
        Change::Within
    }
}

#[cfg(test)]
pub(crate) mod tests;
