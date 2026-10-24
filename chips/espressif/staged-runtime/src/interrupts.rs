//! The image's interrupt table: [`interrupt_table!`](crate::interrupt_table)
//! declares it; `oer_espressif_esp_hal_irq` drives the chip's
//! matrix by it, and its owners enable and disable their sources there.

/// Declare the image's interrupt table, once per image, and hand its
/// `INTERRUPT_TABLE` to [`Handoff::adopt`](crate::Handoff::adopt):
///
/// ```ignore
/// oer_espressif_staged_runtime::interrupt_table! {
///     /// The Embassy time driver's alarm.
///     timer: TimerToken = TG0_T0_LEVEL => crate::time::on_alarm, Priority1, ProCpu, Hardware;
/// }
/// ```
///
/// Each entry names the token field, the token type, the `Interrupt` source,
/// the handler (`fn()`), the `Priority`, the `Cpu` and the handler's class
/// (`oer_interrupt_table::Class`: `Hardware`, `Executor` or
/// `Exempt("reason")`), which the image gate checks. The macro defines the
/// source's handler symbol in SRAM, which the source's vector slot holds from
/// the link on, the token type, and `Interrupts::take` that hands out each
/// token once. The image needs `esp-hal` as a dependency.
#[macro_export]
macro_rules! interrupt_table {
    ($($entries:tt)*) => {
        $crate::__interrupt_table::interrupt_table! {
            source: esp_hal::peripherals::Interrupt = esp_hal::peripherals::Interrupt,
            level: esp_hal::interrupt::Priority = esp_hal::interrupt::Priority,
            core: esp_hal::system::Cpu = esp_hal::system::Cpu,
            handler_attributes: [];
            $($entries)*
        }
    };
}
