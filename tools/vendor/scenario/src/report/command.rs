//! The command line of the typed vendor-comparison scenarios and the shared
//! reviewer commands. The binary's `main` calls [`main`] with a chip's
//! scenarios: the scenario engine serves one chip per process.
use crate::report::{Reviewer, inspect};
use clap::{Parser, Subcommand};
use std::ffi::OsString;
use std::process::ExitCode;

#[derive(Parser)]
#[command(about = "Typed vendor-comparison scenarios over the Blobray CLI")]
struct ChipCli<S: Subcommand> {
    #[command(subcommand)]
    command: Command<S>,
}

#[derive(Subcommand)]
enum Command<S: Subcommand> {
    #[command(flatten)]
    Scenario(Box<S>),
    #[command(flatten)]
    Inspect(inspect::Command),
}

/// Install the chip's scenarios with `install`, then parse the
/// process arguments as the command line of `name` and run the command;
/// `run` executes a scenario.
pub fn main<S: Subcommand>(
    name: &str,
    install: fn(),
    run: impl FnOnce(S, &Reviewer) -> ExitCode,
) -> ExitCode {
    install();
    let arguments = std::env::args_os().skip(1);
    let cli = ChipCli::<S>::parse_from(std::iter::once(OsString::from(name)).chain(arguments));
    match cli.command {
        Command::Scenario(scenario) => run(*scenario, &Reviewer),
        Command::Inspect(command) => match inspect::run(command) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        },
    }
}
