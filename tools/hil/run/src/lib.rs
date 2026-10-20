//! The HIL runner: its command line, the run of a scenario set on the
//! stand and the image, flash and executor work it does; the thin
//! executable is `tools/hil/runner`.
#![forbid(unsafe_code)]

pub mod flash;
pub mod image;

mod board;
pub mod cli;
mod command;
mod execution;
mod fixture;
mod scenario;
#[cfg(test)]
mod tests;

pub(crate) use oer_hil_scenario::workload::Result;
pub(crate) use oer_hil_scenario::workload::emit_json;

/// The runner's host build record, as its executable embedded it.
pub struct Build {
    pub record: &'static str,
    pub package: &'static str,
    pub version: &'static str,
}

/// Run the runner with the command line of this process; `build` is the
/// executable's embedded build record.
pub fn main(build: Build) {
    let arguments = std::env::args_os().collect::<Vec<_>>();
    let cli = match cli::Invocation::try_parse_from(&arguments).unwrap_or_else(|error| error.exit())
    {
        cli::Invocation::ObserverBuild => {
            println!("{}", build.record);
            return;
        }
        cli::Invocation::CommandTree => {
            let tree = oer_os_command_tree::command_tree(&cli::command(), &[]);
            println!("{}", serde_json::to_string(&tree).unwrap_or_default());
            return;
        }
        cli::Invocation::Run(cli) => *cli,
    };
    let registered = oer_hil_runs::run::register_runner(oer_hil_runs::run::RunnerBuild {
        record: build.record,
        package: build.package,
        version: build.version,
    });
    if let Err(error) = registered.and_then(|()| command::run(cli, arguments)) {
        eprintln!("error: {error}");
        std::process::exit(if oer_os_process::is_cancelled(&*error) {
            130
        } else {
            1
        });
    }
}
