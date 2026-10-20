#![no_std]
use oer_radio_port::{Disabled, Ended, Owner, PortBackend};

pub fn cannot_forge<'p, 'id, B: PortBackend>(owner: Owner<'p, 'id, B, Disabled>) -> Ended<'id, B, ()> {
    #[cfg(ui_reject)]
    let _ = Owner::<B, Disabled>::new; // ui-error: E0624
    Ended::fail_stopped(owner.fail_stop(), ())
}
