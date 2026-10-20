#![no_std]
use oer_radio_port::{Disabled, Owner, PortBackend};
pub async fn consume<'p, 'id, B: PortBackend>(owner: Owner<'p, 'id, B, Disabled>) {
    let enabling = owner.enable();
    #[cfg(ui_reject)]
    let _ = owner.enable(); // ui-error: E0382
    drop(enabling.await);
}
