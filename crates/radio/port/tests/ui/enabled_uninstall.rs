#![no_std]
#[cfg(not(ui_reject))]
use oer_radio_port::Disabled as State;
#[cfg(ui_reject)]
use oer_radio_port::Enabled as State;
use oer_radio_port::{Ended, Owner, PortBackend};
pub fn end<'p, 'id, B: PortBackend>(
    owner: Owner<'p, 'id, B, State>,
    control: B::Uninstall,
) -> Ended<'id, B, ()> {
    Ended::uninstall(owner, control, ()) // ui-error: E0277
}
