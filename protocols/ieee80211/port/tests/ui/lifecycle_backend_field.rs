#![no_std]
use oer_radio_port::{Disabled, Owner, PortBackend};

pub fn setup_only<'id, Id: oer_radio_port::SlotIdentity, B: PortBackend<Id>>(owner: &mut Owner<'_, 'id, Id, B, Disabled>) {
    #[cfg(ui_reject)]
    let _ = &mut owner.backend; // ui-error: E0609
    #[cfg(not(ui_reject))]
    let _ = owner.setup();
}
