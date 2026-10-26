#![no_std]
use oer_esp32s31_hal::platform::SystimerAlarm0;
use oer_esp32s31_hal::usb_serial_jtag::{ConsoleEndpoint, ConsoleInterrupt};
use oer_espressif_usb_serial_jtag::{Endpoint, EndpointRegisters};
use oer_interrupt_table::signal::{IrqSignal, SignalExecutor};
use oer_interrupt_table::{Bound, Silent};

pub fn endpoint<E: SignalExecutor, U: oer_interrupt_table::Entry<Part = ConsoleInterrupt>, O: oer_interrupt_table::Entry<Part = SystimerAlarm0>>(
    registers: ConsoleEndpoint,
    signal: &'static IrqSignal<E>,
    usb: &Bound<U>,
    other: &Bound<O>,
) {
    #[cfg(ui_reject)]
    let _endpoint = Endpoint::new(registers, signal, other); // ui-error: E0271
    #[cfg(not(ui_reject))]
    let _endpoint = Endpoint::new(registers, signal, usb);
}

pub fn events(
    registers: &mut ConsoleEndpoint,
    usb: &Silent<ConsoleInterrupt>,
    other: &Silent<SystimerAlarm0>,
) {
    #[cfg(ui_reject)]
    registers.enable_events(other); // ui-error: E0277
    #[cfg(not(ui_reject))]
    registers.enable_events(usb);
}
