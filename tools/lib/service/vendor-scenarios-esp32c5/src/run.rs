//! Run one authenticated ESP32-C5 vendor-comparison scenario: the scenario
//! set, its inputs and the dispatch that decides its verdicts and evidence
//! shard. The binary adds the reviewer commands.
use crate::{LIBRARY, PROBES_MANIFEST, PROBES_PACKAGE, ROM, decisions, phy_i2c};
use clap::Subcommand;
use oer_vendor_scenario_engine::findings::RunReport;
use oer_vendor_scenario_engine::harness::{Budget, Result};
use oer_vendor_scenario_engine::leaf::{self, LeafOptions};
use oer_vendor_scenario_engine::shard::{self, ProbeImages};
use oer_vendor_scenario_engine::{artifacts, observation, session, state};
use std::path::PathBuf;
use std::process::ExitCode;

/// Target triple of the ESP32-C5 probe images.
const PROBES_TARGET: &str = "riscv32imac-unknown-none-elf";
/// The probe images of the ESP32-C5 scenarios.
const PROBES: ProbeImages = ProbeImages {
    manifest: PROBES_MANIFEST,
    packages: &[PROBES_PACKAGE],
    target: PROBES_TARGET,
};

/// One vendor-comparison scenario and its inputs.
#[derive(Subcommand)]
pub enum Scenario {
    /// The analog-register I2C transport of `libphy.a[phy_i2c.o]`.
    PhyI2c {
        #[command(flatten)]
        common: Common,
    },
    /// Every scenario, with the reviewed decisions checked for staleness.
    All {
        #[command(flatten)]
        common: Common,
    },
}

#[derive(clap::Args)]
pub struct Common {
    /// Pinned `libphy.a`.
    #[arg(long, default_value_os_t = artifacts::default_path(LIBRARY))]
    library: PathBuf,
    /// Pinned ROM ELF of the stand's chip revision.
    #[arg(long, default_value_os_t = artifacts::default_path(ROM))]
    rom: PathBuf,
    /// Freshly built production probe ELF.
    #[arg(long)]
    production: PathBuf,
    /// External linker selected for image preparation.
    #[arg(long)]
    linker: PathBuf,
    /// Ignored output root; each run creates a new `run-*` directory.
    #[arg(long)]
    output: PathBuf,
    #[command(flatten)]
    budget: Budget,
    /// Write each scenario's shard of the evidence index into this directory.
    #[arg(long)]
    index: Option<PathBuf>,
    /// Dep-info files of the libraries that decide the verdicts, from the
    /// binary's `--verdict-dep-info`.
    #[arg(skip)]
    verdict: Vec<PathBuf>,
}

/// The transport suite: every leaf must MATCH and meet its expectation.
fn phy_i2c(common: &Common, report: &dyn RunReport) -> Result<session::Claims> {
    let options = LeafOptions {
        suite: &phy_i2c::PHY_I2C,
        archives: vec![common.library.clone()],
        rom: common.rom.clone(),
        firmware: None,
        production: common.production.clone(),
        linker: common.linker.clone(),
        output: common.output.clone(),
        budget: common.budget,
        patches: vec![],
    };
    let mut run = leaf::LeafRun::new(&options)?;
    leaf::exercise(&mut run)?;
    let claims = run.session.claims(
        phy_i2c::PHY_I2C.id,
        &run.roots,
        &leaf::claims(&run),
        oer_vendor_scenario_engine::coverage::decisions()?,
        report,
    )?;
    println!(
        "authenticated PHY I2C transport leaves passed {}",
        run.run.display()
    );
    Ok(claims)
}

/// Write `scenario`'s shard when an index directory was given.
fn record(common: &Common, scenario: &str, claims: &session::Claims) -> Result<()> {
    if let Some(directory) = &common.index {
        let index = shard::shard(
            scenario,
            &common.production,
            claims,
            &PROBES,
            env!("CARGO_PKG_NAME"),
            &common.verdict,
        )?;
        shard::record(directory, &index)?;
    }
    Ok(())
}

/// Every scenario; each reviewed decision must still review something.
fn all(common: &Common, report: &dyn RunReport) -> Result<()> {
    let claims = phy_i2c(common, report)?;
    let root = oer_process::built_root();
    let unobserved = claims.lines.unobserved();
    let mut sources = observation::Sources::default();
    sources.check(&root, decisions::observation::DECISIONS, &unobserved)?;
    let (reviewed, untriaged) =
        sources.classify(&root, decisions::observation::DECISIONS, &unobserved)?;
    println!(
        "{} of {} executed production hardware lines observed; {} unobserved reviewed, {} untriaged",
        claims.lines.observed.len(),
        claims.lines.executed.len(),
        reviewed.len(),
        untriaged.len(),
    );
    for line in &untriaged {
        println!("  untriaged line {}:{}", line.0.display(), line.1);
    }
    oer_vendor_scenario_engine::coverage::Observed::of(&claims.closures)
        .check("all", oer_vendor_scenario_engine::coverage::decisions()?)?;
    state::check(decisions::state::DECISIONS, &claims.unprojected)?;
    record(common, "phy-i2c", &claims)
}

/// Run `scenario`, handing each suite's findings to `report`; `verdict`
/// names the dep-info files of the libraries that decide its verdicts.
pub fn run(mut scenario: Scenario, report: &dyn RunReport, verdict: Vec<PathBuf>) -> ExitCode {
    match &mut scenario {
        Scenario::PhyI2c { common } | Scenario::All { common } => common.verdict = verdict,
    }
    let result = match scenario {
        Scenario::PhyI2c { common } => {
            phy_i2c(&common, report).and_then(|claims| record(&common, "phy-i2c", &claims))
        }
        Scenario::All { common } => all(&common, report),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::FAILURE
        }
    }
}
