#![no_std]
use oer_radio_port::{LocalId, SlotOwner, with_local_slot};

pub async fn slots<S>()
where
    for<'scope> S: SlotOwner<LocalId<'scope>>,
{
    with_local_slot(async |claim| {
        let first = claim.place::<S>();
        #[cfg(ui_reject)]
        let second = claim.place::<S>(); // ui-error: E0382
        drop(first);
        #[cfg(ui_reject)]
        drop(second);
    })
    .await;
}
