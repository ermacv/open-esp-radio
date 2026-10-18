#![forbid(unsafe_code)]
fn main() -> std::process::ExitCode {
    oer_stand_fixture_install::launcher::main(
        oer_stand_fixture_install::launcher::LaunchTarget::Network,
    )
}
