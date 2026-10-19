use oer_hil_run_bundle::run::test_support::repetition;
use oer_hil_run_bundle::run::test_support::write_run;
use oer_hil_run_bundle_format::run::Measurement;
use oer_hil_run_bundle_format::run::MeasurementUnit;
use oer_hil_run_bundle_format::run::Outcome;
use oer_hil_run_bundle_format::run::RunState;
use oer_hil_run_bundle_format::run::ScenarioResult;
use oer_hil_run_bundle_format::run::Threshold;
use oer_hil_schema::image::ImageClass;
use oer_hil_schema::run::Comparison;

use super::*;

pub(crate) fn rate(value: u64) -> Measurement {
    Measurement::observed("udp.tx.host-rate", value, MeasurementUnit::BitsPerSecond)
        .evaluated(Comparison::AtLeast, 1_000_000)
}

/// A suite of one `udp-tx` scenario whose repetitions measured `values` and
/// an ungated count.
pub(crate) fn suite(values: &[u64]) -> SuiteResult {
    let directory = tempfile::tempdir().unwrap();
    let run = directory.path().join("1-a");
    let repetitions = values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            repetition(
                "udp-tx",
                index as u8 + 1,
                Outcome::Passed,
                None,
                vec![
                    rate(*value),
                    Measurement::observed("ungated", 1, MeasurementUnit::Count),
                ],
            )
        })
        .collect();
    write_run(
        &run,
        1,
        RunState::Completed,
        vec![ScenarioResult::from_repetitions(
            "udp-tx".into(),
            ImageClass::SystemPanicReset,
            values.len() as u8,
            repetitions,
        )],
        |_| {},
    );
    oer_hil_run_bundle_format::RunBundle::open(&run)
        .unwrap()
        .unwrap()
        .suite()
        .unwrap()
        .unwrap()
}

#[test]
fn every_measurement_is_one_sample_per_scenario_and_name_with_its_gate() {
    let samples = samples(&suite(&[115_000_000, 116_000_000])).unwrap();
    assert_eq!(samples.len(), 2);
    let rate = samples
        .iter()
        .find(|sample| sample.metric.name == "udp.tx.host-rate")
        .unwrap();
    assert_eq!(rate.values, [115e6, 116e6]);
    assert_eq!(rate.better(), Some(Better::Higher));
    assert_eq!(rate.gate.unwrap().threshold, 1e6);
    let ungated = samples
        .iter()
        .find(|sample| sample.metric.name == "ungated")
        .unwrap();
    assert_eq!(ungated.gate, None);
    assert_eq!(ungated.values, [1.0, 1.0]);
    let gate = |comparison| Threshold {
        comparison,
        value: 3,
    };
    assert_eq!(
        Gate::of(&gate(Comparison::AtMost)).unwrap().better,
        Better::Lower
    );
    assert_eq!(Gate::of(&gate(Comparison::Exactly)), None);
}

#[test]
fn a_comparison_needs_repetitions_and_a_difference_beyond_its_interval() {
    let verdict = |better, a: &[f64], b: &[f64]| compare(better, a, b).unwrap().verdict;
    // Two values a side are too few to judge.
    assert_eq!(
        verdict(Some(Better::Higher), &[100.0, 101.0], &[120.0, 121.0]),
        Verdict::InsufficientRepetitions
    );
    // A clear gain in the better direction, and the same data read with
    // the opposite direction.
    let a = [100.0, 101.0, 99.0, 100.5];
    let b = [110.0, 111.0, 109.5, 110.5];
    assert_eq!(
        verdict(Some(Better::Higher), &a, &b),
        Verdict::Significant { better: Arm::B }
    );
    assert_eq!(
        verdict(Some(Better::Lower), &a, &b),
        Verdict::Significant { better: Arm::A }
    );
    // Overlapping noisy arms stay within noise.
    assert_eq!(
        verdict(
            Some(Better::Higher),
            &[100.0, 110.0, 90.0],
            &[103.0, 95.0, 112.0]
        ),
        Verdict::WithinNoise
    );
    // Perfectly repeatable but below the practical tolerance.
    assert_eq!(
        verdict(Some(Better::Higher), &[100.0; 3], &[101.0; 3]),
        Verdict::WithinNoise
    );
    // Without a gate the same gain is a change, not an improvement.
    assert_eq!(verdict(None, &a, &b), Verdict::Changed { higher: Arm::B });
    let comparison = compare(Some(Better::Higher), &a, &b).unwrap();
    assert!((comparison.difference - 10.125).abs() < 1e-9);
    assert!(comparison.interval > 0.0 && comparison.interval < comparison.difference);
    assert!(compare(Some(Better::Higher), &[], &b).is_none());
}

#[test]
fn a_change_is_judged_against_the_baseline_noise_in_the_better_direction() {
    let baseline = Spread::of(&[115e6, 116e6, 117e6]).unwrap();
    let within = Spread::of(&[114e6]).unwrap();
    let lower = Spread::of(&[105e6]).unwrap();
    let higher = Spread::of(&[125e6]).unwrap();
    assert_eq!(change(&baseline, Better::Higher, &within), Change::Within);
    assert_eq!(change(&baseline, Better::Higher, &lower), Change::Regressed);
    assert_eq!(change(&baseline, Better::Higher, &higher), Change::Improved);
    assert_eq!(change(&baseline, Better::Lower, &higher), Change::Regressed);
    assert_eq!(change(&baseline, Better::Lower, &lower), Change::Improved);
}

#[test]
fn welch_uses_the_exact_quantile_at_its_fractional_freedom() {
    // n = 17 a side: about 32 degrees of freedom, where the old table's
    // 2.000 was too narrow and called this difference significant.
    let a = (200..=1800).step_by(100).map(f64::from).collect::<Vec<_>>();
    let b = a.iter().map(|value| value + 350.0).collect::<Vec<_>>();
    let compared = compare(Some(Better::Higher), &a, &b).unwrap();
    assert!((compared.difference - 350.0).abs() < 1e-9);
    assert!(
        (compared.interval - 352.807).abs() < 1e-3,
        "{}",
        compared.interval
    );
    assert_eq!(compared.verdict, Verdict::WithinNoise);
    let Design::Independent {
        freedom: Some(freedom),
    } = compared.design
    else {
        panic!("{:?}", compared.design);
    };
    assert!((freedom - 32.0).abs() < 1e-9, "{freedom}");
}

#[test]
fn paired_rounds_cancel_a_drift_that_hides_the_difference_from_welch() {
    // Both arms drift together from round to round; B is 10 above A with
    // little noise.
    let pairs = [0.0, 40.0, 80.0, 120.0, 160.0]
        .iter()
        .zip([0.0, 0.5, -0.5, 0.25, -0.25])
        .map(|(drift, noise)| (100.0 + drift, 110.0 + drift + noise))
        .collect::<Vec<_>>();
    let paired = compare_paired(Some(Better::Higher), &pairs).unwrap();
    assert_eq!(paired.verdict, Verdict::Significant { better: Arm::B });
    assert_eq!(paired.design, Design::Paired { pairs: 5 });
    assert!((paired.difference - 10.0).abs() < 1e-9);
    let (a, b): (Vec<f64>, Vec<f64>) = pairs.iter().copied().unzip();
    assert_eq!(
        compare(Some(Better::Higher), &a, &b).unwrap().verdict,
        Verdict::WithinNoise
    );
}

#[test]
fn fewer_than_three_pairs_are_not_judged() {
    let pairs = [(100.0, 110.0), (101.0, 111.0)];
    let paired = compare_paired(Some(Better::Higher), &pairs).unwrap();
    assert_eq!(paired.verdict, Verdict::InsufficientRepetitions);
    assert_eq!(paired.design, Design::Paired { pairs: 2 });
    assert!(compare_paired(Some(Better::Higher), &[]).is_none());
}

#[test]
fn concluded_repetitions_enter_the_statistics_and_the_others_are_counted() {
    assert!(REPETITION_POLICY.admits(Outcome::Passed));
    assert!(REPETITION_POLICY.admits(Outcome::Failed));
    assert!(!REPETITION_POLICY.admits(Outcome::Broken));
    assert!(!REPETITION_POLICY.admits(Outcome::Interrupted));

    let measured = |number, outcome, value| {
        let failure = (outcome != Outcome::Passed).then(|| {
            oer_hil_run_bundle_format::run::Failure::new(
                oer_hil_run_bundle_format::run::FailureKind::Scenario,
                "ended early",
            )
        });
        repetition("udp-tx", number, outcome, failure, vec![rate(value)])
    };
    let suite = suite_of(vec![
        measured(1, Outcome::Passed, 2_000_000),
        measured(2, Outcome::Failed, 3_000_000),
        measured(3, Outcome::Broken, 4_000_000),
        measured(4, Outcome::Interrupted, 5_000_000),
    ]);
    let samples = samples(&suite).unwrap();
    assert_eq!(samples.len(), 1);
    assert_eq!(samples[0].values, [2e6, 3e6]);
    assert_eq!(samples[0].excluded, 2);
}

#[test]
fn one_name_under_two_identities_or_gates_is_refused() {
    let under = |second: Measurement| {
        samples(&suite_of(vec![
            repetition("udp-tx", 1, Outcome::Passed, None, vec![rate(2_000_000)]),
            repetition("udp-tx", 2, Outcome::Passed, None, vec![second]),
        ]))
    };
    let unit = Measurement::observed("udp.tx.host-rate", 2, MeasurementUnit::Count)
        .evaluated(Comparison::AtLeast, 1);
    assert!(under(unit).unwrap_err().to_string().contains("unit"));
    let semantics = rate(2_000_000).semantics(2);
    assert!(
        under(semantics)
            .unwrap_err()
            .to_string()
            .contains("semantics 1 vs 2")
    );
    let direction = Measurement::observed("udp.tx.host-rate", 2, MeasurementUnit::BitsPerSecond)
        .evaluated(Comparison::AtMost, 3_000_000);
    assert!(under(direction).unwrap_err().to_string().contains("better"));
    let gate = Measurement::observed(
        "udp.tx.host-rate",
        2_000_000,
        MeasurementUnit::BitsPerSecond,
    )
    .evaluated(Comparison::AtLeast, 5);
    assert!(under(gate).unwrap_err().to_string().contains("gates"));
}

/// A suite of one `udp-tx` scenario with `repetitions`.
fn suite_of(repetitions: Vec<oer_hil_run_bundle_format::run::RepetitionResult>) -> SuiteResult {
    let directory = tempfile::tempdir().unwrap();
    let run = directory.path().join("1-a");
    let count = repetitions.len() as u8;
    write_run(
        &run,
        1,
        RunState::Completed,
        vec![ScenarioResult::from_repetitions(
            "udp-tx".into(),
            ImageClass::SystemPanicReset,
            count,
            repetitions,
        )],
        |_| {},
    );
    oer_hil_run_bundle_format::RunBundle::open(&run)
        .unwrap()
        .unwrap()
        .suite()
        .unwrap()
        .unwrap()
}
