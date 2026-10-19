#![no_std]
use oer_radio_port::{Call, PortBackend};
pub async fn enable<'id, B: PortBackend>(
    backend: &mut B,
    call: Call,
    held: &B::Held<'id>,
    local: &mut B::Local<'id>,
) {
    #[cfg(ui_reject)]
    let _ = backend.enable(held, local).await; // ui-error: E0061
    #[cfg(not(ui_reject))]
    let _ = backend.enable(call, held, local).await;
}
