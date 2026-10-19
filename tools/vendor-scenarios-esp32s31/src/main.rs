//! The command line of this chip's typed vendor scenarios and the reviewer
//! commands.

fn main() -> std::process::ExitCode {
    oer_vendor_scenario_report::command::main(
        env!("CARGO_PKG_NAME"),
        oer_vendor_scenarios_esp32s31::install,
        |scenario, reviewer, verdict| {
            oer_vendor_scenarios_esp32s31::run::run(scenario, reviewer, verdict)
        },
    )
}
