//! Run one catalog scenario against the compiled vendor driver and print its
//! boundary trace, one record per line, or compare it with the production
//! engine and print the verdict; or compare every scenario, each in its own
//! process, and print its verdict.

#![forbid(unsafe_code)]

use std::process::ExitCode;

use std::panic::{AssertUnwindSafe, catch_unwind};

use oer_vendor_ieee802154_esp32s31::{Verdict, catalog, claim, compare, port};

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let first = arguments.next();
    match (first.as_deref(), arguments.next()) {
        (Some("compare"), Some(name)) if name == "all" => match compare_all() {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        },
        (Some("list"), None) => {
            for scenario in catalog::SCENARIOS {
                println!("{}", scenario.name);
            }
            ExitCode::SUCCESS
        }
        (Some(command @ ("run" | "compare")), Some(name)) => {
            let Some(scenario) = catalog::SCENARIOS
                .iter()
                .find(|scenario| scenario.name == name)
            else {
                eprintln!("unknown scenario: {name}");
                return ExitCode::FAILURE;
            };
            let vendor = claim(scenario);
            if command == "run" {
                for record in vendor {
                    println!("{record}");
                }
                return ExitCode::SUCCESS;
            }
            // A port panic is a vendor assertion the port adds; the panic
            // message goes to stderr and the empty trace fails the comparison.
            let port = catch_unwind(AssertUnwindSafe(|| port::run(scenario)))
                .unwrap_or_else(|_| Ok(Vec::new()));
            let verdict = compare(&vendor, port);
            println!("{verdict}");
            match verdict {
                Verdict::Match => ExitCode::SUCCESS,
                Verdict::Diff { .. } => ExitCode::from(1),
                Verdict::Incomplete(_) => ExitCode::from(2),
            }
        }
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: oer-tool-vendor-ieee802154-esp32s31-cli list | run <scenario> | compare <scenario> | compare all"
    );
    ExitCode::FAILURE
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
            return Err(format!("{name} did not compare MATCH ({status})").into());
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
