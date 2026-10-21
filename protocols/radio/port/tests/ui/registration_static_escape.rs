#![no_std]
use oer_radio_port::{SlotCompletion, SlotIdentity, SlotOwner, SlotRegistration};
#[cfg(ui_reject)]
pub fn retain_for_p2_drop<'scope, Id: SlotIdentity + 'static, S: SlotOwner<Id> + 'static>(registration: SlotRegistration<'scope, Id, S>) {
    fn permanent<T: 'static>(_: T) {}
    permanent(registration.run()); // ui-error: E0521
}
#[cfg(not(ui_reject))]
pub async fn join_before_scope_ends<'scope, Id: SlotIdentity, S: SlotOwner<Id>>(registration: SlotRegistration<'scope, Id, S>) -> Option<SlotCompletion<'scope, Id>> {
    registration.run().await
}
