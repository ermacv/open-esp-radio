#![cfg_attr(target_arch = "riscv32", no_main)]
#![cfg_attr(target_arch = "riscv32", no_std)]
#![forbid(unsafe_code)]

//! Link the real 802.15.4 buffers with the C5 runtime's placement proof.
//! This image claims storage only; it starts no radio operation.

#[cfg(target_arch = "riscv32")]
oer_espressif_staged_runtime::interrupt_table! {}

#[cfg(target_arch = "riscv32")]
oer_espressif_staged_runtime::entry!(main);

#[cfg(target_arch = "riscv32")]
fn main(_: oer_espressif_staged_runtime::Handoff) -> ! {
    let buffers = oer_esp32c5_hal::ieee802154::engine::claim_buffers()
        .expect("the image claims its 802.15.4 frame buffers once");
    core::hint::black_box(buffers);
    oer_espressif_staged_runtime::idle()
}

#[cfg(not(target_arch = "riscv32"))]
fn main() {}
