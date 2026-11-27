#![no_std]
use oer_protocols_ieee80211_contracts_port::{Queue, QueuePlace};
// A queue is reached only through its place's transitions: a holder has no
// way to the queue inside it.
pub fn out<'id>(place: QueuePlace<'id>) -> QueuePlace<'id> {
    #[cfg(ui_reject)]
    let _queue: Queue<'id> = place.state; // ui-error: E0616
    let _ = core::marker::PhantomData::<Queue<'id>>;
    place
}
