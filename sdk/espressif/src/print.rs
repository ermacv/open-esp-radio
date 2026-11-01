//! Text on the console before and beside the live console: the early
//! writer, which waits for room within its budget and counts what it drops.

/// Write `bytes` whole, or fail.
#[cfg(all(feature = "esp32s31", target_arch = "riscv32"))]
pub(crate) fn whole(bytes: &[u8]) -> Result<(), ()> {
    if oer_esp32s31_hal::peripherals::usb_serial_jtag::early_text(bytes).is_whole() {
        Ok(())
    } else {
        Err(())
    }
}
