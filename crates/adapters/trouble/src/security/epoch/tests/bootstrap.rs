//! A bootstrap Reset must not be mistaken for a later shutdown Reset.

use super::*;
use embassy_sync::signal::Signal;

#[test]
fn stop_during_bootstrap_does_not_accept_the_old_reset_completion() {
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
    let stop = Signal::<NoopRawMutex, ()>::new();
    let mut epoch = pin!(run(stack, &mut bonds, &comparison, stop.wait(), |_| {}));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(epoch.as_mut().poll(&mut cx).is_pending());
    let bootstrap_reset = peer.hold_reset();
    stop.signal(());
    for _ in 0..4 {
        assert!(epoch.as_mut().poll(&mut cx).is_pending());
    }
    peer.answer(bootstrap_reset);
    for _ in 0..4 {
        assert!(
            epoch.as_mut().poll(&mut cx).is_pending(),
            "bootstrap Reset completion is not the shutdown Reset completion"
        );
    }
    // The runner may have submitted a subsequent bootstrap command before the
    // stop handoff sees acknowledgement. Answer it through the real core.
    // Its response must also never complete the final Reset.
    let mut commands = 0;
    let shutdown = loop {
        commands += 1;
        assert!(
            commands <= 16,
            "shutdown exceeded the finite bootstrap drain"
        );
        let held = peer
            .hold()
            .expect("shutdown must be queued after the bootstrap boundary");
        if held.opcode == Reset::OPCODE {
            break held;
        }
        peer.answer(held);
        for _ in 0..4 {
            assert!(epoch.as_mut().poll(&mut cx).is_pending());
        }
    };
    assert!(epoch.as_mut().poll(&mut cx).is_pending());
    peer.answer(shutdown);
    let mut exit = None;
    for _ in 0..4 {
        if let Poll::Ready(value) = epoch.as_mut().poll(&mut cx) {
            exit = Some(value);
            break;
        }
    }
    let exit = exit.expect("only the second Reset authorizes shutdown");
    assert_eq!(exit.action(), ShutdownAction::Restart);
    assert!(matches!(exit.cause, Cause::Requested));
    assert!(exit.reset.is_ok());
    assert!(peer.transport.try_retire().is_ok());
}

#[test]
fn bootstrap_transport_failure_does_not_issue_a_second_reset() {
    transport_failure(FailureTiming::HostExit);
}

#[test]
fn transport_failure_while_draining_keeps_request_and_secondary_host_failure() {
    transport_failure(FailureTiming::StopRequest);
}

#[test]
fn store_failure_during_bootstrap_survives_secondary_transport_failure() {
    transport_failure(FailureTiming::StoreFailure);
}

enum FailureTiming {
    HostExit,
    StopRequest,
    StoreFailure,
}

struct RestoreFailure<'a>(&'a Signal<NoopRawMutex, ()>);

impl BondStore for RestoreFailure<'_> {
    type Error = u8;
    fn capacity(&self) -> usize {
        1
    }
    async fn load(
        &mut self,
        _: usize,
    ) -> Result<Option<BondInformation>, crate::security::bonds::StoreError<u8>> {
        self.0.wait().await;
        Err(crate::security::bonds::StoreError::Backend(42))
    }
    async fn insert(
        &mut self,
        _: BondInformation,
    ) -> Result<(), crate::security::bonds::StoreError<u8>> {
        panic!("no enrollment during failed restoration")
    }
    async fn remove(
        &mut self,
        _: Identity,
    ) -> Result<bool, crate::security::bonds::StoreError<u8>> {
        panic!("failure must not remove bonds")
    }
}

fn transport_failure(timing: FailureTiming) {
    let mut transport = transport();
    let endpoints = transport.split();
    let mut peer = Peer::new(endpoints.controller);
    let mut resources = HostResources::<DefaultPacketPool, 1, 3>::new();
    let stack = trouble_host::new(
        ExternalController::<_, 1>::new(endpoints.host),
        &mut resources,
    )
    .build();
    let store_failure = Signal::new();
    let mut bonds = RestoreFailure(&store_failure);
    let comparison = NumericComparison::new();
    let stop = Signal::<NoopRawMutex, ()>::new();
    let mut epoch = pin!(run(stack, &mut bonds, &comparison, stop.wait(), |_| {}));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(epoch.as_mut().poll(&mut cx).is_pending());
    let _unacknowledged = peer.hold_reset();
    match timing {
        FailureTiming::StopRequest => stop.signal(()),
        FailureTiming::StoreFailure => store_failure.signal(()),
        FailureTiming::HostExit => {}
    }
    assert!(epoch.as_mut().poll(&mut cx).is_pending());
    peer.transport.close();
    let Poll::Ready(exit) = epoch.as_mut().poll(&mut cx) else {
        panic!("transport closure must not hang reset preparation");
    };
    assert_eq!(exit.action(), ShutdownAction::Retain);
    match timing {
        FailureTiming::HostExit => {
            assert!(matches!(exit.cause, Cause::Host(Err(_))));
            assert!(matches!(
                exit.reset,
                Err(ResetError::BootstrapUnacknowledged)
            ));
        }
        FailureTiming::StopRequest | FailureTiming::StoreFailure => {
            match timing {
                FailureTiming::StopRequest => assert!(matches!(exit.cause, Cause::Requested)),
                FailureTiming::StoreFailure => assert!(matches!(
                    exit.cause,
                    Cause::Application(gatt::RunError::Store(
                        crate::security::bonds::StoreError::Backend(42)
                    ))
                )),
                FailureTiming::HostExit => unreachable!(),
            }
            assert!(matches!(
                exit.reset,
                Err(ResetError::HostDuringBootstrapDrain(Err(_)))
            ));
        }
    }
    // The held Reset never ran.
    assert_eq!(peer.core.bootstrap().phase(), BootstrapPhase::AwaitingReset);
}

#[test]
fn bootstrap_entropy_rejection_resets_then_closes_without_restart() {
    let mut transport = transport();
    let endpoints = transport.split();
    // Deliberately omit the entropy service. Real LE Rand returns Unknown Command.
    let mut peer = Peer::new(endpoints.controller);
    let mut resources = HostResources::<DefaultPacketPool, 1, 3>::new();
    let stack = trouble_host::new(
        ExternalController::<_, 1>::new(endpoints.host),
        &mut resources,
    )
    .build();
    let mut bonds = RamBondStore::<1>::new();
    let comparison = NumericComparison::new();
    let mut epoch = pin!(run(stack, &mut bonds, &comparison, pending(), |_| panic!(
        "no ready application"
    )));
    let mut cx = Context::from_waker(Waker::noop());
    let mut resets = 0;
    let mut exit = None;
    for _ in 0..32 {
        if let Poll::Ready(value) = epoch.as_mut().poll(&mut cx) {
            exit = Some(value);
            break;
        }
        if let Some(held) = peer.hold()
            && peer.answer(held) == Reset::OPCODE
        {
            resets += 1;
        }
    }
    let exit = exit.expect("finite rejected bootstrap and shutdown");
    assert_eq!(resets, 2);
    assert!(matches!(
        exit.cause,
        Cause::Host(Err(BleHostError::BleHost(trouble_host::Error::Hci(
            bt_hci::param::Error::UNKNOWN_CMD
        ))))
    ));
    assert!(exit.reset.is_ok());
    assert_eq!(exit.action(), ShutdownAction::Close);
    assert!(peer.transport.try_retire().is_ok());
}
