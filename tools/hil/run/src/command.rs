//! CLI command dispatch with command-specific ordering and side-effect boundaries.

use crate::image;
use std::env;

use clap::Parser as _;

use crate::scenario::{Catalog, requirements};
use crate::{
    Result, cli::Cli, cli::CliCommand, cli::ImageCommand, cli::ScenarioCommand, emit_json,
    execution::firmware::RunFirmware, execution::orchestration,
};

use oer_hil_lab as lab;
use oer_hil_model::catalog::SCENARIO_SCHEMA;
use oer_hil_scenario::ScenarioFamily as _;
use oer_hil_scenario::workload::family::Registry as _;
use oer_hil_scenario::workload::output;

pub(crate) fn run() -> Result<()> {
    let root = oer_os_process::built_root();
    let invocation = env::args_os().collect::<Vec<_>>();
    let cli = Cli::parse();
    output::reserve_machine_stdout()?;
    let _signals = oer_os_process::install_signal_handlers()?;
    let command = cli.command;
    let lab_path = cli
        .stand_file
        .unwrap_or(lab::config::LabConfig::default_path()?);
    let choice = lab::config::BoardChoice {
        dut: cli.board,
        peer: cli.peer_board,
    };
    let catalog_path = root.join("tools/hil/scenarios");

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
                oer_hil_model::image_class::NETWORK,
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
            ImageCommand::Build {
                classes,
                layout_seed,
                chip,
            } => {
                let chip = image::chip_for(&root, &classes, chip.as_deref())?;
                for class in classes {
                    let artifacts = image::build(
                        &root,
                        &chip,
                        class,
                        layout_seed,
                        &oer_hil_model::schema::image::FeatureDelta::default(),
                    )?;
                    emit_json(&image::artifact_report(class, &artifacts)?, true)?;
                }
                Ok(())
            }
        },
        CliCommand::Run {
            scenarios,
            source,
            images,
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
                && oer_hil_runs::format::experiment::Experiment::from_environment()?.is_none()
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
            let tree = source.clone().unwrap_or_else(|| root.clone());
            if let Some(directory) = build_only {
                let selected = selected.iter().collect::<Vec<_>>();
                let mut built = image::images::Images::create(
                    &directory,
                    &tree,
                    &chip,
                    layout_seed,
                    &features.clone().unwrap_or_default(),
                )?;
                let mut classes = orchestration::image_classes(&selected);
                // The run may need the recovery image on a silent board.
                if !classes.contains(&orchestration::RECOVERY_IMAGE) {
                    classes.push(orchestration::RECOVERY_IMAGE);
                }
                for class in classes {
                    let artifacts = built.build(&tree, class)?;
                    emit_json(&image::artifact_report(class, &artifacts)?, true)?;
                }
                return Ok(());
            }
            let firmware = match firmware_from {
                Some(run_id) => {
                    let class = orchestration::single_image_class(&selected)?;
                    RunFirmware::Replay(Box::new(oer_hil_runs::replay::archived_firmware(
                        &root, &chip, &run_id, class,
                    )?))
                }
                None => RunFirmware::BuildCurrent(match &images {
                    Some(directory) => {
                        let built = image::images::Images::open(directory)?;
                        crate::image::CurrentBuild {
                            source: crate::image::ImageSource::Built(directory.clone()),
                            layout_seed: built.manifest().layout_seed,
                            features: built.manifest().features.clone(),
                        }
                    }
                    None => crate::image::CurrentBuild {
                        source: crate::image::ImageSource::Tree(tree),
                        layout_seed,
                        features: features.clone().unwrap_or_default(),
                    },
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
                crate::image::CurrentBuild {
                    source: crate::image::ImageSource::Tree(root.clone()),
                    layout_seed,
                    features: oer_hil_model::schema::image::FeatureDelta::default(),
                },
                orchestration::Invocation {
                    arguments: invocation,
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
