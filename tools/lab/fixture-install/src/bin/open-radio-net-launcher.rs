#![forbid(unsafe_code)]
fn main() -> std::process::ExitCode {
    oer_lab_fixture::launcher::main(oer_lab_fixture::launcher::LaunchTarget::Network)
}
