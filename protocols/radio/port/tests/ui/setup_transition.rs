#![no_std]
use oer_radio_port::{Disabled, Owner, PortBackend};
pub async fn setup<'p, 'id, Id: oer_radio_port::SlotIdentity, B: PortBackend<Id>>(mut owner: Owner<'p, 'id, Id, B, Disabled>) {
    let setup = owner.setup();
    #[cfg(not(ui_reject))]
    drop(setup);
    let _ = owner.enable().await; // ui-error: E0505
    #[cfg(ui_reject)]
    drop(setup);
}
