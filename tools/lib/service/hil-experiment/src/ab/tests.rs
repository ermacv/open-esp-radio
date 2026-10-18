use super::*;

#[test]
fn enclosing_lease_reaches_preparation_and_replay() {
    crate::lease_tests::within_enclosing_lease(
        "ab::tests::enclosing_lease_reaches_preparation_and_replay",
        |directory, runner| {
            let root = crate::lease_tests::repository(directory);
            let arm = PreparedArm {
                arm: Arm::A,
                variant: Variant {
                    commit: "HEAD".into(),
                    overrides: Vec::new(),
                    features: FeatureDelta::default(),
                },
                worktree: Worktree::detached(&root, &directory.join("arm-a"), "HEAD").unwrap(),
                snapshot: directory.join("snapshot"),
            };
            let session = Session {
                checkout: &root,
                owner: "experiment-test",
                id: "experiment",
                directory,
                runner: &runner,
                boards: &[],
                store: RunStore::at(directory.join("store")),
            };
            let round = round(&[], NonZeroU32::MIN, 0, 1);
            let run = arm
                .run(
                    &session,
                    &["boot-smoke".into()],
                    &Firmware::Build(NonZeroU32::MIN),
                    round,
                    None,
                )
                .unwrap();
            assert_eq!(run, RunId::new("7"));
            let grant = oer_stand_arbiter::Arbiter::open()
                .unwrap()
                .acquire(&oer_stand_arbiter::Request {
                    owner: "experiment-test".into(),
                    work: "replay".into(),
                    scenarios: Vec::new(),
                    run: None,
                    claims: vec![oer_stand_claims::Claim::stand()],
                })
                .unwrap();
            assert!(grant.is_nested());
            let run = arm
                .run(
                    &session,
                    &["boot-smoke".into()],
                    &Firmware::Replay(run),
                    round,
                    Some(&grant),
                )
                .unwrap();
            assert_eq!(run, RunId::new("7"));
            arm.worktree.remove().unwrap();
        },
    );
}

#[test]
fn a_feature_delta_reaches_the_runner_as_one_argument_even_when_it_removes_first() {
    let removal = "features=-panic-hook"
        .parse::<VariantSpec>()
        .unwrap()
        .features;
    assert_eq!(
        features_argument(&removal).as_deref(),
        Some("--features=-panic-hook")
    );
    assert_eq!(features_argument(&FeatureDelta::default()), None);
}

#[test]
fn a_variant_is_a_revision_and_its_dependency_overrides() {
    assert_eq!(
        "rev=main;override:esp-hal=/src/esp-hal".parse::<VariantSpec>(),
        Ok(VariantSpec {
            revision: String::from("main"),
            overrides: vec![(Dependency::EspHal, PathBuf::from("/src/esp-hal"))],
            features: FeatureDelta::default(),
        })
    );
    assert_eq!(
        "features=+usb-jtag-off,-panic-hook"
            .parse::<VariantSpec>()
            .unwrap()
            .features
            .to_string(),
        "+usb-jtag-off,-panic-hook"
    );
    assert_eq!(
        "override:esp-hal=/h"
            .parse::<VariantSpec>()
            .unwrap()
            .revision,
        "HEAD"
    );
    for invalid in [
        "rev=a;rev=b",
        "override:esp-hal=/a;override:esp-hal=/b",
        "override:tokio=/t",
        "override:xarxa=/x",
        "override:esp-hal",
        "features=x",
        "features=+a;features=+b",
        "colour=red",
    ] {
        assert!(invalid.parse::<VariantSpec>().is_err(), "{invalid}");
    }
}

/// A completed run `id` of `arm` in measured round `index` below `runs` of
/// `udp-rx` that measured `throughput` once (gated, higher is better) and
/// an ungated counter.
fn run(runs: &Path, id: &str, arm: Arm, index: u32, throughput: u64) -> Run {
    use oer_hil_run_bundle::run::test_support::repetition;
    use oer_hil_run_bundle::run::test_support::write_run;
    use oer_hil_run_bundle_format::run::Comparison;
    use oer_hil_run_bundle_format::run::Measurement;
    use oer_hil_run_bundle_format::run::MeasurementUnit;
    use oer_hil_run_bundle_format::run::RunState;
    use oer_hil_run_bundle_format::run::ScenarioResult;
    use oer_hil_schema::image::ImageClass;
    write_run(
        &runs.join(id),
        0,
        RunState::Completed,
        vec![ScenarioResult::from_repetitions(
            "udp-rx".into(),
            ImageClass::DiagnosticUsbJtagOff,
            1,
            vec![repetition(
                "udp-rx",
                1,
                Outcome::Passed,
                None,
                vec![
                    Measurement::observed("rx.bps", throughput, MeasurementUnit::BitsPerSecond)
                        .evaluated(Comparison::AtLeast, 1),
                    Measurement::observed("rx.zero-copy", 5, MeasurementUnit::Count),
                ],
            )],
        )],
        |manifest| {
            manifest.experiment = Some(Experiment {
                id: String::from("7"),
                arm,
                variant: Variant {
                    commit: arm.to_string(),
                    overrides: Vec::new(),
                    features: FeatureDelta::default(),
                },
                round: round(
                    &[Order::Ab, Order::Ba, Order::Ab],
                    NonZeroU32::MIN,
                    index,
                    1,
                ),
            });
        },
    );
    Run::load(&runs.join(id)).unwrap()
}

#[test]
fn the_summary_names_the_variants_and_each_verdict() {
    let directory = tempfile::tempdir().unwrap();
    let runs = [
        (Arm::A, "a1", 1, 100),
        (Arm::A, "a2", 2, 101),
        (Arm::A, "a3", 3, 99),
        (Arm::B, "b1", 1, 120),
        (Arm::B, "b2", 2, 121),
        (Arm::B, "b3", 3, 119),
    ]
    .map(|(arm, id, index, throughput)| (arm, run(directory.path(), id, arm, index, throughput)));
    let variant = |commit: &str| Variant {
        commit: commit.into(),
        overrides: Vec::new(),
        features: FeatureDelta::default(),
    };
    let report = Report {
        schema: REPORT_SCHEMA,
        id: String::from("7"),
        a: variant("aaaaaaaaaaaaaaaa"),
        b: Variant {
            commit: String::from("aaaaaaaaaaaaaaaa"),
            overrides: vec![DependencyOverride {
                dependency: Dependency::EspHal,
                path: PathBuf::from("/x"),
                commit: String::from("ea9385e"),
                dirty: true,
            }],
            features: FeatureDelta::default(),
        },
        scenarios: vec![String::from("udp-rx")],
        repetitions: 3,
        layout_seeds: 1,
        order_seed: 1,
        runs: Vec::new(),
        comparisons: arms::compare(
            &ValidatedExperiment::new(&runs.iter().map(|(_, run)| run).collect::<Vec<_>>())
                .unwrap(),
        )
        .unwrap(),
    };
    let text = summary(&report);
    assert!(
        text.contains("A aaaaaaaaaaaa vs B aaaaaaaaaaaa +esp-hal@ea9385e+"),
        "{text}"
    );
    assert!(
        text.contains("rx.bps") && text.contains("significant, B better"),
        "{text}"
    );
    assert!(
        text.contains("rx.zero-copy") && text.contains("(ungated)"),
        "{text}"
    );
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["b"]["overrides"][0]["dependency"], "esp-hal");
}

#[test]
fn every_replay_round_leases_the_same_work_so_its_estimate_is_a_round() {
    let scenarios = [String::from("udp-rx"), String::from("udp-tx")];
    assert_eq!(round_work(&scenarios), round_work(&scenarios.clone()));
    assert_eq!(round_work(&scenarios), "ab round udp-rx udp-tx");
}

#[test]
fn round_0_prepares_and_the_measured_rounds_take_the_drawn_orders() {
    let seed = NonZeroU32::MIN;
    let orders = orders(9, seed, 4);
    let preparation = round(&orders, seed, 0, 9);
    assert_eq!(
        (preparation.phase, preparation.order),
        (Phase::Preparation, Order::Ab)
    );
    for index in 1..=4 {
        let measured = round(&orders, seed, index, 9);
        assert_eq!(measured.phase, Phase::Measurement);
        assert_eq!(measured.order, orders[index as usize - 1]);
    }
}

#[test]
fn a_metric_without_pairs_is_summarized_as_such() {
    let directory = tempfile::tempdir().unwrap();
    let only_a = run(directory.path(), "a1", Arm::A, 1, 100);
    let comparisons = arms::compare(&ValidatedExperiment::new(&[&only_a]).unwrap()).unwrap();
    let report = Report {
        schema: REPORT_SCHEMA,
        id: String::from("7"),
        a: Variant {
            commit: String::from("a"),
            overrides: Vec::new(),
            features: FeatureDelta::default(),
        },
        b: Variant {
            commit: String::from("b"),
            overrides: Vec::new(),
            features: FeatureDelta::default(),
        },
        scenarios: vec![String::from("udp-rx")],
        repetitions: 1,
        layout_seeds: 1,
        order_seed: 1,
        runs: Vec::new(),
        comparisons,
    };
    let text = summary(&report);
    assert!(text.contains("rx.bps: no pairs"), "{text}");
}

#[test]
fn every_layout_seed_draws_a_balanced_reproducible_order() {
    let mut sequences = std::collections::BTreeSet::new();
    for layout in 1..=8 {
        let seed = NonZeroU32::new(layout).unwrap();
        let drawn = orders(42, seed, 6);
        assert_eq!(drawn, orders(42, seed, 6));
        assert_eq!(drawn.iter().filter(|order| **order == Order::Ba).count(), 3);
        for pair in drawn.chunks(2) {
            assert_ne!(pair[0], pair[1], "layout seed {layout}: {drawn:?}");
        }
        sequences.insert(format!("{drawn:?}"));
    }
    // Layout seeds mix into the stream: they do not all share one order.
    assert!(sequences.len() > 1);
    // Another order seed draws another sequence for some layout seed.
    assert!((1..=8).any(|layout| {
        let seed = NonZeroU32::new(layout).unwrap();
        orders(42, seed, 6) != orders(43, seed, 6)
    }));
}
