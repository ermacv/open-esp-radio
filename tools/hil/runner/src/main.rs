//! The HIL runner executable (`oer-tool-hil-runner`): the runner of
//! `oer-hil-run` with this executable's embedded host build record.
#![forbid(unsafe_code)]

/// This executable's host build record, embedded by its build script.
const RUNNER_BUILD: &str = include_str!(concat!(env!("OUT_DIR"), "/runner-build.json"));

fn main() {
    oer_hil_run::main(oer_hil_run::Build {
        record: RUNNER_BUILD,
        package: env!("CARGO_PKG_NAME"),
        version: env!("CARGO_PKG_VERSION"),
    });
}
