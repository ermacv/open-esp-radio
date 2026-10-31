#![no_std]
use oer_radio_port::{Disabled, Ended, Owner, PortBackend};

pub async fn cannot_forge<'id, Id: oer_radio_port::SlotIdentity, B: PortBackend<Id>>(owner: Owner<'_, 'id, Id, B, Disabled>) -> Ended<Id, B, ()> {
    #[cfg(ui_reject)]
    { drop(owner); Ended::fail_stop((), ()) } // ui-error: E0308
    #[cfg(not(ui_reject))]
    Ended::fail_stop(owner.fail_stop(), ())
}
