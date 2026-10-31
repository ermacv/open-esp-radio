//! The mask ROM functions the HAL calls: the two `ets_printf` output slot
//! installers, which the secondary-hart launch clears so the ROM's secondary
//! path prints through neither UART0 nor the runtime's USB Serial/JTAG
//! console. The facts (`chips/esp32s31/platform/rom-rev0.toml`) define both
//! symbols for the linker; their summaries bound them for the stack analysis.

#[cfg(target_arch = "riscv32")]
#[allow(
    unsafe_code,
    reason = "the ROM's printf slot installers are foreign functions"
)]
// SAFETY: both symbols are the pinned rev0 ROM's `ets_install_putc1` at
// 0x2f8055a6 and `ets_install_putc2` at 0x2f8055b0 (SOURCE: the ROM facts):
// each stores its one argument, a nullable C function pointer, to its ROM
// `.bss` slot and returns; neither touches the stack or other memory.
unsafe extern "C" {
    fn ets_install_putc1(putc: Option<extern "C" fn(u8)>);
    fn ets_install_putc2(putc: Option<extern "C" fn(u8)>);
}

/// Empty the ROM's `_putc1` and `_putc2` slots: an `ets_printf` then writes
/// nothing. The runtime never prints through the ROM.
#[cfg(target_arch = "riscv32")]
pub(crate) fn clear_printf_slots() {
    #[allow(
        unsafe_code,
        reason = "the ROM's printf slot installers are foreign functions"
    )]
    // SAFETY: the ROM functions above take a nullable function pointer; null
    // empties the slot. They write only their ROM-owned SRAM word, which no
    // runtime code reads, and the boot hart calls them before hart 1 runs.
    unsafe {
        ets_install_putc1(None);
        ets_install_putc2(None);
    }
}
