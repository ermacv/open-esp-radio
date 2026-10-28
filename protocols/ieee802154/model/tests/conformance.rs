#![cfg(feature = "test-support")]

use core::{
    future::Future,
    pin::pin,
    task::{Context, Poll, Waker},
};
use oer_ieee802154_model::OwnedModel;
use oer_radio_port::{conformance::registration::guarded, with_local_slot};
use std::{sync::Arc, task::Wake};

// Exercise feature-enabled cleanup through the normally compiled library,
// rather than the unit-test build's cfg(test) implementation.
#[test]
fn cancellation_before_first_poll_stops_the_registered_model() {
    struct Noop;
    impl Wake for Noop {
        fn wake(self: Arc<Self>) {}
    }
    let waker = Waker::from(Arc::new(Noop));
    let future = with_local_slot(async |claim| {
        let mut slot = claim.place();
        let (backend, handle) = OwnedModel::new();
        let registered = guarded(slot.register(backend).ok().expect("fresh slot"));
        drop(registered);
        assert!(handle.abandoned());
        assert!(!handle.enabled());
        assert_eq!(handle.operation(), None);
        slot.end_model_world();
    });
    assert!(matches!(
        pin!(future).poll(&mut Context::from_waker(&waker)),
        Poll::Ready(())
    ));
}
