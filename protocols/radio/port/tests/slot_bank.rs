//! Every slot a bank consumes stays obtainable from it.
#![allow(
    clippy::disallowed_methods,
    clippy::disallowed_macros,
    reason = "test: a no-op waker and assertions"
)]

use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};
use oer_protocols_radio_port::{SlotBank, Slots, branded};

enum Capacity {}

#[test]
fn a_successful_put_preserves_each_slot_it_consumes() {
    let mut install = pin!(branded::<Slots<Capacity, 2>, ()>(async |mint| {
        let [first, second] = mint.into_pools();
        let mut bank = SlotBank::new([first]);
        // The public API accepts this second slot of exactly the same pool.
        bank.put(second);
        let mut recovered = Vec::new();
        while let Some(slot) = bank.take_first() {
            recovered.push(slot);
        }
        assert_eq!(
            recovered.len(),
            2,
            "both consumed slots must remain obtainable"
        );
    }));
    assert!(matches!(
        install
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(())
    ));
}

#[test]
fn an_empty_bank_does_not_consume_a_slot_without_a_return_path() {
    let mut install = pin!(branded::<Slots<Capacity, 1>, ()>(async |mint| {
        let [slot] = mint.into_pools();
        let mut bank = SlotBank::empty();
        bank.put(slot);
        assert!(
            bank.take_first().is_some(),
            "successful put must preserve its slot"
        );
    }));
    assert!(matches!(
        install
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(())
    ));
}
