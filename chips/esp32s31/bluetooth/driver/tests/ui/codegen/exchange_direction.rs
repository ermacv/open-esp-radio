use oer_espressif_interrupt_table::{Context};
#[derive(Clone)]
pub struct Bell;
impl oer_esp32s31_bluetooth_driver::interrupt::RadioDoorbell for Bell {
    const HANDLER: Context = Context { level: 3, hart: 0 };
    #[cfg(ui_reject)]
    const EXECUTOR: Context = Context { level: 3, hart: 0 };
    #[cfg(not(ui_reject))]
    const EXECUTOR: Context = Context { level: 2, hart: 0 };
    fn ring(&self) {}
}

use core::{future::Future, task::Context as TaskContext};
use oer_esp32s31_bluetooth_driver::interrupt::{BluetoothInterruptGate, ChipGate};
use oer_espressif_interrupt_table::signal::host::Host;
pub fn exchange(gate: &mut BluetoothInterruptGate<ChipGate, Host, Bell>, cx: &mut TaskContext<'_>) {
    let _ = core::pin::pin!(gate.stop_timer()).as_mut().poll(cx); // ui-error@outside: E0080: evaluation panicked: a request handler must be above its executor on the same hart
}
