//! The command line of the typed vendor scenarios and the reviewer commands.

#![forbid(unsafe_code)]

fn main() -> std::process::ExitCode {
    oer_vendor_scenario::report::command::main(
        env!("CARGO_PKG_NAME"),
        oer_vendor_scenario::esp32s31::install,
        |scenario, reviewer| oer_vendor_scenario::esp32s31::run::run(scenario, reviewer),
    )
}
