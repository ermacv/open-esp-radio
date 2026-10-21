#![no_std]
use oer_radio_port::{Disabled, Owner, PortBackend, SlotIdentity};
pub fn stop<Id: SlotIdentity, B: PortBackend<Id>>(owner: Owner<'_, '_, Id, B, Disabled>) {
    let request = owner.fail_stop();
    #[cfg(ui_reject)]
    let _ = owner.enable(); // ui-error: E0382
    drop(request);
}
