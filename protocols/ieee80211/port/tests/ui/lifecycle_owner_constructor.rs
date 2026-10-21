#![no_std]
use oer_radio_port::{Disabled, Ended, Owner, PortBackend};

pub async fn cannot_forge<'p, 'id, Id: oer_radio_port::SlotIdentity, B: PortBackend<Id>>(owner: Owner<'p, 'id, Id, B, Disabled>) -> Ended<Id, B, ()> {
    #[cfg(ui_reject)]
    let _ = Owner::<Id, B, Disabled>::new; // ui-error: E0624
    Ended::fail_stop(owner.fail_stop(), ())
}
