#![no_std]
use oer_radio_port::{Call, PortBackend, SlotIdentity};
pub async fn enable<'p, Id: SlotIdentity, B: PortBackend<Id>>(call: Call, view: &mut B::View<'p>) {
    #[cfg(ui_reject)]
    let _ = B::enable(view).await; // ui-error: E0061
    #[cfg(not(ui_reject))]
    let _ = B::enable(call, view).await;
}
