#![forbid(unsafe_code)]
fn main() -> std::process::ExitCode {
    oer_tool_lab_fixture_install::launcher::main(
        oer_tool_lab_fixture_install::launcher::LaunchTarget::Probe,
    )
}
