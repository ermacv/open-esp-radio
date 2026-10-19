//! The command line of one chip's typed vendor-comparison scenarios and the
//! shared reviewer commands. Each chip's scenario package is its own binary,
//! whose `main` calls [`main`] with its library: the scenario engine serves
//! one chip per process, and no binary links another chip's scenarios.
use clap::{Parser, Subcommand};
use oer_vendor_scenario_report::{Reviewer, inspect};
use std::ffi::OsString;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(about = "Typed vendor-comparison scenarios over the Blobray CLI")]
struct ChipCli<S: Subcommand> {
    #[command(subcommand)]
    command: Command<S>,
    /// Dep-info file of a library that decides the verdicts; `cargo
    /// verification scenario` passes one for each, and shards are written
    /// only with them.
    #[arg(long = "verdict-dep-info", global = true, hide = true)]
    verdict_dep_info: Vec<PathBuf>,
}

#[derive(Subcommand)]
enum Command<S: Subcommand> {
    #[command(flatten)]
    Scenario(Box<S>),
    #[command(flatten)]
    Inspect(inspect::Command),
}

/// Install the chip's scenario library with `install`, then parse the
/// process arguments as the command line of `name` and run the command;
/// `run` executes a scenario.
pub fn main<S: Subcommand>(
    name: &str,
    install: fn(),
    run: impl FnOnce(S, &Reviewer, Vec<PathBuf>) -> ExitCode,
) -> ExitCode {
    install();
    let arguments = std::env::args_os().skip(1);
    let cli = ChipCli::<S>::parse_from(std::iter::once(OsString::from(name)).chain(arguments));
    match cli.command {
        Command::Scenario(scenario) => run(*scenario, &Reviewer, cli.verdict_dep_info),
        Command::Inspect(command) => match inspect::run(command) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        },
    }
}
