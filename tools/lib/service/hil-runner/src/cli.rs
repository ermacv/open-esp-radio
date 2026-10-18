//! Public command grammar; workload APIs accept typed configuration.

use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "cargo hil",
    about = "Open ESP radio hardware-in-the-loop runner"
)]
pub(crate) struct Cli {
    /// The stand file (`~/.config/open-esp-radio/stand.toml` by default).
    /// Secrets never belong to scenarios.
    #[arg(long, global = true)]
    pub(crate) stand_file: Option<PathBuf>,
    /// The device under test, by its stand-file id, when the pool has
    /// several boards of the chip.
    #[arg(long, global = true)]
    pub(crate) board: Option<String>,
    /// The peer board, by its stand-file id, when the pool has several.
    #[arg(long, global = true)]
    pub(crate) peer_board: Option<String>,
    #[command(subcommand)]
    pub(crate) command: CliCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum CliCommand {
    /// Check the selected scenarios' tools and fixture, without resetting hardware.
    Doctor(Selection),
    /// Apply and verify a scenario's fixture, then restore it. Never accesses the DUT.
    Fixture {
        #[command(subcommand)]
        command: FixtureCommand,
    },
    /// Resolve scenario requirements offline, without opening a device or the stand file.
    Plan {
        #[command(flatten)]
        selection: Selection,
        /// Require every named check; controls are added only after this filter.
        #[arg(long = "proof")]
        proofs: Vec<String>,
    },
    /// Inspect and validate the host-owned scenario catalog.
    Scenario {
        #[command(subcommand)]
        command: ScenarioCommand,
    },
    /// Snapshot the sources of, or build, reproducible firmware classes.
    Image {
        #[command(subcommand)]
        command: ImageCommand,
    },
    /// Check sealed run bundles offline.
    Report {
        #[command(subcommand)]
        command: ReportCommand,
    },
    /// Build, flash and execute catalog scenarios in one run under one stand
    /// lease. Images are built before the run waits for the stand.
    Run {
        /// Scenarios in execution order within each image class.
        #[arg(required = true, num_args = 1..)]
        scenarios: Vec<String>,
        /// Explicit nonignored untracked source file; repeat for each included file.
        #[arg(
            long = "source-include",
            value_name = "FILE",
            conflicts_with = "firmware_from"
        )]
        source_include: Vec<String>,
        /// Archive every untracked file inside a package the image builds,
        /// recorded as such in the snapshot manifest.
        #[arg(long, conflicts_with = "firmware_from")]
        include_untracked: bool,
        /// Build from this verified source snapshot directory (`image
        /// snapshot`, possibly of another checkout) instead of capturing the
        /// live checkout.
        #[arg(
            long,
            value_name = "DIR",
            conflicts_with_all = ["firmware_from", "source_include", "include_untracked"]
        )]
        source_snapshot: Option<PathBuf>,
        /// Use the scenarios' image class from a sealed earlier HIL run; every
        /// selected scenario must use that class.
        #[arg(long, value_name = "RUN_ID")]
        firmware_from: Option<String>,
        /// Shuffle the runtime's code and read-only data by this nonzero
        /// seed, recorded with the image; without it the linker keeps its
        /// natural order.
        #[arg(long, value_name = "SEED", conflicts_with = "firmware_from")]
        layout_seed: Option<std::num::NonZeroU32>,
        /// Runtime features added to (`+name`) or removed from (`-name`) each
        /// image class's own, comma-separated. Only an A/B experiment builds
        /// such an image; it never qualifies.
        #[arg(long, value_name = "+F,-G", conflicts_with = "firmware_from")]
        features: Option<oer_hil_schema::image::FeatureDelta>,
        /// Run each selected scenario this many times instead of its own
        /// repetitions, for a quicker look while debugging. The run records
        /// the reduced count and is never noted as qualification evidence.
        #[arg(long, value_name = "N", value_parser = clap::value_parser!(u8).range(1..=20))]
        repetitions: Option<u8>,
        /// Shell command run after the scenarios while the run still holds
        /// its stand lease, for example to read the board's reset reason.
        /// It may use the boards' ports directly; nested `cargo hil` commands
        /// join the lease. Its exit status does not change the run's outcome.
        #[arg(long, value_name = "COMMAND")]
        then: Option<String>,
        /// Chip whose device under test runs the scenarios; default: the only
        /// chip whose HIL agent builds every image they use.
        #[arg(long, value_name = "CHIP")]
        chip: Option<String>,
        /// Check the scenarios, chip and options, then exit without a
        /// snapshot, a build or a run: what `--enqueue` checks at once.
        #[arg(long, hide = true)]
        validate_only: bool,
        /// Build and audit every image the scenarios use from their sources,
        /// then exit without a run: `--enqueue` builds this way before the
        /// job waits, so a failing build or audit shows at once.
        #[arg(long, hide = true, conflicts_with_all = ["firmware_from", "validate_only"])]
        build_only: bool,
    },
    /// Execute catalog scenarios, flashing once per selected image class.
    /// The whole catalog runs only with an explicit `--all`.
    #[command(group(clap::ArgGroup::new("selection").required(true).args(["tag", "role", "all"])))]
    RunAll {
        /// Explicit nonignored untracked source file; repeat for each included file.
        #[arg(long = "source-include", value_name = "FILE")]
        source_include: Vec<String>,
        /// Archive every untracked file inside a package the image builds,
        /// recorded as such in the snapshot manifest.
        #[arg(long)]
        include_untracked: bool,
        /// Shuffle the runtime's code and read-only data by this nonzero
        /// seed, recorded with the image; without it the linker keeps its
        /// natural order.
        #[arg(long, value_name = "SEED")]
        layout_seed: Option<std::num::NonZeroU32>,
        /// Select only scenarios carrying this tag. May be repeated.
        #[arg(long)]
        tag: Vec<String>,
        /// Select only scenarios of this role: `qualification` runs every
        /// scenario a qualification program references.
        #[arg(long, value_parser = ["qualification", "investigation"])]
        role: Option<String>,
        /// Run every catalog scenario: the whole default suite.
        #[arg(long)]
        all: bool,
        /// Leave this scenario out, such as a quarantined one. May be
        /// repeated.
        #[arg(long, value_name = "SCENARIO")]
        exclude: Vec<String>,
        /// Chip whose device under test runs the scenarios; default: the only
        /// chip whose HIL agent builds every image they use.
        #[arg(long, value_name = "CHIP")]
        chip: Option<String>,
    },
}

#[derive(Debug, Args)]
pub(crate) struct Selection {
    /// One scenario; omission selects the catalog, optionally filtered by tags.
    #[arg(conflicts_with = "tag")]
    pub(crate) scenario: Option<String>,
    /// Require every supplied tag. May be repeated.
    #[arg(long)]
    pub(crate) tag: Vec<String>,
    /// Require this role: `qualification` selects the scenarios a
    /// qualification program references, `investigation` every other one.
    #[arg(long, conflicts_with = "scenario", value_parser = ["qualification", "investigation"])]
    pub(crate) role: Option<String>,
    /// Chip whose device under test the selection is for; default: the only
    /// chip whose HIL agent builds every image the selection uses.
    #[arg(long, value_name = "CHIP")]
    pub(crate) chip: Option<String>,
}

impl Selection {
    pub(crate) fn resolve<'a>(
        &self,
        catalog: &'a crate::scenario::Catalog,
    ) -> crate::Result<Vec<&'a crate::scenario::Scenario>> {
        if let Some(id) = &self.scenario {
            let selected = vec![catalog.get(id)?];
            crate::execution::orchestration::refuse_unsupported(&selected)?;
            return Ok(selected);
        }
        let (selected, unsupported): (Vec<_>, Vec<_>) = catalog
            .all()
            .iter()
            .filter(|entry| self.tag.iter().all(|tag| entry.header.tags.contains(tag)))
            .filter(|entry| {
                self.role.as_deref().is_none_or(|role| {
                    serde_json::to_value(entry.header.role).ok().as_ref()
                        == Some(&serde_json::Value::from(role))
                })
            })
            .partition(|entry| entry.header.unsupported.is_none());
        for scenario in unsupported {
            eprintln!(
                "skipping unsupported scenario `{}`: {}",
                scenario.id(),
                scenario.header.unsupported.as_deref().unwrap_or_default()
            );
        }
        if selected.is_empty() {
            return Err("no HIL scenarios match the requested tags and role".into());
        }
        Ok(selected)
    }
}

#[derive(Debug, Subcommand)]
pub(crate) enum ScenarioCommand {
    List,
    Validate { scenario: Option<String> },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ImageCommand {
    /// Capture exact source inputs offline. Does not build, flash or qualify firmware.
    Snapshot {
        /// Explicit untracked file: repository-relative path or ROLE:path for a local override.
        #[arg(long = "source-include", value_name = "FILE")]
        source_include: Vec<String>,
        /// Archive every untracked file inside a package the image builds,
        /// recorded as such in the snapshot manifest.
        #[arg(long)]
        include_untracked: bool,
    },
    Build {
        /// Image classes, built one after the other; with a source snapshot
        /// they share one materialization of it.
        #[arg(required = true, num_args = 1..)]
        classes: Vec<oer_hil_schema::image::ImageClass>,
        /// Build only from a verified source snapshot directory, not the live checkout.
        #[arg(long)]
        source_snapshot: Option<PathBuf>,
        /// Shuffle the runtime's code and read-only data by this nonzero
        /// seed, recorded with the image; without it the linker keeps its
        /// natural order.
        #[arg(long, value_name = "SEED")]
        layout_seed: Option<std::num::NonZeroU32>,
        /// The chip to build for; without it, the one chip whose HIL agent
        /// builds every class.
        #[arg(long)]
        chip: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ReportCommand {
    /// Verify one run bundle, or every bundle when RUN_ID is omitted.
    Verify {
        run_id: Option<String>,
        /// Only this chip's runs; default: every chip's.
        #[arg(long, value_name = "CHIP")]
        chip: Option<String>,
    },
}

#[cfg(test)]
mod tests;

#[derive(Debug, Subcommand)]
pub(crate) enum FixtureCommand {
    /// Validate prerequisites and exercise AP preparation/restoration without firmware or serial.
    Check {
        scenario: String,
        /// Chip whose device under test the scenario would run on; default:
        /// the only chip whose HIL agent builds its image.
        #[arg(long, value_name = "CHIP")]
        chip: Option<String>,
    },
}
