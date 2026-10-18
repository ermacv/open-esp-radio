use std::collections::BTreeSet;

use super::*;

fn catalog() -> Catalog {
    Catalog::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenarios"))
        .expect("load scenario catalog")
}

fn passed(scenario: &Scenario) -> ScenarioResult {
    ScenarioResult {
        schema: RUN_SCHEMA,
        scenario: scenario.id().to_owned(),
        image: scenario.image(),
        outcome: Outcome::Passed,
        required_repetitions: scenario.repetitions(),
        repetitions: (1..=scenario.repetitions())
            .map(|repetition| RepetitionResult {
                schema: RUN_SCHEMA,
                repetition,
                outcome: Outcome::Passed,
                started_unix_millis: 0,
                duration_millis: 0,
                artifact_directory: PathBuf::from("scenarios")
                    .join(scenario.id())
                    .join(format!("repetition-{repetition:03}")),
                attachments: Vec::new(),
                measurements: Vec::new(),
                failure: None,
            })
            .collect(),
        failure: None,
    }
}

fn session(root: &Path) -> RunSession {
    crate::tests::register();
    RunSession::create(
        root,
        "chip-a",
        "test-cell",
        "test-dut",
        Path::new("/test/no-device"),
        vec![OsString::from("test")],
    )
    .unwrap()
}

#[derive(Default)]
struct FakeSuite {
    log: Vec<String>,
    preparation_failures: Vec<(ImageClass, FailureKind)>,
    preflight_failures: BTreeSet<String>,
    cancel_on_check: Option<usize>,
    checks: usize,
    /// Failure returned by the reflash after a yield before a scenario.
    yield_failure: Option<FailureKind>,
}

impl SuiteEffects for FakeSuite {
    fn check_cancelled(&mut self) -> Result<()> {
        self.checks += 1;
        self.log.push(format!("cancel-check:{}", self.checks));
        if self.cancel_on_check == Some(self.checks) {
            return Err("injected cancellation".into());
        }
        Ok(())
    }

    fn preflight(&mut self, scenario: &Scenario) -> Option<Failure> {
        self.log.push(format!("preflight:{}", scenario.id()));
        self.preflight_failures
            .contains(scenario.id())
            .then(|| Failure::new(FailureKind::Precondition, "injected preflight failure"))
    }

    fn prepare_image(
        &mut self,
        class: ImageClass,
        _session: &mut RunSession,
    ) -> Result<Option<Failure>> {
        self.log.push(format!("prepare:{}", class.id()));
        Ok(self
            .preparation_failures
            .iter()
            .find(|(candidate, _)| *candidate == class)
            .map(|(_, kind)| Failure::new(*kind, "injected image failure")))
    }

    fn execute_scenario(
        &mut self,
        scenario: &Scenario,
        _session: &RunSession,
    ) -> Result<ScenarioResult> {
        self.log.push(format!("execute:{}", scenario.id()));
        Ok(passed(scenario))
    }

    fn yield_point(
        &mut self,
        class: Option<ImageClass>,
        _session: &mut RunSession,
    ) -> Result<Option<Failure>> {
        self.log.push(format!(
            "yield:{}",
            class.map_or("before-image", ImageClass::id)
        ));
        Ok(class
            .and(self.yield_failure)
            .map(|kind| Failure::new(kind, "injected reflash failure")))
    }
}

/// A catalog of two scenarios on one image class, which no two scenarios of
/// the tree's catalog share, and one on another.
fn shared_image_catalog() -> (tempfile::TempDir, Catalog) {
    let directory = tempfile::tempdir().unwrap();
    let watchdog = include_str!("../../../../../scenarios/system/system-watchdog.toml");
    for id in ["watchdog-a", "watchdog-b"] {
        std::fs::write(
            directory.path().join(format!("{id}.toml")),
            watchdog.replace("id = \"system-watchdog\"", &format!("id = \"{id}\"")),
        )
        .unwrap();
    }
    std::fs::write(
        directory.path().join("system-panic-reset.toml"),
        include_str!("../../../../../scenarios/system/system-panic-reset.toml"),
    )
    .unwrap();
    let catalog = Catalog::load(directory.path()).unwrap();
    (directory, catalog)
}

fn two_same_image(catalog: &Catalog) -> [&Scenario; 2] {
    for class in ImageClass::ALL {
        let candidates = catalog
            .all()
            .iter()
            .filter(|scenario| scenario.image() == class)
            .take(2)
            .collect::<Vec<_>>();
        if let [first, second] = candidates.as_slice() {
            return [*first, *second];
        }
    }
    panic!("catalog has no shared image class")
}

#[test]
fn successful_group_prepares_once_and_preserves_scenario_order() {
    let (_scenarios, catalog) = shared_image_catalog();
    let selected = two_same_image(&catalog);
    let root = tempfile::tempdir().unwrap();
    let mut session = session(root.path());
    let mut fake = FakeSuite::default();
    let results = execute_selected(&mut session, &mut fake, &selected).unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(
        fake.log
            .iter()
            .filter(|entry| entry.starts_with("prepare:"))
            .count(),
        1
    );
    let executed = fake
        .log
        .iter()
        .filter_map(|entry| entry.strip_prefix("execute:"))
        .collect::<Vec<_>>();
    assert_eq!(executed, [selected[0].id(), selected[1].id()]);
}

#[test]
fn image_build_and_flash_failures_block_the_group_but_continue_later_classes() {
    let catalog = catalog();
    let first = catalog
        .all()
        .iter()
        .find(|scenario| scenario.image() == ImageClass::SystemWatchdog)
        .expect("Bluetooth GATT scenario");
    let second = catalog
        .all()
        .iter()
        .find(|scenario| scenario.image() == ImageClass::BootSmoke)
        .expect("boot-smoke scenario");

    for kind in [FailureKind::ImageBuild, FailureKind::ImageFlash] {
        let root = tempfile::tempdir().unwrap();
        let mut session = session(root.path());
        let mut fake = FakeSuite {
            preparation_failures: vec![(first.image(), kind)],
            ..FakeSuite::default()
        };
        let results = execute_selected(&mut session, &mut fake, &[first, second]).unwrap();
        assert_eq!(results[0].outcome, Outcome::Blocked);
        assert_eq!(results[0].failure.as_ref().unwrap().kind, kind);
        assert_eq!(results[1].outcome, Outcome::Passed);
        assert!(fake.log.contains(&format!("execute:{}", second.id())));
    }
}

#[test]
fn preflight_failure_has_no_image_or_workload_side_effect() {
    let catalog = catalog();
    let scenario = catalog.get("boot-smoke").expect("boot-smoke");
    let root = tempfile::tempdir().unwrap();
    let mut session = session(root.path());
    let mut fake = FakeSuite {
        preflight_failures: BTreeSet::from([scenario.id().to_owned()]),
        ..FakeSuite::default()
    };
    let results = execute_selected(&mut session, &mut fake, &[scenario]).unwrap();
    assert_eq!(results[0].outcome, Outcome::Blocked);
    assert!(!fake.log.iter().any(|entry| entry.starts_with("prepare:")));
    assert!(!fake.log.iter().any(|entry| entry.starts_with("execute:")));
}

#[test]
fn cancellation_stops_before_the_next_scenario() {
    let (_scenarios, catalog) = shared_image_catalog();
    let selected = two_same_image(&catalog);
    let root = tempfile::tempdir().unwrap();
    let mut session = session(root.path());
    let mut fake = FakeSuite {
        cancel_on_check: Some(3),
        ..FakeSuite::default()
    };
    let error = execute_selected(&mut session, &mut fake, &selected).unwrap_err();
    assert_eq!(error.to_string(), "injected cancellation");
    assert!(fake.log.contains(&format!("execute:{}", selected[0].id())));
    assert!(!fake.log.contains(&format!("execute:{}", selected[1].id())));
    let seal = session
        .directory()
        .join("attempts")
        .join(format!("{}.json", selected[0].id()));
    let before = fs::read(&seal).unwrap();
    assert!(
        !session
            .directory()
            .join("attempts")
            .join(format!("{}.json", selected[1].id()))
            .exists()
    );
    let record: serde_json::Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(record["suite"]["scenarios"][0]["outcome"], "passed");
    drop(session);
    assert_eq!(fs::read(seal).unwrap(), before);
}

#[test]
fn attempt_seal_is_write_once_and_partial_series_cannot_claim_completion() {
    let catalog = catalog();
    let scenario = catalog.get("boot-smoke").unwrap();
    let root = tempfile::tempdir().unwrap();
    let mut session = session(root.path());
    let complete = passed(scenario);
    let mut partial = complete.clone();
    partial.repetitions.pop();
    assert!(session.seal_scenario(scenario, &partial).is_err());
    session.seal_scenario(scenario, &complete).unwrap();
    let seal = session.directory().join("attempts/boot-smoke.json");
    let before = fs::read(&seal).unwrap();
    assert!(session.seal_scenario(scenario, &complete).is_err());
    assert_eq!(fs::read(seal).unwrap(), before);
}

#[test]
fn single_scenario_preserves_its_original_preflight_prepare_event_order() {
    let catalog = catalog();
    let scenario = catalog.get("boot-smoke").expect("boot-smoke");
    let root = tempfile::tempdir().unwrap();
    let mut session = session(root.path());
    let mut fake = FakeSuite::default();
    let results = execute_one(&mut session, &mut fake, scenario).unwrap();

    assert_eq!(results.len(), 1);
    assert_eq!(fake.checks, 0);
    assert_eq!(
        fake.log,
        [
            format!("preflight:{}", scenario.id()),
            format!("prepare:{}", scenario.image().id()),
            format!("execute:{}", scenario.id()),
        ]
    );
    let events = fs::read_to_string(session.directory().join("events.jsonl")).unwrap();
    let kinds = events
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap()["kind"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        &kinds[kinds.len() - 2..],
        ["scenario-started", "scenario-finished"]
    );
}

#[test]
fn cleanup_is_written_before_attachment_indexing_and_preserves_partial_output() {
    oer_hil_workload::fixture::cleanup::reset_for_test();
    let output = tempfile::tempdir().unwrap();
    fs::write(output.path().join("partial.json"), b"{\"seen\":true}\n").unwrap();
    let cleanup = oer_hil_workload::fixture::cleanup::Scope::new(output.path());
    oer_hil_workload::fixture::cleanup::record("restore fixture", || {
        Err("injected cleanup error".into())
    });
    let started = std::time::Instant::now();
    let result = finalize_repetition(
        1,
        Path::new("scenarios/test/repetition-001"),
        &oer_hil_lab::usb_events::UsbWatch::start([], 1),
        output.path(),
        1,
        started,
        cleanup,
        Outcome::Passed,
        None,
        Vec::new(),
    )
    .unwrap();

    assert_eq!(result.outcome, Outcome::Broken);
    assert_eq!(
        result.failure.as_ref().unwrap().kind,
        FailureKind::Infrastructure
    );
    let names = result
        .attachments
        .iter()
        .map(|attachment| attachment.path.file_name().unwrap().to_string_lossy())
        .collect::<BTreeSet<_>>();
    assert!(names.contains("cleanup.json"));
    assert!(names.contains("partial.json"));
    assert!(oer_hil_workload::fixture::cleanup::require_healthy().is_err());
    oer_hil_workload::fixture::cleanup::reset_for_test();
}

#[test]
fn cleanup_error_appends_to_the_primary_workload_failure() {
    let mut outcome = Outcome::Failed;
    let mut failure = Some(Failure::new(FailureKind::Scenario, "workload timed out"));
    apply_cleanup_failures(&mut outcome, &mut failure, &["restore failed"]);
    let failure = failure.unwrap();
    assert_eq!(outcome, Outcome::Failed);
    assert_eq!(failure.kind, FailureKind::Scenario);
    assert!(failure.message.contains("workload timed out"));
    assert!(failure.message.contains("restore failed"));
}

#[test]
fn run_all_selection_has_one_ordered_image_plan_and_requirement_union() {
    let catalog = catalog();
    let selected = crate::cli::Selection {
        scenario: None,
        tag: Vec::new(),
        role: Some(String::from("investigation")),
        chip: None,
    }
    .resolve(&catalog)
    .unwrap();
    assert!(
        selected
            .iter()
            .all(|scenario| scenario.header.role == oer_hil_scenario_catalog::Role::Investigation)
    );
    let groups = group_selected_scenarios(&selected);
    let flattened = groups
        .iter()
        .flat_map(|(_, scenarios)| scenarios.iter().copied())
        .collect::<Vec<_>>();
    assert_eq!(flattened.len(), selected.len());
    assert_eq!(
        crate::scenario::requirements(&selected),
        crate::scenario::requirements(&flattened)
    );
    for scenario in selected {
        assert_eq!(
            flattened
                .iter()
                .filter(|entry| entry.id() == scenario.id())
                .count(),
            1
        );
    }
}

#[test]
fn named_scenarios_keep_their_order_and_replay_needs_one_image_class() {
    let (_scenarios, catalog) = shared_image_catalog();
    let [first, second] = two_same_image(&catalog);
    let ids = [second.id().to_owned(), first.id().to_owned()];
    let selected = named_scenarios(&catalog, &ids).unwrap();
    assert_eq!(selected[0].id(), second.id());
    assert_eq!(single_image_class(&selected).unwrap(), first.image());
    let refs = selected.iter().collect::<Vec<_>>();
    assert_eq!(
        scenarios_description(&refs),
        format!("scenarios: {}, {}", second.id(), first.id())
    );
    assert!(named_scenarios(&catalog, &[ids[0].clone(), ids[0].clone()]).is_err());
    assert!(named_scenarios(&catalog, &[String::from("no-such-scenario")]).is_err());
    let other = catalog
        .all()
        .iter()
        .find(|scenario| scenario.image() != first.image() && scenario.header.unsupported.is_none())
        .unwrap();
    let mixed = named_scenarios(&catalog, &[first.id().to_owned(), other.id().to_owned()]).unwrap();
    assert!(single_image_class(&mixed).is_err());
}

#[test]
fn a_series_offers_to_yield_before_each_image_and_between_scenarios() {
    let (_scenarios, catalog) = shared_image_catalog();
    let selected = two_same_image(&catalog);
    let root = tempfile::tempdir().unwrap();
    let mut session = session(root.path());
    let mut fake = FakeSuite::default();
    execute_selected(&mut session, &mut fake, &selected).unwrap();
    let steps = fake
        .log
        .iter()
        .filter(|entry| !entry.starts_with("cancel-check") && !entry.starts_with("preflight"))
        .cloned()
        .collect::<Vec<_>>();
    let class = selected[0].image().id();
    assert_eq!(
        steps,
        [
            String::from("yield:before-image"),
            format!("prepare:{class}"),
            format!("execute:{}", selected[0].id()),
            format!("yield:{class}"),
            format!("execute:{}", selected[1].id()),
        ]
    );
}

#[test]
fn a_failed_reflash_after_yielding_blocks_only_the_next_scenario() {
    let (_scenarios, catalog) = shared_image_catalog();
    let selected = two_same_image(&catalog);
    let root = tempfile::tempdir().unwrap();
    let mut session = session(root.path());
    let mut fake = FakeSuite {
        yield_failure: Some(FailureKind::ImageFlash),
        ..FakeSuite::default()
    };
    let results = execute_selected(&mut session, &mut fake, &selected).unwrap();
    assert_eq!(results[0].outcome, Outcome::Passed);
    assert_eq!(results[1].outcome, Outcome::Blocked);
}

#[test]
fn a_series_yields_and_an_air_measurement_claims_its_ranges_strictly() {
    use oer_hil_lab::lock::{BAND_2G4, Emits, Need, Spectrum};
    let (_scenarios, catalog) = shared_image_catalog();
    let lab = LabConfig::for_test();
    let lease_request = |selected: &[&Scenario]| lease_request(&lab, selected);
    let [first, second] = two_same_image(&catalog);
    assert!(!lease_request(&[first]).divisible);
    let series = lease_request(&[first, second]);
    assert!(series.divisible);
    assert_eq!(series.scenarios, [first.id(), second.id()]);
    let mut measured = first.clone();
    measured.header.tags.push(AIR_EXCLUSIVE_TAG.to_owned());
    let air = lease_request(&[&measured, second]).air;
    for range in air_use(&lab, first) {
        assert!(air.contains(&Spectrum::new(
            (range.low_khz, range.high_khz),
            Need::Strict,
            Emits::Noisy
        )));
    }
    // Scenario ranges follow the family through the registry: the watchdog
    // none, another system scenario the band.
    let find = |key: &str, predicate: &dyn Fn(&serde_json::Value) -> bool| {
        catalog
            .all()
            .iter()
            .find(|scenario| scenario.family.key() == key && predicate(&scenario.family.table()))
            .expect("the catalog has such a scenario")
    };
    let watchdog = find("system", &|table| table["kind"] == "watchdog");
    assert!(lease_request(&[watchdog]).air.is_empty());
    let panic = find("system", &|table| table["kind"] == "panic-reset");
    let [range] = air_use(&lab, panic)[..] else {
        panic!("a system scenario on the radio occupies one range");
    };
    assert_eq!((range.low_khz, range.high_khz), BAND_2G4);
}

#[test]
fn the_then_command_joins_the_lease_and_names_the_run() {
    let directory = tempfile::tempdir().unwrap();
    let run = directory.path().join("runs/1-a");
    let output = then_command(
        directory.path(),
        "printf '%s\n%s' \"$OER_OPERATION_CONTEXT\" \"$OER_HIL_RUN_DIRECTORY\"; exit 3",
        &oer_process::Context::default().with("stand.lease", "token"),
        &run,
    )
    .unwrap()
    .output()
    .unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        format!("{{\"stand.lease\":\"token\"}}\n{}", run.display())
    );
}

#[test]
fn a_run_takes_the_named_chip_or_the_only_one_that_builds_its_images() {
    let catalog = catalog();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
    let panic = catalog.get("system-panic-reset").unwrap();
    let watchdog = catalog.get("system-watchdog").unwrap();
    let building = |classes: &[ImageClass]| oer_hil_image::chips_building(&root, classes).unwrap();
    // Exactly one chip's agent builds the panic-reset image.
    let [full] = building(&[ImageClass::SystemPanicReset])
        .try_into()
        .unwrap();
    assert_eq!(select_chip(&root, &[panic], None).unwrap(), full);
    let others: Vec<String> = oer_chip_profile::supported(&root)
        .unwrap()
        .into_iter()
        .filter(|chip| *chip != full)
        .collect();
    for other in &others {
        assert!(select_chip(&root, &[panic], Some(other)).is_err());
    }
    // Several chips build the watchdog image: the run names one.
    let watchdogs = building(&[ImageClass::SystemWatchdog]);
    assert!(watchdogs.len() > 1);
    let error = select_chip(&root, &[watchdog], None)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(&watchdogs.join(", ")) && error.contains("--chip"),
        "{error}"
    );
    for chip in &watchdogs {
        assert_eq!(select_chip(&root, &[watchdog], Some(chip)).unwrap(), *chip);
    }
    assert_eq!(select_chip(&root, &[panic, watchdog], None).unwrap(), full);
}

#[test]
fn a_silent_image_on_a_loadable_board_gets_the_recovery_image_once() {
    assert!(super::needs_recovery_image(true, false, false, false));
    assert!(
        !super::needs_recovery_image(false, false, false, false),
        "the image answers"
    );
    assert!(
        !super::needs_recovery_image(true, true, false, false),
        "a quarantined board waits for a person"
    );
    assert!(
        !super::needs_recovery_image(true, false, true, false),
        "once per image class and run"
    );
    assert!(
        !super::needs_recovery_image(true, false, false, true),
        "a recovery image this run found silent is not flashed again"
    );
}

#[test]
fn a_recovery_image_that_does_not_answer_is_the_images_fault_not_the_boards() {
    use oer_hil_run_bundle_format::run::{Failure, FailureKind};
    let unreached = || -> std::result::Result<(), String> { panic!("not flashed") };
    assert_eq!(
        super::after_reflash(None, || Ok(())),
        super::AfterReflash::Recovered
    );
    // The flash proved the ROM answers: never the board's fault.
    assert_eq!(
        super::after_reflash(None, || Err(String::from("no boot-smoke pass line"))),
        super::AfterReflash::ImageFault(String::from("no boot-smoke pass line"))
    );
    assert_eq!(
        super::after_reflash(
            Some(Failure::new(FailureKind::ImageBuild, "does not compile")),
            unreached
        ),
        super::AfterReflash::ImageFault(String::from("does not compile"))
    );
    // A flash that failed proves nothing of the ROM: the ladder judges it.
    assert_eq!(
        super::after_reflash(
            Some(Failure::new(
                FailureKind::ImageFlash,
                "no answer from the ROM"
            )),
            unreached
        ),
        super::AfterReflash::FlashFailed(String::from("no answer from the ROM"))
    );
}
