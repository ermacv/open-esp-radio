#![no_std]
use oer_radio_port::{Disabled, Ended, Owner, PortBackend};

pub async fn cannot_forge<'p, 'id, B: PortBackend>(owner: Owner<'p, 'id, B, Disabled>) -> Ended<'id, B, ()> {
    #[cfg(ui_reject)]
    let _ = Owner::<B, Disabled>::new; // ui-error: E0624
    Ended::fail_stop(owner.fail_stop(), ())
}
