//! `cargo regdb <db.txt> <revision> <out.rs>`: writes the regulatory tables of
//! `oer-radio-regulatory` from a wireless-regdb `db.txt`.

#![forbid(unsafe_code)]

mod regdb;
mod render;

use std::process::ExitCode;

/// The countries whose rules go into the tables: the stand's and the owner's
/// requests.
const COUNTRIES: &[&str] = &["DE"];

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let [db, revision, out] = arguments.as_slice() else {
        eprintln!("usage: cargo regdb <db.txt> <revision> <out.rs>");
        return ExitCode::from(2);
    };
    match run(db, revision, out) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
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
