#![forbid(unsafe_code)]
fn main() -> std::process::ExitCode {
    oer_lab_stand_fixture_install::launcher::main(
        oer_lab_stand_fixture_install::launcher::LaunchTarget::Bluetooth,
    )
}
