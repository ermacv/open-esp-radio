#![cfg_attr(target_arch = "riscv32", no_main, no_std)]
#![deny(clippy::undocumented_unsafe_blocks)]

#[cfg(target_arch = "riscv32")]
extern crate oer_firmware_probes_esp32s31_register as _;
#[cfg(target_arch = "riscv32")]
include!(concat!(env!("OUT_DIR"), "/probe_catalog.rs"));

#[allow(
    clippy::disallowed_methods,
    reason = "verification image idle entry is never a production runtime"
)]
// SAFETY: the probe image links no runtime, so this is the one `_start`;
// it touches nothing and never returns.
#[unsafe(no_mangle)]
#[cfg(target_arch = "riscv32")]
pub extern "C" fn _start() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(not(target_arch = "riscv32"))]
fn main() {}
