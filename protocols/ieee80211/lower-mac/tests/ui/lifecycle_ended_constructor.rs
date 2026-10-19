#![no_std]
use oer_radio_port::{Disabled, Ended, Owner, PortBackend};

pub fn cannot_forge<'id, B: PortBackend>(owner: Owner<'_, 'id, B, Disabled>) -> Ended<'id, B, ()> {
    #[cfg(ui_reject)]
    { drop(owner); Ended::fail_stopped((), ()) } // ui-error: E0308
    #[cfg(not(ui_reject))]
    Ended::fail_stopped(owner.fail_stop(), ())
}
