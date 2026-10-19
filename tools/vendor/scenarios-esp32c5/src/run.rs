//! Run one authenticated ESP32-C5 vendor-comparison scenario: the scenario
//! set, its inputs and the dispatch that decides its verdicts. The binary adds the reviewer commands.
use crate::{LIBRARY, ROM, decisions, phy_i2c};
use clap::Subcommand;
use oer_vendor_scenario::findings::RunReport;
use oer_vendor_scenario::harness::{Budget, Result};
use oer_vendor_scenario::leaf::{self, LeafOptions};
use oer_vendor_scenario::{artifacts, observation, session, state};
use std::path::PathBuf;
use std::process::ExitCode;

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
    };
    let mut run = leaf::LeafRun::new(&options)?;
    leaf::exercise(&mut run)?;
    let claims =
        run.session
            .claims(phy_i2c::PHY_I2C.id, &run.roots, &leaf::claims(&run), report)?;
    println!(
        "authenticated PHY I2C transport leaves passed {}",
        run.run.display()
    );
    Ok(claims)
}

/// Every scenario; each reviewed decision must still review something.
fn all(common: &Common, report: &dyn RunReport) -> Result<()> {
    let claims = phy_i2c(common, report)?;
    let root = oer_os_process::built_root();
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
    state::check(decisions::state::DECISIONS, &claims.unprojected)?;
    Ok(())
}

/// Run `scenario`, handing each suite's findings to `report`.
pub fn run(scenario: Scenario, report: &dyn RunReport) -> ExitCode {
    let result = match scenario {
        Scenario::PhyI2c { common } => phy_i2c(&common, report).map(|_| ()),
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

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scenarios_accept_their_inputs_and_reject_an_index() {
        #[derive(clap::Parser)]
        struct Cli {
            #[command(subcommand)]
            scenario: Scenario,
        }
        use clap::Parser as _;
        crate::install();
        let args = [
            "scenario",
            "phy-i2c",
            "--production",
            "probe.elf",
            "--linker",
            "ld.lld",
            "--output",
            "runs",
        ];
        assert!(Cli::try_parse_from(args).is_ok());
        let error = Cli::try_parse_from([&args[..], &["--index", "index"]].concat())
            .err()
            .expect("index rejected");
        assert_eq!(error.kind(), clap::error::ErrorKind::UnknownArgument);
    }
}
