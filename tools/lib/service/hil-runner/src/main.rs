//! HIL host process entry point.
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

mod board;
mod cli;
mod command;
mod execution;
mod fixture;
mod scenario;
#[cfg(test)]
mod tests;

pub(crate) use oer_hil_workload::Result;
pub(crate) use oer_hil_workload::emit_json;

/// This executable's host build record, embedded by its build script.
const RUNNER_BUILD: &str = include_str!(concat!(env!("OUT_DIR"), "/runner-build.json"));

fn main() {
    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == "--observer-build")
    {
        println!("{RUNNER_BUILD}");
        return;
    }
    // The runner's commands for `cargo hil __command-tree`.
    if std::env::args_os()
        .nth(1)
        .is_some_and(|a| a == "__command-tree")
    {
        use clap::CommandFactory as _;
        let tree = oer_command_tree::command_tree(&cli::Cli::command(), &[]);
        println!("{}", serde_json::to_string(&tree).unwrap_or_default());
        return;
    }
    let registered =
        oer_hil_run_bundle::run::register_runner(oer_hil_run_bundle::run::RunnerBuild {
            record: RUNNER_BUILD,
            package: env!("CARGO_PKG_NAME"),
            version: env!("CARGO_PKG_VERSION"),
        });
    if let Err(error) = registered.and_then(|()| command::run()) {
        eprintln!("error: {error}");
        std::process::exit(if oer_process::is_cancelled(&*error) {
            130
        } else {
            1
        });
    }
}
