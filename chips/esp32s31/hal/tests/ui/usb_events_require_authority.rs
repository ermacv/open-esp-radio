#![no_std]
use oer_esp32s31_hal::usb_serial_jtag::{ConsoleEndpoint, ConsoleInterrupt};
use oer_espressif_usb_serial_jtag::{Endpoint, EndpointRegisters};
use oer_interrupt_table::signal::{IrqSignal, SignalExecutor};
use oer_interrupt_table::{Bound, EventAuthority, Silent};

pub fn endpoint<E: SignalExecutor>(
    registers: ConsoleEndpoint,
    signal: &'static IrqSignal<E>,
    authority: &impl EventAuthority<ConsoleInterrupt>,
) {
    #[cfg(ui_reject)]
    let _endpoint = Endpoint::new(registers, signal); // ui-error: E0061
    #[cfg(not(ui_reject))]
    let _endpoint = Endpoint::new(registers, signal, authority);
}

pub fn events(
    registers: &mut ConsoleEndpoint,
    authority: &impl EventAuthority<ConsoleInterrupt>,
) {
    #[cfg(ui_reject)]
    registers.enable_events(); // ui-error: E0061
    #[cfg(not(ui_reject))]
    registers.enable_events(authority);
}

pub fn bound<E: SignalExecutor, U: oer_interrupt_table::Entry<Part = ConsoleInterrupt>>(
    registers: ConsoleEndpoint,
    signal: &'static IrqSignal<E>,
    authority: &Bound<U>,
) {
    endpoint(registers, signal, authority);
}

pub fn silent<E: SignalExecutor>(
    mut registers: ConsoleEndpoint,
    signal: &'static IrqSignal<E>,
    authority: &Silent<ConsoleInterrupt>,
) {
    events(&mut registers, authority);
    endpoint(registers, signal, authority);
}
