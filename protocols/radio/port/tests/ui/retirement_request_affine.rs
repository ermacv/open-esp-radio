#![no_std]
use oer_radio_port::{SlotIdentity, SlotOwner, SlotRetire, SlotStopped};
pub async fn stop<Id: SlotIdentity, S: SlotOwner<Id>>(request: SlotRetire<'_, Id, S>) -> SlotStopped<Id, S> {
    let first = request.requested();
    #[cfg(ui_reject)]
    let _ = request.requested(); // ui-error: E0382
    first.await
}
