use oer_interrupt_table::{Bound, Context, Lines, TieBreak};
const LINES: Lines = Lines { first: 16, count: 48, levels: 8, harts: 1, tie_break: TieBreak::HighestLine };
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

use oer_esp32s31_bluetooth_driver::interrupt::{BluetoothInterruptEnds, BluetoothInterrupts, ChipGate, DoorbellLine};
use oer_interrupt_table::signal::host::Host;
pub struct Lower;
impl DoorbellLine for Lower { fn acknowledge(&mut self) {} }
pub fn on_bt(_: &mut BluetoothInterrupts<Host, Lower>) {}
oer_interrupt_table::interrupt_table! {
    lines: LINES, handler_attributes: [];
    contexts { radio: 3; }
    bt: BtToken = BluetoothInterrupts<Host, Lower> => on_bt, line 18, context radio, hart 0, Hardware;
}
pub fn attach(ends: BluetoothInterruptEnds<ChipGate, Host>, bound: &Bound<BtToken>) {
    let _ = ends.attach(bound, Bell); // ui-error@outside: E0080: evaluation panicked: a request handler must be above its executor on the same hart
}
