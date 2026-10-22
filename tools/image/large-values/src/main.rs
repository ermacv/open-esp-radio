//! A workspace compiler wrapper. Reporting never changes the compiler verdict.
#![feature(rustc_private)]
#![forbid(unsafe_code)]

extern crate rustc_public;

mod bridge;
mod report;

use std::{ffi::OsString, path::PathBuf, process::ExitCode};

fn main() -> ExitCode {
    let mut arguments = std::env::args_os();
    let executable = arguments.next().unwrap_or_default();
    let Some(rustc) = arguments.next() else {
        eprintln!("usage: oer-tool-image-large-values <rustc> <rustc arguments...>");
        return ExitCode::FAILURE;
    };
    let arguments: Vec<OsString> = arguments.collect();
    let configuration = report::Configuration::from_arguments(&arguments);
    // Cargo also invokes wrappers for dependencies, host build scripts, macros,
    // compiler probes and type checks: those run the original compiler.
    let Some(configuration) = configuration else {
        return match std::process::Command::new(rustc).args(arguments).status() {
            Ok(status) => ExitCode::from(status.code().unwrap_or(1) as u8),
            Err(error) => {
                eprintln!("run rustc: {error}");
                ExitCode::FAILURE
            }
        };
    };
    let arguments: Result<Vec<_>, _> = std::iter::once(executable)
        .chain(arguments)
        .map(OsString::into_string)
        .collect();
    let Ok(arguments) = arguments else {
        eprintln!("the pinned compiler driver requires UTF-8 arguments");
        return ExitCode::FAILURE;
    };
    let mut collected = None;
    let status = bridge::run(&arguments, |instances, source_span| {
        collected = Some(report::collect(instances, &configuration, source_span));
    });
    if status == ExitCode::SUCCESS
        && let Some(mut report) = collected
    {
        if let Err(error) = report.write(&configuration) {
            eprintln!("warning: large-values report: {error}");
        }
    }
    status
}

fn argument(arguments: &[OsString], name: &str) -> Option<String> {
    arguments.iter().enumerate().find_map(|(index, argument)| {
        let text = argument.to_str()?;
        text.strip_prefix(&format!("{name}="))
            .map(str::to_owned)
            .or_else(|| {
                (text == name)
                    .then(|| arguments.get(index + 1)?.to_str().map(str::to_owned))
                    .flatten()
            })
    })
}

fn output_path(arguments: &[OsString]) -> Option<PathBuf> {
    let directory = PathBuf::from(argument(arguments, "--out-dir")?);
    let name = argument(arguments, "--crate-name")?;
    let extra = codegen_argument(arguments, "extra-filename").unwrap_or_default();
    Some(directory.join(format!("{name}{extra}.large-values.json")))
}

fn codegen_argument(arguments: &[OsString], name: &str) -> Option<String> {
    arguments
        .iter()
        .filter_map(|argument| argument.to_str())
        .find_map(|text| {
            text.strip_prefix(&format!("{name}="))
                .or_else(|| text.strip_prefix(&format!("-C{name}=")))
                .map(str::to_owned)
        })
}
