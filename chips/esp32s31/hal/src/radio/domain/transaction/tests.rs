use super::*;
use crate::{
    power::test_clocks::CountingBackend, system::owner::PhyInitializationAccess,
    system::root::RadioHardware,
};
use core::{
    future::Future,
    task::{Context, Poll, Waker},
};
use oer_portable_clock::{Clock, Timer};
use oer_portable_time::Duration;
use oer_portable_time_virtual::VirtualClock;
use std::{
    boxed::Box,
    panic::{AssertUnwindSafe, catch_unwind},
};

fn domain() -> RadioDomain<CountingBackend> {
    RadioHardware::for_host_test()
        .split(PhyIdentity::for_host_test())
        .expect("identity capacity")
        .0
}

#[test]
fn exact_reunion_preserves_epoch_and_every_restore_obligation() {
    for occupy in [
        PhyRouteState::occupy_txdc_for_test,
        PhyRouteState::occupy_txiq_for_test,
        PhyRouteState::occupy_rx_dco_for_test,
        PhyRouteState::occupy_bluetooth_tx_power_control_for_test,
    ] {
        let mut transaction = domain().detach(Operation::Maintain);
        let epoch = transaction.hardware_mut().begin_registration().epoch();
        occupy(&mut transaction.hardware_mut().restore);
        let (owner, phy) = transaction.split();
        let radio = owner.reunite(phy).unwrap_or_else(|_| panic!("exact owner"));
        assert_eq!(radio.registration_epoch(), Some(epoch));
        let before = crate::system::root::check_phy_restore_complete(&radio.state.phy);
        assert!(before.is_err());
        let radio = radio.detach(Operation::Maintain).reunite();
        assert_eq!(radio.registration_epoch(), Some(epoch));
        assert_eq!(
            crate::system::root::check_phy_restore_complete(&radio.state.phy),
            before
        );
    }
}

#[test]
fn equal_registration_epochs_do_not_authorize_a_foreign_reunion() {
    let mut first = domain().detach(Operation::Maintain);
    let mut second = domain().detach(Operation::Maintain);
    assert_eq!(
        first.hardware_mut().begin_registration().epoch(),
        second.hardware_mut().begin_registration().epoch()
    );
    let (first, first_phy) = first.split();
    let (second, second_phy) = second.split();
    let failure = first
        .reunite(second_phy)
        .err()
        .expect("foreign owner refused");
    assert_ne!(failure.expected_owner, failure.received_owner);
    let first = failure
        .domain
        .reunite(first_phy)
        .unwrap_or_else(|_| panic!("original first"));
    let second = second
        .reunite(failure.transaction)
        .unwrap_or_else(|_| panic!("original second"));
    assert_eq!(first.fault_handle().fault(), None);
    assert_eq!(second.fault_handle().fault(), None);
}

fn report(fault: Fault) -> ! {
    std::panic::panic_any(fault)
}
fn cancellable(
    radio: RadioDomain<CountingBackend>,
    clock: &VirtualClock,
) -> impl Future<Output = RadioDomain<CountingBackend>> {
    let transaction = radio.detach(Operation::Maintain);
    async move {
        clock
            .wait_until(clock.now().saturating_add(Duration::from_micros(5)))
            .await;
        transaction.reunite()
    }
}

#[test]
fn cancellation_before_poll_and_while_waiting_quarantines_every_owner() {
    for poll in [false, true] {
        let seat = Box::leak(Box::new(TransactionSeat::for_host(report)));
        let identity = seat.claim().expect("unique seat");
        assert!(seat.claim().is_none());
        let (mut radio, _) = RadioHardware::for_host_test()
            .split::<CountingBackend>(identity)
            .expect("identity capacity");
        let epoch = radio.begin_registration_for_host_test().epoch();
        let handle = radio.fault_handle();
        let clock: VirtualClock = VirtualClock::new();
        let mut future = Box::pin(cancellable(radio, &clock));
        if poll {
            assert!(
                future
                    .as_mut()
                    .poll(&mut Context::from_waker(Waker::noop()))
                    .is_pending()
            );
        }
        let error =
            catch_unwind(AssertUnwindSafe(|| drop(future))).expect_err("reporter never returns");
        let fault = *error.downcast::<Fault>().expect("named fault");
        assert_eq!(fault.reason, FaultReason::Cancelled(Operation::Maintain));
        assert_eq!(fault.registration, Some(epoch));
        assert_eq!(handle.fault(), Some(fault));
    }
}

#[test]
fn completed_virtual_time_operation_returns_owner_once_without_fault() {
    let clock: VirtualClock = VirtualClock::new();
    let radio = domain();
    let fault = radio.fault_handle();
    let mut future = Box::pin(cancellable(radio, &clock));
    let mut cx = Context::from_waker(Waker::noop());
    assert!(future.as_mut().poll(&mut cx).is_pending());
    clock
        .advance(Duration::from_micros(5))
        .expect("virtual deadline");
    let Poll::Ready(radio) = future.as_mut().poll(&mut cx) else {
        panic!("completed operation")
    };
    assert_eq!(radio.fault_handle().fault(), None);
    assert_eq!(fault.fault(), None);
}

#[test]
fn losing_the_movable_half_retains_the_original_fault_and_return_payload() {
    let seat = Box::leak(Box::new(TransactionSeat::for_host(report)));
    let (radio, _) = RadioHardware::for_host_test()
        .split::<CountingBackend>(seat.claim().unwrap())
        .expect("identity capacity");
    let (owner, phy) = radio.detach(Operation::Maintain).split();
    let error = catch_unwind(AssertUnwindSafe(|| drop(phy))).unwrap_err();
    let fault = *error.downcast::<Fault>().unwrap();
    assert_eq!(fault.reason, FaultReason::Cancelled(Operation::Maintain));
    drop(owner);
    assert_eq!(seat.fault(), Some(fault));
}

#[test]
fn diagnostic_handle_reports_only_the_missing_owner_observation() {
    let seat = Box::leak(Box::new(TransactionSeat::for_host(report)));
    let (mut radio, _) = RadioHardware::for_host_test()
        .split::<CountingBackend>(seat.claim().unwrap())
        .expect("identity capacity");
    let epoch = radio.begin_registration_for_host_test().epoch();
    let handle = radio.fault_handle();
    assert_eq!(handle.require_owner(Some(17)), 17);
    let error = catch_unwind(AssertUnwindSafe(|| handle.require_owner::<()>(None))).unwrap_err();
    let fault = *error.downcast::<Fault>().unwrap();
    assert_eq!(fault.reason, FaultReason::OwnerUnavailable);
    assert_eq!(fault.registration, Some(epoch));
}

#[test]
fn channel_custody_records_channel_or_retune_on_unpolled_and_pending_drop() {
    use crate::radio::ieee80211::client::{WifiClocked, WifiCold};
    for operation in [Operation::Channel, Operation::Retune] {
        for polled in [false, true] {
            let seat = Box::leak(Box::new(TransactionSeat::for_host(report)));
            let (mut radio, partitions) = RadioHardware::for_host_test()
                .split::<CountingBackend>(seat.claim().unwrap())
                .expect("identity capacity");
            let epoch = radio.begin_registration_for_host_test().epoch();
            let mut wifi = WifiClocked::for_host_test(WifiCold::from_partition(partitions.wifi).0);
            let mut platform = ();
            let channel = wifi.channel_transaction(&mut platform, radio.detach(operation));
            let clock: VirtualClock = VirtualClock::new();
            let mut future = Box::pin(async {
                clock
                    .wait_until(clock.now().saturating_add(Duration::from_micros(5)))
                    .await;
                channel.reunite()
            });
            if polled {
                assert!(
                    future
                        .as_mut()
                        .poll(&mut Context::from_waker(Waker::noop()))
                        .is_pending()
                );
            }
            let error = catch_unwind(AssertUnwindSafe(|| drop(future))).unwrap_err();
            let fault = *error.downcast::<Fault>().unwrap();
            assert_eq!(fault.reason, FaultReason::Cancelled(operation));
            assert_eq!(fault.registration, Some(epoch));
        }
    }
}

#[test]
fn exact_reunion_retains_power_and_the_registration() {
    let mut radio = domain();
    let grant = radio.begin_registration_for_host_test();
    radio.state.power.hold_for_test(RadioClient::Wifi);
    let coexisting = radio.enter_coex(CoexAbsent::<WifiClient>::issued());

    let (owner, phy) = radio.detach(Operation::Maintain).split();
    let mut radio = owner
        .reunite(phy)
        .unwrap_or_else(|_| panic!("the exact seat reunites"));
    assert!(radio.holds_common_power(RadioClient::Wifi));
    assert_eq!(radio.registration_epoch(), Some(grant.epoch()));
    let _absent = radio.leave_coex(coexisting);
    assert!(matches!(
        radio.into_parts(),
        Err((_, RadioDomainReleaseError::CommonPowerHeld))
    ));
}
