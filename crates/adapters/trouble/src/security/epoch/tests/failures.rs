//! Failure/cancellation through the actual Host and the Controller core.

use super::*;
use crate::security::bonds::StoreError;
use core::cell::Cell;

struct Store<'a> {
    wait: bool,
    reads: &'a Cell<u32>,
    cancelled: &'a Cell<u32>,
}

impl BondStore for Store<'_> {
    type Error = u8;

    fn capacity(&self) -> usize {
        1
    }

    async fn load(&mut self, _: usize) -> Result<Option<BondInformation>, StoreError<u8>> {
        self.reads.set(self.reads.get() + 1);
        if !self.wait {
            return Err(StoreError::Backend(42));
        }
        struct Cancelled<'a>(&'a Cell<u32>);
        impl Drop for Cancelled<'_> {
            fn drop(&mut self) {
                self.0.set(self.0.get() + 1);
            }
        }
        let _cancelled = Cancelled(self.cancelled);
        pending().await
    }

    async fn insert(&mut self, _: BondInformation) -> Result<(), StoreError<u8>> {
        panic!("failed or cancelled restoration must not enroll")
    }

    async fn remove(&mut self, _: Identity) -> Result<bool, StoreError<u8>> {
        panic!("shutdown must not erase application bonds")
    }
}

#[test]
fn successful_reset_preserves_application_store_failure() {
    check_store_exit(false);
}

#[test]
fn immediate_stop_cancels_pending_store_read_before_reset_completes() {
    check_store_exit(true);
}

fn check_store_exit(stop: bool) {
    let mut transport = transport();
    let endpoints = transport.split();
    let mut peer = Peer::new(endpoints.controller);
    let mut resources = HostResources::<DefaultPacketPool, 1, 3>::new();
    let stack = trouble_host::new(
        ExternalController::<_, 1>::new(endpoints.host),
        &mut resources,
    )
    .build();
    let reads = Cell::new(0);
    let cancelled = Cell::new(0);
    let mut store = Store {
        wait: stop,
        reads: &reads,
        cancelled: &cancelled,
    };
    let comparison = NumericComparison::new();
    let exit = {
        let mut epoch = pin!(run(
            stack,
            &mut store,
            &comparison,
            async {
                if !stop {
                    pending::<()>().await;
                }
            },
            |_| panic!("no successful startup observation")
        ));
        let mut cx = Context::from_waker(Waker::noop());
        assert!(epoch.as_mut().poll(&mut cx).is_pending());
        assert_eq!(reads.get(), 1);
        assert_eq!(cancelled.get(), u32::from(stop));
        let reset = peer.hold_reset();
        // Ending the application does not synthesize a Reset response.
        assert!(epoch.as_mut().poll(&mut cx).is_pending());
        peer.answer(reset);
        let mut exit = None;
        for _ in 0..4 {
            if let Poll::Ready(value) = epoch.as_mut().poll(&mut cx) {
                exit = Some(value);
                break;
            }
        }
        exit.expect("real Reset completion")
    };
    assert!(exit.reset.is_ok());
    if stop {
        assert!(matches!(exit.cause, Cause::Requested));
        assert_eq!(exit.action(), ShutdownAction::Restart);
    } else {
        assert_eq!(exit.action(), ShutdownAction::Close);
        assert!(matches!(
            exit.cause,
            Cause::Application(gatt::RunError::Store(StoreError::Backend(42)))
        ));
    }
    assert!(comparison.pending().is_none());
    let stack = trouble_host::new(exit.controller, &mut resources).build();
    assert!(stack.with_bond_information(|keys| keys.is_empty()));
}

#[test]
fn cancelled_host_epoch_cannot_retire_an_unconsumed_reset_response() {
    let mut transport = transport();
    let endpoints = transport.split();
    let mut peer = Peer::new(endpoints.controller);
    let mut resources = HostResources::<DefaultPacketPool, 1, 3>::new();
    let stack = trouble_host::new(
        ExternalController::<_, 1>::new(endpoints.host),
        &mut resources,
    )
    .build();
    let mut bonds = RamBondStore::<1>::new();
    let comparison = NumericComparison::new();
    let reset = {
        let mut epoch = pin!(run(stack, &mut bonds, &comparison, async {}, |_| {}));
        assert!(
            epoch
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        peer.hold_reset()
        // Drop the consuming Host future while the peer still holds the Reset.
    };
    peer.answer(reset);
    let Err(error) = peer.transport.try_retire() else {
        panic!("dropped Host must not manufacture drained HCI ownership");
    };
    assert_eq!(error, HciRetirementError::ControllerPacketsPending);
    // Retirement stays refused until the response is consumed.
    assert_eq!(peer.transport.try_retire().err(), Some(error));
    assert!(comparison.pending().is_none());
}
