#![no_std]
use oer_radio_port::{Disabled, Owner, PortBackend};

pub fn setup_only<'id, B: PortBackend>(owner: &mut Owner<'_, 'id, B, Disabled>) {
    #[cfg(ui_reject)]
    let _ = &mut owner.backend; // ui-error: E0616
    #[cfg(not(ui_reject))]
    let _ = owner.setup();
}
