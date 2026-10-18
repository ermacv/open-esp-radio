use std::path::Path;

use oer_hil_run_bundle::run::test_support::repetition;
use oer_hil_run_bundle::run::test_support::write_run;
use oer_hil_run_bundle_format::run::Measurement;
use oer_hil_run_bundle_format::run::MeasurementUnit;
use oer_hil_run_bundle_format::run::Outcome;
use oer_hil_run_bundle_format::run::ScenarioResult;
use oer_hil_schema::image::ImageClass;

use super::*;
use crate::samples::tests::rate;

/// The summary of a run whose metrics are compatible.
fn summary(run: &Run) -> RunSummary {
    super::summary(run).unwrap()
}

/// A completed run `id` below `store` of one `udp-tx` scenario whose
/// repetitions measured `values`, at `commit`.
fn run_in(store: &Path, id: &str, commit: &str, dirty: bool, values: &[u64]) -> Run {
    let directory = store.join(id);
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
        &directory,
        1,
        RunState::Completed,
        vec![ScenarioResult::from_repetitions(
            "udp-tx".into(),
            ImageClass::SystemPanicReset,
            values.len() as u8,
            repetitions,
        )],
        |manifest| {
            manifest.repository.commit = commit.into();
            manifest.repository.dirty = dirty;
        },
    );
    Run::load(&directory).unwrap()
}

#[test]
fn only_gated_measurements_enter_a_summary() {
    let store = tempfile::tempdir().unwrap();
    let run = run_in(store.path(), "1-a", "c", false, &[115_000_000, 116_000_000]);
    let summary = summary(&run);
    assert_eq!(summary.samples.len(), 1);
    assert_eq!(summary.samples[0].values, [115e6, 116e6]);
    assert_eq!(summary.commit.as_deref(), Some("c"));
}

#[test]
fn baselines_come_only_from_clean_sealed_runs_and_flag_regressions() {
    let directory = tempfile::tempdir().unwrap();
    let store = RunStore::at(directory.path());
    let runs = directory.path().join("runs");
    let summarize =
        |id, commit, dirty, values: &[u64]| summary(&run_in(&runs, id, commit, dirty, values));
    let dirty = summarize("1-d", "c", true, &[115_000_000]);
    assert!(set_baseline(&store, &dirty, &[], "r", "me", 0).is_err());
    let clean = summarize("2-b", "c", false, &[115_000_000, 116_000_000, 117_000_000]);
    assert_eq!(
        set_baseline(&store, &clean, &[], "owned-xarxa", "me", 0).unwrap(),
        ["udp-tx"]
    );
    assert!(set_baseline(&store, &clean, &["other".into()], "r", "me", 0).is_err());
    let baselines = baselines(&store).unwrap();
    assert!(regressions(&summarize("3-x", "d", false, &[116_000_000]), &baselines).is_empty());
    let regressed = summarize("4-y", "e", false, &[100_000_000]);
    assert_eq!(regressions(&regressed, &baselines).len(), 1);
    let text = report(&[clean.clone(), regressed, dirty], &[], None, &baselines);
    assert!(text.contains("REGRESSED"), "{text}");
    assert!(
        !text.contains("[1-d]"),
        "dirty runs are not reported: {text}"
    );
}

#[test]
fn sealed_summaries_are_reused_and_older_runs_are_not_read() {
    let directory = tempfile::tempdir().unwrap();
    let store = RunStore::at(directory.path());
    for name in ["1000-old", "3000-new"] {
        fs::create_dir_all(store.run(name)).unwrap();
    }
    // The new run's bundle is empty: only its cached summary can supply it.
    let cache = store.cache("perf");
    fs::create_dir_all(&cache).unwrap();
    let mut cached = summary(&run_in(
        &directory.path().join("other"),
        "3000-new",
        "c",
        false,
        &[115_000_000],
    ));
    cached.started_millis = 3000;
    fs::write(
        cache.join("3000-new.json"),
        serde_json::to_vec(&cached).unwrap(),
    )
    .unwrap();
    let found = summaries_since(&store, 2000).unwrap();
    assert_eq!(found, [cached.clone()]);
    let stale = RunSummary {
        version: 0,
        ..cached
    };
    fs::write(
        cache.join("3000-new.json"),
        serde_json::to_vec(&stale).unwrap(),
    )
    .unwrap();
    assert!(summaries_since(&store, 2000).unwrap().is_empty());
}

#[test]
fn the_layout_seed_comes_from_each_flashed_image() {
    let store = tempfile::tempdir().unwrap();
    let directory = store.path().join("1-s");
    write_run(&directory, 1, RunState::Completed, Vec::new(), |manifest| {
        let mut firmware: oer_hil_run_bundle_format::run::FirmwareArtifact =
            serde_json::from_value(serde_json::json!({
                "image": "system-panic-reset",
                "application_path": "firmware/performance/application.bin",
                "application_size_bytes": 1,
                "application_sha256": "00",
                "runtime_elf_sha256": "01",
                "layout_seed": 7,
            }))
            .unwrap();
        manifest.firmware.push(firmware.clone());
        firmware.image = ImageClass::SystemWatchdog;
        firmware.layout_seed = None;
        manifest.firmware.push(firmware);
    });
    assert_eq!(summary(&Run::load(&directory).unwrap()).layout_seeds, [7]);
    assert!(
        summary(&run_in(store.path(), "2-n", "c", false, &[1_000_000]))
            .layout_seeds
            .is_empty()
    );
    assert_eq!(layout(&[]), "natural");
    assert_eq!(layout(&[7, 11]), "seed 7,11");
}

#[test]
fn layouts_of_one_commit_are_reported_apart_with_their_spread() {
    let store = tempfile::tempdir().unwrap();
    let seeded = |id, seed, values: &[u64]| {
        let mut summary = summary(&run_in(store.path(), id, "c", false, values));
        summary.layout_seeds = vec![seed];
        summary
    };
    let runs = [
        summary(&run_in(
            store.path(),
            "1-n",
            "c",
            false,
            &[100_000_000, 101_000_000],
        )),
        seeded("2-a", 7, &[120_000_000, 121_000_000]),
        seeded("3-b", 11, &[90_000_000, 91_000_000]),
    ];
    let text = report(&runs, &[], None, &BTreeMap::new());
    assert!(text.contains("natural"), "{text}");
    assert!(text.contains("seed 7"), "{text}");
    assert!(text.contains("seed 11"), "{text}");
    assert!(text.contains("across 3 layouts"), "{text}");
    // One layout alone reports no layout sensitivity.
    let single = report(&runs[..1], &[], None, &BTreeMap::new());
    assert!(!single.contains("across"), "{single}");
}
