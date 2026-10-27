#![no_std]
use oer_esp32s31_bluetooth_driver::runtime_resources::ControllerRuntimeResources;
use oer_espressif_interrupt_table::signal::host::Deferred;

#[cfg(not(ui_reject))]
const _: usize = ControllerRuntimeResources::<1, Deferred>::CAPACITY;

#[cfg(ui_reject)]
const _: usize = ControllerRuntimeResources::<0, Deferred>::CAPACITY;
// ui-error@outside: E0080: evaluation panicked: a Controller runtime needs at least one modem timer slot
