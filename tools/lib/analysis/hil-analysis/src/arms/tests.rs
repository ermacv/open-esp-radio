use std::path::Path;

use oer_hil_run_bundle::run::test_support::repetition;
use oer_hil_run_bundle::run::test_support::write_run;
use oer_hil_run_bundle_format::experiment::{Experiment, Variant};
use oer_hil_run_bundle_format::run::Comparison;
use oer_hil_run_bundle_format::run::Measurement;
use oer_hil_run_bundle_format::run::MeasurementUnit;
use oer_hil_run_bundle_format::run::Outcome;
use oer_hil_run_bundle_format::run::RunState;
use oer_hil_run_bundle_format::run::ScenarioResult;
use oer_hil_schema::image::ImageClass;

use super::*;
use crate::samples::Verdict;

/// Where a test run belongs: its experiment, arm, variant and round.
#[derive(Clone)]
struct In {
    experiment: &'static str,
    arm: Arm,
    commit: &'static str,
    index: u32,
    order_seed: u64,
}

impl In {
    fn round(arm: Arm, index: u32) -> Self {
        Self {
            experiment: "x",
            arm,
            commit: match arm {
                Arm::A => "a",
                Arm::B => "b",
            },
            index,
            order_seed: 7,
        }
    }
}

/// A completed run `id` below `runs` of `udp-rx` in `place` whose
/// repetitions measured `throughput` (gated, higher is better) and an
/// ungated `counter`.
fn run(runs: &Path, id: &str, place: In, throughput: &[u64], counter: u64) -> Run {
    let repetitions = throughput
        .iter()
        .enumerate()
        .map(|(index, value)| {
            repetition(
                "udp-rx",
                index as u8 + 1,
                Outcome::Passed,
                None,
                vec![
                    Measurement::observed("rx.bps", *value, MeasurementUnit::BitsPerSecond)
                        .evaluated(Comparison::AtLeast, 1),
                    Measurement::observed("rx.zero-copy", counter, MeasurementUnit::Count),
                ],
            )
        })
        .collect();
    write_run(
        &runs.join(id),
        0,
        RunState::Completed,
        vec![ScenarioResult::from_repetitions(
            "udp-rx".into(),
            ImageClass::DiagnosticUsbJtagOff,
            throughput.len() as u8,
            repetitions,
        )],
        |manifest| {
            manifest.experiment = Some(Experiment {
                id: place.experiment.into(),
                arm: place.arm,
                variant: Variant {
                    commit: place.commit.into(),
                    overrides: Vec::new(),
                    features: Default::default(),
                },
                round: Round {
                    layout_seed: 1,
                    index: place.index,
                    phase: Phase::of(place.index),
                    order: Order::Ab,
                    order_seed: place.order_seed,
                },
            });
        },
    );
    Run::load(&runs.join(id)).unwrap()
}

fn refs(runs: &[Run]) -> Vec<&Run> {
    runs.iter().collect()
}

#[test]
fn measured_rounds_pair_and_the_preparation_round_is_left_out() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path();
    let runs = [
        // The preparation round differs wildly: it must not count.
        run(path, "a0", In::round(Arm::A, 0), &[1], 5),
        run(path, "b0", In::round(Arm::B, 0), &[1000], 5),
        run(path, "a1", In::round(Arm::A, 1), &[100, 102], 5),
        run(path, "b1", In::round(Arm::B, 1), &[110, 112], 5),
        run(path, "a2", In::round(Arm::A, 2), &[101], 5),
        run(path, "b2", In::round(Arm::B, 2), &[111], 5),
    ];
    let experiment = ValidatedExperiment::new(&refs(&runs)).unwrap();
    assert_eq!((experiment.id(), experiment.order_seed()), ("x", 7));
    let comparisons = compare(&experiment).unwrap();
    let rx = comparisons
        .iter()
        .find(|entry| entry.metric.name == "rx.bps")
        .unwrap();
    assert_eq!(rx.metric.unit, MeasurementUnit::BitsPerSecond);
    assert_eq!(rx.better, Some(Better::Higher));
    assert_eq!(
        rx.pairs.iter().map(|pair| pair.round).collect::<Vec<_>>(),
        [1, 2]
    );
    let compared = rx.comparison.compared().unwrap();
    // One value per run: the mean of its repetitions.
    assert_eq!((compared.a.mean, compared.a.count), (101.0, 2));
    assert_eq!((compared.b.mean, compared.b.count), (111.0, 2));
    assert_eq!(compared.difference, 10.0);
    // Two pairs are too few to judge.
    assert_eq!(compared.verdict, Verdict::InsufficientRepetitions);
    let counter = comparisons
        .iter()
        .find(|entry| entry.metric.name == "rx.zero-copy")
        .unwrap();
    assert_eq!(counter.better, None);
}

#[test]
fn runs_of_two_experiments_never_pair() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path();
    let other = In {
        experiment: "y",
        ..In::round(Arm::B, 1)
    };
    let runs = [
        run(path, "a1", In::round(Arm::A, 1), &[100], 5),
        run(path, "b1", other, &[110], 5),
    ];
    let error = ValidatedExperiment::new(&refs(&runs))
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("experiment"), "{error}");

    let reseeded = In {
        order_seed: 8,
        ..In::round(Arm::B, 1)
    };
    let runs = [
        run(path, "c1", In::round(Arm::A, 1), &[100], 5),
        run(path, "d1", reseeded, &[110], 5),
    ];
    let error = ValidatedExperiment::new(&refs(&runs))
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("order seed"), "{error}");
}

#[test]
fn an_arm_measured_on_two_variants_is_refused() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path();
    let moved = In {
        commit: "a-prime",
        ..In::round(Arm::A, 2)
    };
    let runs = [
        run(path, "a1", In::round(Arm::A, 1), &[100], 5),
        run(path, "a2", moved, &[100], 5),
    ];
    let error = ValidatedExperiment::new(&refs(&runs))
        .err()
        .unwrap()
        .to_string();
    assert!(error.contains("variant"), "{error}");
}

#[test]
fn a_metric_without_a_complete_pair_is_reported_as_no_pairs() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path();
    let runs = [
        run(path, "a0", In::round(Arm::A, 0), &[100], 5),
        run(path, "b0", In::round(Arm::B, 0), &[110], 5),
        run(path, "a1", In::round(Arm::A, 1), &[100], 5),
    ];
    let comparisons = compare(&ValidatedExperiment::new(&refs(&runs)).unwrap()).unwrap();
    let rx = comparisons
        .iter()
        .find(|entry| entry.metric.name == "rx.bps")
        .unwrap();
    assert!(matches!(rx.comparison, Paired::NoPairs));
    assert_eq!(rx.unpaired_rounds, 1);
    assert!(ValidatedExperiment::new(&[]).is_err());
}
