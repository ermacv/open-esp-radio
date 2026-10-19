//! The comparison of an A/B experiment's arms.
//!
//! Each run contributes one value per measurement, the mean of its
//! repetitions' [`samples`]. An experiment runs in rounds: per layout seed a
//! preparation round (builds and warm-up, never paired), then measured
//! rounds, both arms in the round's recorded order under one stand lease.
//! The two arms' values of one measured round are a pair, so drift between
//! rounds falls on both values of a pair alike and cancels in their
//! difference; each measurement is judged by the interval of the paired
//! differences ([`samples::compare_paired`]). A round in which only one arm
//! measured a metric (a run that ended before its verdict, a repetition the
//! [`samples::REPETITION_POLICY`] excluded) pairs with nothing and is
//! counted as unpaired, not compared against the other arm's other rounds;
//! a metric without one complete pair is reported as [`Paired::NoPairs`].
//!
//! [`compare`] takes a [`ValidatedExperiment`]: runs of one experiment (one
//! id and order seed), each arm on one variant, each round with one order
//! and the phase its index has. Runs of two experiments never pair.

use std::collections::BTreeMap;

use oer_hil_run_bundle_format::experiment::{Arm, Experiment, Order, Phase, Round, Variant};
use oer_hil_run_bundle_format::run::MetricId;
use serde::Serialize;

use crate::{
    Result,
    run::Run,
    samples::{self, Better, Compared},
};

/// The runs of one A/B experiment, checked to belong together.
pub struct ValidatedExperiment<'a> {
    id: String,
    order_seed: u64,
    runs: Vec<(&'a Run, Experiment)>,
}

impl<'a> ValidatedExperiment<'a> {
    /// `runs` as one experiment: an error unless there is a run, every run
    /// records an experiment round, all record one experiment id and order
    /// seed, every run of an arm records one variant, the runs of one round
    /// record one order, and every round's phase is its index's
    /// (preparation for round 0).
    pub fn new(runs: &[&'a Run]) -> Result<Self> {
        let mut checked: Vec<(&'a Run, Experiment)> = Vec::new();
        let mut variants: BTreeMap<Arm, Variant> = BTreeMap::new();
        let mut orders: BTreeMap<(u32, u32), Order> = BTreeMap::new();
        for run in runs {
            let experiment = run
                .bundle
                .manifest()
                .experiment
                .clone()
                .ok_or_else(|| format!("run {} records no experiment round", run.id()))?;
            if let Some((first, known)) = checked.first() {
                if experiment.id != known.id {
                    return Err(format!(
                        "run {} belongs to experiment {}, run {} to experiment {}",
                        run.id(),
                        experiment.id,
                        first.id(),
                        known.id
                    )
                    .into());
                }
                if experiment.round.order_seed != known.round.order_seed {
                    return Err(format!(
                        "run {} records order seed {}, run {} order seed {}",
                        run.id(),
                        experiment.round.order_seed,
                        first.id(),
                        known.round.order_seed
                    )
                    .into());
                }
            }
            if let Some(variant) = variants.get(&experiment.arm)
                && *variant != experiment.variant
            {
                return Err(format!(
                    "run {} measured arm {} on another variant than the arm's other runs",
                    run.id(),
                    experiment.arm
                )
                .into());
            }
            variants.insert(experiment.arm, experiment.variant.clone());
            let Round {
                layout_seed,
                index,
                phase,
                order,
                ..
            } = experiment.round;
            if phase != Phase::of(index) {
                return Err(format!(
                    "run {} records round {index} of layout seed {layout_seed} in the {phase:?} \
                     phase",
                    run.id()
                )
                .into());
            }
            let key = (layout_seed, index);
            if *orders.entry(key).or_insert(order) != order {
                return Err(format!(
                    "run {} records round {index} of layout seed {layout_seed} as {order}, \
                     another run of that round as {}",
                    run.id(),
                    orders[&key]
                )
                .into());
            }
            checked.push((*run, experiment));
        }
        let (id, order_seed) = checked
            .first()
            .map(|(_, experiment)| (experiment.id.clone(), experiment.round.order_seed))
            .ok_or("an experiment without runs compares nothing")?;
        Ok(Self {
            id,
            order_seed,
            runs: checked,
        })
    }

    /// The experiment's id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// The seed every round's order was drawn from.
    pub fn order_seed(&self) -> u64 {
        self.order_seed
    }
}

/// One compared measurement.
#[derive(Clone, Debug, Serialize)]
pub struct MeasurementComparison {
    pub metric: MetricId,
    /// Which way the metric improves; `None` for a figure without a
    /// declared direction, whose significant differences are reported as
    /// changes.
    pub better: Option<Better>,
    pub comparison: Paired,
    /// Every pair the comparison used.
    pub pairs: Vec<Pair>,
    /// Measured rounds in which only one arm measured the metric.
    pub unpaired_rounds: usize,
}

/// The paired comparison of one measurement, or why there is none.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "kebab-case", tag = "result", content = "compared")]
pub enum Paired {
    Compared(Compared),
    /// No measured round has both arms' values: nothing to compare.
    NoPairs,
}

impl Paired {
    /// The comparison, when there is one.
    pub fn compared(&self) -> Option<&Compared> {
        match self {
            Self::Compared(compared) => Some(compared),
            Self::NoPairs => None,
        }
    }
}

/// The two arms' values of one metric in one measured round.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct Pair {
    pub layout_seed: u32,
    pub round: u32,
    pub order: Order,
    pub a: f64,
    pub b: f64,
}

/// A round's order and its A's and B's run means.
type RoundMeans = (Order, Vec<f64>, Vec<f64>);

/// One metric's run means per measured round and arm.
struct Rounds {
    metric: MetricId,
    /// (layout seed, round index) -> the round's order and run means.
    rounds: BTreeMap<(u32, u32), RoundMeans>,
}

fn mean(values: &[f64]) -> Option<f64> {
    (!values.is_empty()).then(|| values.iter().sum::<f64>() / values.len() as f64)
}

/// The paired comparison of every measurement of `experiment`'s measured
/// rounds; the preparation rounds are left out. An error when two runs
/// report one scenario's measurement as different metrics (unit, semantics
/// or direction), which are never combined.
pub fn compare(experiment: &ValidatedExperiment<'_>) -> Result<Vec<MeasurementComparison>> {
    let mut metrics: BTreeMap<(String, String), Rounds> = BTreeMap::new();
    for (run, experiment) in &experiment.runs {
        let Round {
            layout_seed,
            index,
            phase,
            order,
            ..
        } = experiment.round;
        if phase == Phase::Preparation {
            continue;
        }
        let Some(suite) = &run.suite else {
            continue;
        };
        let samples =
            samples::samples(suite).map_err(|error| format!("run {}: {error}", run.id()))?;
        for sample in samples {
            let Some(spread) = sample.spread() else {
                continue;
            };
            let entry = metrics
                .entry((sample.metric.scenario.clone(), sample.metric.name.clone()))
                .or_insert_with(|| Rounds {
                    metric: sample.metric.clone(),
                    rounds: BTreeMap::new(),
                });
            if let Some(reason) = entry.metric.incompatibility(&sample.metric) {
                return Err(format!(
                    "run {} ({}) reports {} as another metric than earlier runs: {reason}",
                    run.id(),
                    experiment.arm,
                    entry.metric
                )
                .into());
            }
            let round = entry
                .rounds
                .entry((layout_seed, index))
                .or_insert_with(|| (order, Vec::new(), Vec::new()));
            match experiment.arm {
                Arm::A => round.1.push(spread.mean),
                Arm::B => round.2.push(spread.mean),
            }
        }
    }
    Ok(metrics
        .into_values()
        .map(|Rounds { metric, rounds }| {
            let mut pairs = Vec::new();
            let mut unpaired_rounds = 0;
            for ((layout_seed, round), (order, a, b)) in rounds {
                match (mean(&a), mean(&b)) {
                    (Some(a), Some(b)) => pairs.push(Pair {
                        layout_seed,
                        round,
                        order,
                        a,
                        b,
                    }),
                    _ => unpaired_rounds += 1,
                }
            }
            let values = pairs
                .iter()
                .map(|pair| (pair.a, pair.b))
                .collect::<Vec<_>>();
            MeasurementComparison {
                comparison: samples::compare_paired(metric.better, &values)
                    .map_or(Paired::NoPairs, Paired::Compared),
                better: metric.better,
                metric,
                pairs,
                unpaired_rounds,
            }
        })
        .collect())
}

#[cfg(test)]
mod tests;
