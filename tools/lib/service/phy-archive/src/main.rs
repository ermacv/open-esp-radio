//! `oer-check-phy-archive RLIB [--source CRATE]...`: the PHY archive
//! symbol audit; each `--source` names a crate the source-only
//! code may call into.
use std::process::ExitCode;

fn main() -> ExitCode {
    let mut arguments = std::env::args().skip(1);
    let Some(archive) = arguments.next() else {
        eprintln!("usage: oer-check-phy-archive RLIB [--source CRATE]...");
        return ExitCode::FAILURE;
    };
    let mut sources = Vec::new();
    while let Some(argument) = arguments.next() {
        match (argument.as_str(), arguments.next()) {
            ("--source", Some(source)) => sources.push(source.replace('-', "_")),
            _ => {
                eprintln!("usage: oer-check-phy-archive RLIB [--source CRATE]...");
                return ExitCode::FAILURE;
            }
        }
    }
    let result = oer_process::Checkout::discover("oer-check-phy-archive").and_then(|ctx| {
        oer_check_phy_archive::audit_phy(&ctx, std::path::Path::new(&archive), &sources)
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("phy archive: {error}");
            ExitCode::FAILURE
        }
    }
}
