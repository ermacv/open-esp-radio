use super::*;
use crate::{
    owner::PhyInitializationAccess, power::test_clocks::CountingBackend, root::RadioHardware,
};
use core::{
    cell::Cell,
    future::Future,
    task::{Context, Poll, Waker},
};
use oer_time::{Clock, Duration, Timer};
use oer_time_virtual::VirtualClock;
use std::{
    boxed::Box,
    panic::{AssertUnwindSafe, catch_unwind},
    rc::Rc,
};

fn domain<T>(attachment: T) -> RadioDomain<CountingBackend, T> {
    RadioHardware::for_host_test()
        .split(attachment, PhyIdentity::for_host_test())
        .0
}

#[test]
fn exact_reunion_preserves_epoch_attachment_and_every_restore_obligation() {
    for occupy in [
        PhyRouteState::occupy_txdc_for_test,
        PhyRouteState::occupy_txiq_for_test,
        PhyRouteState::occupy_rx_dco_for_test,
        PhyRouteState::occupy_bluetooth_tx_power_control_for_test,
    ] {
        let mut transaction = domain(17_u8).detach(Operation::Maintain);
        let epoch = transaction.hardware_mut().begin_registration().epoch();
        occupy(&mut transaction.hardware_mut().restore);
        let (owner, phy) = transaction.split();
        let radio = owner.reunite(phy).unwrap_or_else(|_| panic!("exact owner"));
        assert_eq!(radio.registration_epoch(), Some(epoch));
        assert_eq!(*radio.attachment(), 17);
        let before = crate::root::check_phy_restore_complete(&radio.state.phy);
        assert!(before.is_err());
        let radio = radio.detach(Operation::Maintain).reunite();
        assert_eq!(radio.registration_epoch(), Some(epoch));
        assert_eq!(
            crate::root::check_phy_restore_complete(&radio.state.phy),
            before
        );
    }
}

#[test]
fn equal_registration_epochs_do_not_authorize_a_foreign_reunion() {
    let mut first = domain(13).detach(Operation::Maintain);
    let mut second = domain(29).detach(Operation::Maintain);
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
    assert_eq!(*first.attachment(), 13);
    assert_eq!(*second.attachment(), 29);
    assert_eq!(first.fault_handle().fault(), None);
    assert_eq!(second.fault_handle().fault(), None);
}

struct Drops(Rc<Cell<usize>>);
impl Drop for Drops {
    fn drop(&mut self) {
        self.0.set(self.0.get() + 1);
    }
}
fn report(fault: Fault) -> ! {
    std::panic::panic_any(fault)
}
fn cancellable<T>(
    radio: RadioDomain<CountingBackend, T>,
    clock: &VirtualClock,
) -> impl Future<Output = RadioDomain<CountingBackend, T>> {
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
        let drops = Rc::new(Cell::new(0));
        let (mut radio, _) = RadioHardware::for_host_test()
            .split::<CountingBackend, _>(Drops(drops.clone()), identity);
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
        assert_eq!(drops.get(), 0, "quarantined attachments are retained");
    }
}

#[test]
fn completed_virtual_time_operation_returns_owner_once_without_fault() {
    let clock: VirtualClock = VirtualClock::new();
    let radio = domain(41);
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
    assert_eq!(*radio.attachment(), 41);
    assert_eq!(fault.fault(), None);
}

#[test]
fn losing_the_movable_half_retains_the_original_fault_and_return_payload() {
    let seat = Box::leak(Box::new(TransactionSeat::for_host(report)));
    let drops = Rc::new(Cell::new(0));
    let (radio, _) = RadioHardware::for_host_test()
        .split::<CountingBackend, _>(Drops(drops.clone()), seat.claim().unwrap());
    let (owner, phy) = radio.detach(Operation::Maintain).split();
    let error = catch_unwind(AssertUnwindSafe(|| drop(phy))).unwrap_err();
    let fault = *error.downcast::<Fault>().unwrap();
    assert_eq!(fault.reason, FaultReason::Cancelled(Operation::Maintain));
    drop(owner);
    assert_eq!(seat.fault(), Some(fault));
    assert_eq!(drops.get(), 0);
}

#[test]
fn diagnostic_handle_reports_only_the_missing_owner_observation() {
    let seat = Box::leak(Box::new(TransactionSeat::for_host(report)));
    let (mut radio, _) =
        RadioHardware::for_host_test().split::<CountingBackend, _>((), seat.claim().unwrap());
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
    use crate::ieee80211::client::{WifiClocked, WifiCold};
    for operation in [Operation::Channel, Operation::Retune] {
        for polled in [false, true] {
            let seat = Box::leak(Box::new(TransactionSeat::for_host(report)));
            let (mut radio, partitions) = RadioHardware::for_host_test()
                .split::<CountingBackend, _>((), seat.claim().unwrap());
            let epoch = radio.begin_registration_for_host_test().epoch();
            let mut wifi = WifiClocked::for_host_test(WifiCold::from_partition(partitions.wifi));
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
