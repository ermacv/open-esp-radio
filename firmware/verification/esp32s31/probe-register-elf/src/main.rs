#![no_main]
#![no_std]

extern crate oer_esp32s31_probe_register as _;
include!(concat!(env!("OUT_DIR"), "/probe_catalog.rs"));

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
