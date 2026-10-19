//! The command line of this chip's typed vendor scenarios and the reviewer
//! commands.

fn main() -> std::process::ExitCode {
    oer_vendor_scenario_command::main(
        env!("CARGO_PKG_NAME"),
        oer_vendor_scenarios_esp32c5::install,
        |scenario, reviewer, verdict| {
            oer_vendor_scenarios_esp32c5::run::run(scenario, reviewer, verdict)
        },
    )
}
