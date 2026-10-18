//! CLI command dispatch with command-specific ordering and side-effect boundaries.

use std::env;

use clap::Parser as _;

use crate::scenario::{Catalog, requirements};
use crate::{
    Result, cli::Cli, cli::CliCommand, cli::ImageCommand, cli::ReportCommand, cli::ScenarioCommand,
    emit_json, execution::firmware::RunFirmware, execution::orchestration,
};
use oer_hil_image as image;
use oer_hil_lab as lab;
use oer_hil_scenario::ScenarioFamily as _;
use oer_hil_scenario_catalog::SCENARIO_SCHEMA;
use oer_hil_workload::family::Registry as _;
use oer_hil_workload::output;

pub(crate) fn run() -> Result<()> {
    let root = oer_process::built_root();
    let invocation = env::args_os().collect::<Vec<_>>();
    let cli = Cli::parse();
    output::reserve_machine_stdout()?;
    let _signals = oer_process::install_signal_handlers()?;
    let command = cli.command;
    let lab_path = cli
        .stand_file
        .unwrap_or(lab::config::LabConfig::default_path()?);
    let choice = lab::config::BoardChoice {
        dut: cli.board,
        peer: cli.peer_board,
    };
    let catalog_path = root.join("hil/scenarios");

    match command {
        CliCommand::Fixture {
            command: crate::cli::FixtureCommand::Check { scenario: id, chip },
        } => {
            let catalog = Catalog::load(&catalog_path)?;
            let selected = catalog.get(&id)?;
            let chip = orchestration::select_chip(&root, &[selected], chip.as_deref())?;
            let lab = lab::config::LabConfig::load(&lab_path, &chip, &choice)?
                .with_peer_images(&peer_images([selected]))?;
            // The stand's lease takes the fixture software once granted.
            crate::execution::fixture_check::check_without_device(&root, &lab, selected)
        }
        CliCommand::Doctor(selection) => {
            let catalog = Catalog::load(&catalog_path)?;
            let selected = selection.resolve(&catalog)?;
            let chip = orchestration::select_chip(&root, &selected, selection.chip.as_deref())?;
            let lab = lab::config::LabConfig::load(&lab_path, &chip, &choice)?
                .with_peer_images(&peer_images(selected.iter().copied()))?;
            let required = requirements(&selected);
            let _software = oer_hil_lab::software::SoftwareLease::acquire_for(&lab, required)?;
            crate::execution::doctor::run(&root, &lab, &selected)
        }
        CliCommand::Plan { selection, proofs } => {
            let catalog = Catalog::load(&catalog_path)?;
            let selected = selection.resolve(&catalog)?;
            let plan = oer_hil_scenario::campaign::Plan::create_for_checks(
                &catalog,
                &selected,
                oer_hil_image_class::NETWORK,
                &proofs,
            )?;
            emit_json(&plan, true)
        }
        CliCommand::Scenario { command } => {
            let catalog = Catalog::load(&catalog_path)?;
            match command {
                ScenarioCommand::List => emit_json(
                    &serde_json::json!({
                        "schema": SCENARIO_SCHEMA,
                        "scenarios": catalog.all(),
                    }),
                    true,
                ),
                ScenarioCommand::Validate { scenario: id } => {
                    if let Some(id) = id {
                        let _ = catalog.get(&id)?;
                    }
                    emit_json(
                        &serde_json::json!({
                            "schema": SCENARIO_SCHEMA,
                            "scenarios": catalog.all().len(),
                            "status": "valid"
                        }),
                        false,
                    )
                }
            }
        }
        CliCommand::Image { command } => match command {
            ImageCommand::Snapshot {
                source_include,
                include_untracked,
            } => {
                let snapshot = oer_hil_source_snapshot::capture(
                    &root,
                    &source_include,
                    include_untracked,
                    &image::source_snapshot_store()?,
                    &image::source_objects()?,
                )?;
                emit_json(&snapshot, true)
            }
            ImageCommand::Build {
                classes,
                source_snapshot,
                layout_seed,
                chip,
            } => {
                let chip = image::chip_for(&root, &classes, chip.as_deref())?;
                let frozen = source_snapshot
                    .as_deref()
                    .map(|snapshot| {
                        oer_hil_source_snapshot::mark_used(snapshot)?;
                        oer_hil_source_snapshot::FrozenSources::open_in_free_workspace(
                            snapshot,
                            &image::source_objects()?,
                            &image::frozen::build_slots(&chip)?,
                        )
                    })
                    .transpose()?;
                for class in classes {
                    let artifacts = match (&frozen, &source_snapshot) {
                        (Some(frozen), Some(snapshot)) => {
                            let artifacts = image::frozen::build(
                                frozen,
                                &chip,
                                class,
                                layout_seed,
                                &oer_hil_schema::image::FeatureDelta::default(),
                            )?;
                            let record = oer_hil_image::record::publish(
                                &root,
                                snapshot,
                                &image::source_objects()?,
                                class,
                                &artifacts,
                            )?;
                            eprintln!("build_record={}", record.display());
                            artifacts
                        }
                        _ => image::build(
                            &root,
                            &chip,
                            class,
                            layout_seed,
                            &oer_hil_schema::image::FeatureDelta::default(),
                        )?,
                    };
                    emit_json(&image::artifact_report(class, &artifacts)?, true)?;
                }
                Ok(())
            }
        },
        CliCommand::Report {
            command: ReportCommand::Verify { run_id, chip },
        } => {
            emit_json(
                &oer_hil_run_bundle::verify::verify(
                    &root,
                    chip.as_deref(),
                    run_id.as_deref(),
                    &oer_hil_image::record::Recipe,
                )?,
                false,
            )?;
            Ok(())
        }
        CliCommand::Run {
            scenarios,
            source_include,
            include_untracked,
            source_snapshot,
            firmware_from,
            layout_seed,
            features,
            repetitions,
            then,
            chip,
            validate_only,
            build_only,
        } => {
            let catalog = Catalog::load(&catalog_path)?;
            let mut selected = orchestration::named_scenarios(&catalog, &scenarios)?;
            if let Some(repetitions) = repetitions {
                for scenario in &mut selected {
                    scenario.header.repetitions = repetitions;
                }
            }
            let chip = orchestration::select_chip(
                &root,
                &selected.iter().collect::<Vec<_>>(),
                chip.as_deref(),
            )?;
            // An image with other features is not its class's image: only an
            // experiment, whose runs never qualify, may build one.
            if features.as_ref().is_some_and(|delta| !delta.is_empty())
                && oer_hil_run_bundle_format::experiment::Experiment::from_environment()?.is_none()
            {
                return Err(
                    "--features builds an experiment's image; use `cargo hil ab` with a \
                            `features=` variant"
                        .into(),
                );
            }
            if validate_only {
                println!(
                    "valid: {} on {chip}",
                    selected
                        .iter()
                        .map(|scenario| scenario.id())
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                return Ok(());
            }
            let snapshot = match (&firmware_from, source_snapshot) {
                (Some(_), _) => None,
                (None, Some(directory)) => {
                    oer_hil_source_snapshot::mark_used(&directory)?;
                    Some(oer_hil_source_snapshot::Snapshot::load(
                        &directory,
                        &image::source_objects()?,
                    )?)
                }
                (None, None) => Some(oer_hil_source_snapshot::capture(
                    &root,
                    &source_include,
                    include_untracked,
                    &image::source_snapshot_store()?,
                    &image::source_objects()?,
                )?),
            };
            if build_only {
                let snapshot = snapshot
                    .as_ref()
                    .ok_or("--build-only builds from the sources, not a replay")?;
                let selected = selected.iter().collect::<Vec<_>>();
                let frozen = oer_hil_source_snapshot::FrozenSources::open_in_free_workspace(
                    snapshot.directory(),
                    &image::source_objects()?,
                    &image::frozen::build_slots(&chip)?,
                )?;
                for class in orchestration::image_classes(&selected) {
                    let artifacts = image::frozen::build(
                        &frozen,
                        &chip,
                        class,
                        layout_seed,
                        &features.clone().unwrap_or_default(),
                    )?;
                    emit_json(&image::artifact_report(class, &artifacts)?, true)?;
                }
                return Ok(());
            }
            let firmware = match firmware_from {
                Some(run_id) => {
                    let class = orchestration::single_image_class(&selected)?;
                    RunFirmware::Replay(Box::new(oer_hil_run_bundle::verify::archived_firmware(
                        &root,
                        &chip,
                        &run_id,
                        class,
                        &oer_hil_image::record::Recipe,
                    )?))
                }
                None => RunFirmware::BuildCurrent(oer_hil_image::CurrentBuild {
                    layout_seed,
                    features: features.clone().unwrap_or_default(),
                }),
            };
            let selected = selected.iter().collect::<Vec<_>>();
            let lab = lab::config::LabConfig::load(&lab_path, &chip, &choice)?
                .with_peer_images(&peer_images(selected.iter().copied()))?;
            let required = requirements(&selected);
            for provider in crate::scenario::Families::FIXTURES {
                provider.admit(&lab, required)?;
            }
            let invocation = orchestration::Invocation {
                arguments: invocation,
                snapshot,
                then,
            };
            // Orchestration builds the images, then leases the stand and
            // the fixture software.
            match selected.as_slice() {
                [single] => {
                    orchestration::run_one(&root, &lab, &catalog, single, firmware, invocation)
                }
                _ => {
                    orchestration::run_many(&root, &lab, &catalog, &selected, firmware, invocation)
                }
            }
        }
        CliCommand::RunAll {
            tag,
            role,
            layout_seed,
            source_include,
            include_untracked,
            all: _,
            exclude,
            chip,
        } => {
            let catalog = Catalog::load(&catalog_path)?;
            let selected = crate::cli::Selection {
                scenario: None,
                tag: tag.clone(),
                role: role.clone(),
                chip: None,
            }
            .resolve(&catalog)?;
            let selected = excluding(selected, &exclude)?;
            let chip = orchestration::select_chip(&root, &selected, chip.as_deref())?;
            let lab = lab::config::LabConfig::load(&lab_path, &chip, &choice)?
                .with_peer_images(&peer_images(selected.iter().copied()))?;
            let snapshot = oer_hil_source_snapshot::capture(
                &root,
                &source_include,
                include_untracked,
                &image::source_snapshot_store()?,
                &image::source_objects()?,
            )?;
            let required = requirements(&selected);
            for provider in crate::scenario::Families::FIXTURES {
                provider.admit(&lab, required)?;
            }
            // Orchestration builds the images, then leases the stand and
            // the fixture software.
            orchestration::run_all(
                &root,
                &lab,
                &catalog,
                &selected,
                orchestration::selection_description(&tag),
                oer_hil_image::CurrentBuild {
                    layout_seed,
                    features: oer_hil_schema::image::FeatureDelta::default(),
                },
                orchestration::Invocation {
                    arguments: invocation,
                    snapshot: Some(snapshot),
                    then: None,
                },
            )
        }
    }
}

/// `selected` without the scenarios named in `exclude`, each reported.
fn excluding<'a>(
    selected: Vec<&'a crate::scenario::Scenario>,
    exclude: &[String],
) -> crate::Result<Vec<&'a crate::scenario::Scenario>> {
    let (left_out, kept): (Vec<_>, Vec<_>) = selected
        .into_iter()
        .partition(|scenario| exclude.iter().any(|id| id == scenario.id()));
    for scenario in left_out {
        eprintln!("skipping excluded scenario `{}`", scenario.id());
    }
    if kept.is_empty() {
        return Err("every selected HIL scenario is excluded".into());
    }
    Ok(kept)
}

/// The catalog images the peers of `selected` carry, each once.
fn peer_images<'a>(
    selected: impl IntoIterator<Item = &'a crate::scenario::Scenario>,
) -> Vec<&'static str> {
    let mut images = Vec::new();
    for image in selected
        .into_iter()
        .filter_map(|scenario| scenario.family.peer_image())
    {
        if !images.contains(&image.name) {
            images.push(image.name);
        }
    }
    images
}
