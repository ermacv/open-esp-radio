//! Privileged, finite probe generator. It never reconfigures the managed peer.
#![forbid(unsafe_code)]
#[cfg(any(target_os = "linux", test))]
use oer_hil_fixture::probe::frame;
#[cfg(target_os = "linux")]
mod linux;
use oer_hil_fixture::probe::model;

fn main() {
    #[cfg(target_os = "linux")]
    let _software = match oer_stand_fixture_install::launcher::enter(
        oer_stand_fixture_install::launcher::LaunchTarget::Probe,
    ) {
        Ok(lease) => lease,
        Err(error) => {
            eprintln!("probe software lease: {error}");
            std::process::exit(1);
        }
    };
    #[cfg(target_os = "linux")]
    let result = linux::run();
    #[cfg(not(target_os = "linux"))]
    let result: Result<(), Box<dyn std::error::Error + Send + Sync>> =
        Err("probe injection requires Linux".into());
    if let Err(error) = result {
        eprintln!("probe helper: {error}");
        std::process::exit(1);
    }
}
