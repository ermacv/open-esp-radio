//! `cargo hil`: the HIL scenarios are the workspace packages whose metadata
//! says `[package.metadata.open-radio] hil-scenario = true`; their names
//! drop the `oer-hil-scenarios-` prefix. `list` prints them, `run NAME`
//! builds and runs that scenario's program with the arguments that follow.
//! The tool knows no scenario, image or result: the program's own exit code
//! is its verdict (0 pass, 1 fail, 2 incomplete). The program runs in a
//! process group the tool owns: a signal to the tool stops the group after
//! [`SCENARIO_SHUTDOWN`], and the group dies with the tool however it ends.
#![forbid(unsafe_code)]

use std::{
    process::{Command as Process, ExitCode},
    time::Duration,
};

use clap::{Parser, Subcommand};

mod scenarios;
#[cfg(test)]
mod tests;

/// How long a stopped scenario may take to finish its session: end its
/// operations, bring the board back and release its lease.
const SCENARIO_SHUTDOWN: Duration = Duration::from_secs(60);

#[derive(Parser)]
#[command(name = "cargo hil", about = "List and run the HIL scenarios")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print every scenario with its description.
    List,
    /// Build and run one scenario's program; ARGUMENTS go to it (`--help`
    /// shows its own).
    Run {
        scenario: String,
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        arguments: Vec<String>,
    },
}

fn main() -> ExitCode {
    match run(Cli::parse()) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(2)
        }
    }
}

fn run(cli: Cli) -> scenarios::Result<ExitCode> {
    let _signals = oer_os_process::install_signal_handlers()?;
    let root = oer_os_process::Checkout::discover("cargo hil")?.root;
    let found = scenarios::discover(&scenarios::metadata(&root)?)?;
    match cli.command {
        Command::List => {
            for scenario in &found {
                println!("{:<24} {}", scenario.name, scenario.description);
            }
            Ok(ExitCode::SUCCESS)
        }
        Command::Run {
            scenario,
            arguments,
        } => {
            let scenario = scenarios::find(&found, &scenario)?;
            let mut command =
                Process::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()));
            command
                .current_dir(&root)
                .args([
                    "run",
                    "--quiet",
                    "--profile",
                    "tools",
                    "-p",
                    &scenario.package,
                    "--",
                ])
                .args(arguments);
            owned(&mut command, SCENARIO_SHUTDOWN)
        }
    }
}

/// Run `command` in an owned process group; its exit code. A cancelled
/// tool gives the group `shutdown` before it kills it.
fn owned(command: &mut Process, shutdown: Duration) -> scenarios::Result<ExitCode> {
    let mut child = oer_os_process::owned::Child::spawn_with_shutdown_grace(command, shutdown)?;
    Ok(oer_os_process::exit_code(
        child.wait_forwarding_cancellation()?,
    ))
}
