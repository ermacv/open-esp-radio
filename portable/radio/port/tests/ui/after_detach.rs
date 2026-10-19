#![no_std]
use oer_radio_port::{Detach, PortBackend};
pub fn detach<'id, B: PortBackend>(backend: &mut B, held: &B::Held<'id>, call: Detach) {
    backend.detach(call, held);
    #[cfg(ui_reject)]
    let _ = backend.detach(call, held); // ui-error: E0382
}
