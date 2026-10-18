#![no_main]
#![no_std]

extern crate oer_esp32s31_probe_bluetooth as _;
include!(concat!(env!("OUT_DIR"), "/probe_catalog.rs"));

struct IsolatedVerifierCriticalSection;

critical_section::set_impl!(IsolatedVerifierCriticalSection);

// SAFETY: Blobray executes one entry synchronously in an isolated image. No
// interrupt handler, second hart, or concurrent caller exists in this modeled
// environment, so an empty restore token provides the critical-section
// contract needed by the production ownership coordinator.
unsafe impl critical_section::Impl for IsolatedVerifierCriticalSection {
    unsafe fn acquire() -> critical_section::RawRestoreState {}

    unsafe fn release(_: critical_section::RawRestoreState) {}
}

#[allow(
    clippy::disallowed_methods,
    reason = "verification image idle entry is never a production runtime"
)]
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {
        core::hint::spin_loop();
    }
}
