//! The command line of this chip's typed vendor scenarios and the reviewer
//! commands.

fn main() -> std::process::ExitCode {
    oer_tool_cli_vendor_scenario::main(
        env!("CARGO_PKG_NAME"),
        oer_tool_service_vendor_scenarios_esp32c5::install,
        |scenario, reviewer, verdict| {
            oer_tool_service_vendor_scenarios_esp32c5::run::run(scenario, reviewer, verdict)
        },
    )
}
