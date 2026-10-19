#![no_main]
#![no_std]
#![deny(clippy::undocumented_unsafe_blocks)]

extern crate oer_firmware_probes_esp32s31_radio as _;
include!(concat!(env!("OUT_DIR"), "/probe_catalog.rs"));

#[allow(
    clippy::disallowed_methods,
    reason = "verification image idle entry is never a production runtime"
)]
// SAFETY: the probe image links no runtime, so this is the one `_start`;
// it touches nothing and never returns.
#[unsafe(no_mangle)]
pub extern "C" fn _start() -> ! {
    loop {
        core::hint::spin_loop();
    }
}
