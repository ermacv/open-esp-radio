#![no_std]
use oer_radio_port::{Disabled, Owner, PortBackend};
pub async fn consume<'p, 'id, Id: oer_radio_port::SlotIdentity, B: PortBackend<Id>>(owner: Owner<'p, 'id, Id, B, Disabled>) {
    let enabling = owner.enable();
    #[cfg(ui_reject)]
    let _ = owner.enable(); // ui-error: E0382
    drop(enabling.await);
}
