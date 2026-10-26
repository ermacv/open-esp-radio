#![cfg_attr(target_arch = "riscv32", no_main)]
#![cfg_attr(target_arch = "riscv32", no_std)]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented
)]

//! The production bootstrap performs all cold bring-up. This entry only
//! acknowledges that it handed off; it starts no radio, interrupt or task.

#[cfg(target_arch = "riscv32")]
oer_espressif_staged_runtime::interrupt_table! {
    contexts {}
}

#[cfg(target_arch = "riscv32")]
oer_espressif_staged_runtime::entry!(main);

#[cfg(target_arch = "riscv32")]
fn main(_: oer_espressif_staged_runtime::Handoff) -> ! {
    // Each stage owns its own console state. Splitting the runtime root installs
    // its early writer without repeating cold boot or enabling interrupts.
    let _runtime =
        oer_esp32s31_hal::root::Root::take().map(oer_esp32s31_hal::root::Root::into_runtime);
    oer_esp32s31_hal::usb_serial_jtag::early_text(b"OER-RESULT: pass psram-bootstrap\r\n");
    loop {
        oer_riscv::wait_for_interrupt();
    }
}

#[cfg(not(target_arch = "riscv32"))]
fn main() {}
