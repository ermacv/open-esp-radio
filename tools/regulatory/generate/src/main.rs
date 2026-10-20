//! `cargo regdb <db.txt> <revision> <out.rs>`: writes the regulatory tables of
//! `oer-radio-regulatory` from a wireless-regdb `db.txt`.

#![forbid(unsafe_code)]

mod regdb;
mod render;

use clap::Parser as _;
use std::process::ExitCode;

#[derive(clap::Parser)]
#[command(name = "cargo regdb")]
struct Cli {
    #[arg(value_name = "db.txt")]
    db: String,
    revision: String,
    #[arg(value_name = "out.rs")]
    out: String,
}

/// The countries whose rules go into the tables: the stand's and the owner's
/// requests.
const COUNTRIES: &[&str] = &["DE"];

fn main() -> ExitCode {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(error) => {
            let _ = error.print();
            return ExitCode::from(2);
        }
    };
    match run(&cli.db, &cli.revision, &cli.out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use clap::CommandFactory as _;

    #[test]
    fn the_three_positional_arguments_keep_their_order() {
        Cli::command().debug_assert();
        let cli = Cli::try_parse_from(["regdb", "db.txt", "revision", "out.rs"]).unwrap();
        assert_eq!(
            (cli.db.as_str(), cli.revision.as_str(), cli.out.as_str()),
            ("db.txt", "revision", "out.rs")
        );
        for args in [
            vec!["regdb"],
            vec!["regdb", "db.txt", "revision"],
            vec!["regdb", "db.txt", "revision", "out.rs", "extra"],
        ] {
            assert_eq!(Cli::try_parse_from(args).err().unwrap().exit_code(), 2);
        }
    }
}

fn run(db: &str, revision: &str, out: &str) -> Result<(), String> {
    let text = std::fs::read_to_string(db).map_err(|error| format!("{db}: {error}"))?;
    let countries = regdb::parse(&text, COUNTRIES)?;
    let mut selected = Vec::new();
    for code in COUNTRIES {
        let rules = countries
            .get(*code)
            .ok_or_else(|| format!("country {code} is not in {db}"))?;
        selected.push((*code, rules.as_slice()));
    }
    let source = render::render(revision, &selected)?;
    std::fs::write(out, source).map_err(|error| format!("{out}: {error}"))?;
    let status = std::process::Command::new("rustfmt")
        .args(["--edition", "2024", out])
        .status()
        .map_err(|error| format!("rustfmt: {error}"))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("rustfmt failed on {out}: {status}"))
    }
}
