#![no_std]
use oer_radio_port::{SlotIdentity, SlotOwner, SlotRetire, SlotStopped};
pub async fn stop<Id: SlotIdentity, S: SlotOwner<Id>>(request: SlotRetire<'_, Id, S>) -> SlotStopped<Id, S> {
    #[cfg(ui_reject)]
    return SlotStopped { identity: core::marker::PhantomData, owner: core::marker::PhantomData }; // ui-error: E0451
    #[cfg(not(ui_reject))]
    request.requested().await
}
