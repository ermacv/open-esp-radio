#![no_std]
#[cfg(not(ui_reject))]
use oer_radio_port::Disabled as State;
#[cfg(ui_reject)]
use oer_radio_port::Enabled as State;
use oer_radio_port::{Ended, Owner, PortBackend};
pub fn end<'p, 'id, Id: oer_radio_port::SlotIdentity, B: PortBackend<Id>>(
    owner: Owner<'p, 'id, Id, B, State>,
    control: B::Uninstall,
) -> Ended<Id, B, ()> {
    Ended::uninstall(owner, control, ()) // ui-error: E0277
}
