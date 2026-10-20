//! Run one catalog scenario against the compiled vendor driver and print its
//! boundary trace, one record per line, or compare it with the production
//! engine and print the verdict; or compare every scenario, each in its own
//! process, and print its verdict.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use std::panic::{AssertUnwindSafe, catch_unwind};

use oer_vendor_ieee802154_esp32s31::{catalog, claim, compare_scenario, port};

#[derive(clap::Parser)]
#[command(name = "oer-tool-vendor-ieee802154-esp32s31-cli")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Subcommand)]
enum Command {
    List,
    Run {
        scenario: String,
        #[arg(hide = true, trailing_var_arg = true, allow_hyphen_values = true)]
        ignored: Vec<std::ffi::OsString>,
    },
    Port {
        scenario: String,
        #[arg(hide = true, trailing_var_arg = true, allow_hyphen_values = true)]
        ignored: Vec<std::ffi::OsString>,
    },
    Compare {
        scenario: String,
        #[arg(hide = true, trailing_var_arg = true, allow_hyphen_values = true)]
        ignored: Vec<std::ffi::OsString>,
    },
}

fn main() -> ExitCode {
    use clap::Parser as _;
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            return ExitCode::FAILURE;
        }
    };
    if matches!(cli.command, Command::Compare { ref scenario, .. } if scenario == "all") {
        return match compare_all() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        };
    }
    let name = match &cli.command {
        Command::List => {
            for scenario in catalog::SCENARIOS {
                println!("{}", scenario.name);
            }
            return ExitCode::SUCCESS;
        }
        Command::Run { scenario, .. }
        | Command::Port { scenario, .. }
        | Command::Compare { scenario, .. } => scenario,
    };
    let Some(scenario) = catalog::SCENARIOS
        .iter()
        .find(|scenario| scenario.name == name)
    else {
        eprintln!("unknown scenario: {name}");
        return ExitCode::FAILURE;
    };
    if matches!(cli.command, Command::Port { .. }) {
        return match port::run(scenario) {
            Ok(records) => {
                for record in records {
                    println!("{record}");
                }
                ExitCode::SUCCESS
            }
            Err(reason) => {
                eprintln!("{reason}");
                ExitCode::FAILURE
            }
        };
    }
    let vendor = claim(scenario);
    if matches!(cli.command, Command::Run { .. }) {
        for record in vendor {
            println!("{record}");
        }
        return ExitCode::SUCCESS;
    }
    // A panic cannot be accepted as a catalogued difference.
    let port = catch_unwind(AssertUnwindSafe(|| port::run(scenario)))
        .unwrap_or_else(|_| Err("the engine panicked".to_owned()));
    let verdict = compare_scenario(scenario, &vendor, port);
    println!("{verdict}");
    ExitCode::from(verdict.exit_code())
}

type Result<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Compare every catalog scenario in its own process.
fn compare_all() -> Result<()> {
    let program = std::env::current_exe()?;
    compare_scenarios(|name| {
        let status = oer_os_process::command(&program)
            .args(["compare", name])
            .status()?;
        if !status.success() {
            return Err(format!("{name} did not compare MATCH or EXPECTED ({status})").into());
        }
        Ok(())
    })?;
    println!("all IEEE 802.15.4 host scenarios passed");
    Ok(())
}

fn compare_scenarios(mut run: impl FnMut(&str) -> Result<()>) -> Result<()> {
    for scenario in catalog::SCENARIOS {
        run(scenario.name)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_grammar_accepts_the_catalog_commands() {
        use clap::{CommandFactory as _, Parser as _};
        Cli::command().debug_assert();
        for args in [
            vec!["vendor", "list"],
            vec!["vendor", "run", "scenario"],
            vec!["vendor", "port", "scenario"],
            vec!["vendor", "compare", "scenario"],
            vec!["vendor", "compare", "all"],
            vec!["vendor", "compare", "all", "ignored", "--tail"],
        ] {
            assert!(Cli::try_parse_from(args).is_ok());
        }
        for args in [
            vec!["vendor"],
            vec!["vendor", "run"],
            vec!["vendor", "list", "extra"],
        ] {
            assert!(Cli::try_parse_from(args).is_err());
        }
    }

    #[test]
    fn every_catalog_scenario_is_compared_and_the_first_failure_stops_the_run() {
        let mut names = vec![];
        compare_scenarios(|name| {
            names.push(name.to_owned());
            Ok(())
        })
        .expect("every scenario matched");
        assert_eq!(
            names,
            catalog::SCENARIOS
                .iter()
                .map(|s| s.name)
                .collect::<Vec<_>>()
        );
        let mut calls = 0;
        let error = compare_scenarios(|_| {
            calls += 1;
            Err("DIFF".into())
        })
        .expect_err("a failed comparison fails the run");
        assert_eq!(calls, 1);
        assert_eq!(error.to_string(), "DIFF");
    }
}
