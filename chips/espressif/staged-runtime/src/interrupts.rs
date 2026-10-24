//! The image's interrupt table: [`interrupt_table!`](crate::interrupt_table)
//! declares it; the hart's [`Router`](oer_interrupt_table::Router) binds its
//! entries, and the runtime's vectors call each entry's handler directly.

/// The CPU lines of the staged chips' CLIC: peripheral sources route to
/// lines 16 to 47 (lines 0 to 15 are the core's own), three level bits hold
/// eight levels, and the image's harts are one, or two with `two-harts`.
///
/// SOURCE: ESP-IDF 4d59230d `soc/esp32s31/include/soc/clic_reg.h` and
/// `soc/esp32c5/include/soc/clic_reg.h` (48 lines, `CLIC_EXT_INTR_NUM_OFFSET`
/// 16) and `riscv/include/riscv/csr_clic.h` (`NLBITS` 3).
pub const LINES: oer_interrupt_table::Lines = oer_interrupt_table::Lines {
    first: 16,
    count: 48,
    levels: 8,
    harts: if cfg!(feature = "two-harts") { 2 } else { 1 },
};

/// Declare the image's interrupt table, once per image, and hand its
/// `INTERRUPT_TABLE` to [`Handoff::adopt`](crate::Handoff::adopt):
///
/// ```ignore
/// oer_espressif_staged_runtime::interrupt_table! {
///     /// The IEEE 802.15.4 MAC's events.
///     ieee802154: Ieee802154Token = Ieee802154MacIrq => crate::radio::on_event,
///         line 20, level 3, hart 0, Hardware;
/// }
/// ```
///
/// Each entry names the token field, the token type, the interrupt part its
/// handler holds (its type names the source:
/// [`oer_mmio::InterruptSource`]), the handler (`fn(&mut Part)`), the CPU
/// line (16 to 47), the level (1 to 8), the hart and the handler's class
/// (`oer_interrupt_table::Class`: `Hardware`, `Executor` or
/// `Exempt(VIOLATION, "reason")`), which the image gate checks. The macro
/// defines the line's handler symbol, which the line's vector calls, the
/// token type, and `Interrupts::take` that hands out each token once.
#[macro_export]
macro_rules! interrupt_table {
    ($($entries:tt)*) => {
        $crate::__interrupt_table::interrupt_table! {
            lines: $crate::interrupts::LINES,
            handler_attributes: [];
            $($entries)*
        }
    };
}
