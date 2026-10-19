#![forbid(unsafe_code)]
fn main() -> std::process::ExitCode {
    oer_lab_fixtures::launcher::main(oer_lab_fixtures::launcher::LaunchTarget::Network)
}
