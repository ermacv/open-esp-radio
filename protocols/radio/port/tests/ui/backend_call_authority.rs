#![no_std]
use oer_radio_port::{Call, PortBackend};
pub async fn enable<'id, B: PortBackend>(
    call: Call,
    held: &B::Held<'id>,
    local: &mut B::Frontend<'id>,
) {
    #[cfg(ui_reject)]
    let _ = B::enable(held, local).await; // ui-error: E0061
    #[cfg(not(ui_reject))]
    let _ = B::enable(call, held, local).await;
}
