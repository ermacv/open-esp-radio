#![no_std]
use oer_radio_port::{Enabled, Owner, PortBackend};
pub async fn phase<'p, 'id, Id: oer_radio_port::SlotIdentity, B: PortBackend<Id>>(mut owner: Owner<'p, 'id, Id, B, Enabled>) {
    let phase = owner.phase();
    #[cfg(not(ui_reject))]
    drop(phase);
    let _ = owner.disable(None, &mut |_| core::ops::ControlFlow::Continue(())).await; // ui-error: E0505
    #[cfg(ui_reject)]
    drop(phase);
}
