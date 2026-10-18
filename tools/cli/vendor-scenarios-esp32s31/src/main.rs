//! The command line of this chip's typed vendor scenarios and the reviewer
//! commands.

fn main() -> std::process::ExitCode {
    oer_vendor_scenario_cli::main(
        env!("CARGO_PKG_NAME"),
        oer_esp32s31_vendor_scenarios::install,
        |scenario, reviewer, verdict| {
            oer_esp32s31_vendor_scenarios::run::run(scenario, reviewer, verdict)
        },
    )
}
