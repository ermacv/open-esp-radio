//! `cargo vendor-check`: the vendor verification application. It compares vendor revisions, records and
//! checks provenance, runs the typed scenarios, regenerates and checks the
//! evidence shards, builds the comparison probes and host stands, carries
//! symbol lineage and audits the PHY archive. The gate runs it as a process.

#![forbid(unsafe_code)]
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

mod copies;
mod lineage;
use oer_os_process::Checkout;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

#[derive(Parser)]
#[command(about = "Vendor verification: pins, provenance, scenarios and evidence")]
struct Cli {
    #[arg(long, global = true)]
    root: Option<PathBuf>,
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Compare two revisions of a vendor archive function by function, or
    /// every pinned artifact with its namesake in `--baseline`.
    Diff {
        #[arg(long)]
        chip: String,
        #[arg(long, requires = "new", conflicts_with = "baseline")]
        old: Option<PathBuf>,
        #[arg(long, requires = "old")]
        new: Option<PathBuf>,
        #[arg(long)]
        baseline: Option<PathBuf>,
        /// Also list unchanged functions.
        #[arg(long)]
        all: bool,
    },
    /// Compare the MMIO behavior of every function the chip's pinned ROM
    /// and a pinned library both define, against a built vendor firmware
    /// image where one links it; writes `target/verification/<chip>/copies.json`.
    Copies {
        #[arg(long)]
        chip: String,
        #[command(flatten)]
        budget: copies::Budget,
    },
    /// Record reviewed code fingerprints of cited vendor functions.
    Provenance {
        #[arg(long)]
        chip: String,
        /// Functions whose pinned code was reviewed: symbols, or the
        /// `artifact[member]::symbol` form `check provenance` prints.
        #[arg(long, value_delimiter = ',')]
        accept: Vec<String>,
        /// Recompute the registry from the current citations.
        #[arg(long)]
        rebuild: bool,
        /// Directory of the revision the facts were observed in, for
        /// `--rebuild`.
        #[arg(long, requires = "rebuild")]
        baseline: Option<PathBuf>,
        /// Print each accepted function's annotated pinned code before
        /// recording it.
        #[arg(long, requires = "accept")]
        show: bool,
    },
    /// Build the typed vendor scenarios, then run one scenario with the
    /// forwarded arguments (for example `gain --library ...`).
    #[command(disable_help_flag = true)]
    Scenario {
        #[arg(long)]
        chip: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<OsString>,
    },
    /// Compute the chip's derived vendor evidence index for this checkout:
    /// every shard, its firmware inputs built from their recipes, or only the
    /// named scenarios' shards.
    Evidence {
        /// The chip; every verified chip's whole index when omitted.
        #[arg(long)]
        chip: Option<String>,
        /// Linker the scenarios prepare images with; the pinned Rust
        /// toolchain's LLD when omitted.
        #[arg(long)]
        linker: Option<PathBuf>,
        /// Ignored output root of the scenario runs.
        #[arg(long, default_value = "target/blobray-research/evidence")]
        output: PathBuf,
        /// Print every vendor location the index leaves untriaged that no
        /// other scenario covers, without running a scenario.
        #[arg(long)]
        untriaged: bool,
        /// Compare every computed index with the one `main` computed at the
        /// change's merge base, below this directory as
        /// `<chip>/evidence`; fail when evidence was lost. An absent
        /// directory skips the comparison.
        #[arg(long, conflicts_with_all = ["chip", "untriaged", "scenarios"])]
        compare: Option<PathBuf>,
        /// Scenarios whose shards to rewrite; the whole index when empty.
        scenarios: Vec<String>,
    },
    /// Build the chip's Rust comparison probe images.
    Probes {
        #[arg(long)]
        chip: String,
        #[arg(long)]
        list_roles: bool,
    },
    Check {
        #[command(subcommand)]
        check: Check,
    },
    /// Carry source function names from the earliest archive revision to
    /// the last one.
    Lineage(lineage::Arguments),
    /// Audit the symbols of a PHY archive (an rlib); each `--source` names
    /// a crate the source-only code may call into.
    PhyArchive {
        archive: PathBuf,
        #[arg(long = "source", value_name = "CRATE")]
        sources: Vec<String>,
    },
}

#[derive(Subcommand)]
enum Check {
    /// Check that every vendor function production and the register model
    /// cite is registered with its reviewed, still pinned code; fetches the
    /// pins first when one is missing. Every verified chip without `--chip`.
    Provenance {
        #[arg(long)]
        chip: Option<String>,
    },
    /// Build every verified chip's comparison probe images.
    Probes,
    /// Clippy and the tests of each verified chip's vendor host stands, at
    /// the workspace's lint levels: an error fails, a warning (such as an
    /// existing panic site) does not.
    HostStands,
}

/// The chips with pinned vendor artifacts.
fn verified_chips(ctx: &Checkout) -> Result<Vec<String>> {
    Ok(oer_pins::artifacts::project::projects(&ctx.root)?)
}

fn provenance(ctx: &Checkout, chip: &str) -> Result<()> {
    if !oer_pins::artifacts::unfetched(&ctx.root, chip)?.is_empty() {
        oer_pins::artifacts::fetch_vendor_sources(&ctx.root, chip)?;
    }
    oer_vendor_provenance::registry::check(&ctx.root, chip)
}

/// Clippy of a host stand's manifest, at the workspace's lint levels.
const STAND_CLIPPY: [&str; 4] = ["clippy", "--locked", "--all-targets", "--manifest-path"];

fn host_stands(ctx: &Checkout) -> Result<()> {
    let workspace = oer_vendor_evidence::workspace::Workspace::load(&ctx.root, None)?;
    for chip in verified_chips(ctx)? {
        for stand in
            oer_vendor_evidence::producer::host_stand::stands(&workspace, &chip).into_values()
        {
            for manifest in [stand.library, stand.program] {
                let manifest = ctx.root.join(manifest);
                oer_os_process::run(
                    oer_os_toolchain::cargo_in(&ctx.root)
                        .args(STAND_CLIPPY)
                        .arg(&manifest),
                )?;
                oer_os_process::run(
                    oer_os_toolchain::cargo_in(&ctx.root)
                        .args(["test", "--locked", "--no-fail-fast", "--manifest-path"])
                        .arg(&manifest),
                )?;
            }
        }
    }
    Ok(())
}

fn run() -> Result<ExitCode> {
    let cli = Cli::parse();
    let ctx = match cli.root {
        Some(root) => Checkout::new(root)?,
        None => Checkout::discover("vendor-check")?,
    };
    let _signals = oer_os_process::install_signal_handlers()?;
    match cli.command {
        Command::Lineage(arguments) => lineage::run(arguments)?,
        Command::PhyArchive { archive, sources } => {
            let sources: Vec<_> = sources
                .iter()
                .map(|source| source.replace('-', "_"))
                .collect();
            oer_vendor_phy::archive::audit_phy(&ctx, &archive, &sources)?
        }
        Command::Diff {
            chip,
            old,
            new,
            baseline,
            all,
        } => oer_vendor_provenance::diff::run(&ctx.root, &chip, old, new, baseline, all)?,
        Command::Copies { chip, budget } => copies::run(&ctx.root, &chip, &budget)?,
        Command::Provenance {
            chip,
            accept,
            rebuild,
            baseline,
            show,
        } => {
            let show_code = |symbol: &str| -> oer_vendor_provenance::Result<()> {
                let code = oer_vendor_evidence::run::scenario::run(
                    &ctx,
                    &chip,
                    &["show".into(), symbol.into()],
                )?;
                if code == ExitCode::SUCCESS {
                    Ok(())
                } else {
                    Err(format!("scenario show {symbol} failed").into())
                }
            };
            oer_vendor_provenance::registry::update(
                &ctx.root,
                &chip,
                &accept,
                rebuild,
                baseline,
                show.then_some(&show_code as &oer_vendor_provenance::registry::Show<'_>),
            )?
        }
        Command::Scenario { chip, args } => {
            return oer_vendor_evidence::run::scenario::run(&ctx, &chip, &args);
        }
        Command::Evidence {
            chip,
            linker,
            output,
            untriaged,
            compare,
            scenarios,
        } => {
            use oer_vendor_evidence::run::regenerate;
            let Some(chip) = chip else {
                if untriaged || !scenarios.is_empty() {
                    return Err("naming scenarios or `--untriaged` needs `--chip`".into());
                }
                // Every verified chip's whole index, as the vendor-evidence check runs it.
                let chips = verified_chips(&ctx)?;
                for chip in &chips {
                    regenerate::run(&ctx, chip, vec![], linker.clone(), output.clone())?;
                }
                let Some(base) = compare.map(|base| ctx.root.join(base)) else {
                    return Ok(ExitCode::SUCCESS);
                };
                if !base.is_dir() {
                    println!(
                        "no base vendor evidence index at {}: the comparison is skipped",
                        base.display()
                    );
                    return Ok(ExitCode::SUCCESS);
                }
                let mut lost = false;
                for chip in &chips {
                    lost |= regenerate::compare(&ctx, chip, &base.join(chip).join("evidence"))?;
                }
                return if lost {
                    Err("the change loses vendor evidence the merge base's index holds".into())
                } else {
                    println!("no vendor evidence of the merge base's index is lost");
                    Ok(ExitCode::SUCCESS)
                };
            };
            return if untriaged {
                regenerate::untriaged(&ctx, &chip)
            } else {
                regenerate::run(&ctx, &chip, scenarios, linker, output)
            };
        }
        Command::Probes { chip, list_roles } => {
            oer_vendor_evidence::run::probes::run(&ctx, &chip, list_roles)?
        }
        Command::Check { check } => match check {
            Check::Provenance { chip: Some(chip) } => provenance(&ctx, &chip)?,
            Check::Provenance { chip: None } => {
                for chip in verified_chips(&ctx)? {
                    provenance(&ctx, &chip)?;
                }
            }
            Check::Probes => {
                for chip in verified_chips(&ctx)? {
                    oer_vendor_evidence::run::probes::run(&ctx, &chip, false)?;
                }
            }
            Check::HostStands => host_stands(&ctx)?,
        },
    }
    Ok(ExitCode::SUCCESS)
}

fn main() -> ExitCode {
    if oer_os_command_tree::requested() {
        use clap::CommandFactory as _;
        let tree =
            oer_os_command_tree::command_tree(&Cli::command(), &[String::from("vendor-check")]);
        println!("{}", oer_os_command_tree::json(&tree));
        return ExitCode::SUCCESS;
    }
    match run() {
        Ok(status) => status,
        Err(error) => {
            eprintln!("vendor-check: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_stand_keeps_the_workspace_lint_levels() {
        // A warning, such as an existing panic site, never fails the check.
        assert!(!STAND_CLIPPY.iter().any(|arg| *arg == "-D" || *arg == "--"));
    }

    #[test]
    fn the_comparison_covers_every_chip_and_never_one() {
        let parse =
            |args: &[&str]| Cli::try_parse_from([&["vendor-check", "evidence"], args].concat());
        assert!(parse(&["--compare", "base"]).is_ok());
        for args in [
            &["--chip", "esp32s31", "--compare", "base"][..],
            &["--untriaged", "--compare", "base"],
            &["wifi-mac", "--compare", "base"],
        ] {
            assert!(parse(args).is_err(), "{args:?}");
        }
    }
}
