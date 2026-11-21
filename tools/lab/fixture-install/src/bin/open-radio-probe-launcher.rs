#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]
fn main() -> std::process::ExitCode {
    oer_tools_lab_fixture::launcher::main(oer_tools_lab_fixture::launcher::LaunchTarget::Probe)
}
