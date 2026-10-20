use oer_esp32s31_hal::{
    bluetooth::{ClockedOwner, TaskOwner},
    domain::{BtbbError, RadioClient, RadioDomain},
    owner::PhyInitializationAccess,
    root::{ProtocolPartitions, RadioHardware},
};
use oer_time_virtual::VirtualClock;

use super::*;
use crate::concurrent::ConcurrentAcquire;
use crate::{
    PhyConfig, PhyState, RegisteredPhyState,
    concurrent::ConcurrentPhy,
    domain::PhyDomain,
    state::client::{DEFAULT_PLL_TRACK_PERIOD_MICROS, PhyClientState},
};

/// A radio domain whose PHY is registered in the domain's current epoch, or,
/// when `stale`, in an epoch a later registration retired.
fn radio_domain(stale: bool) -> (RadioDomain<ConcurrentPhy>, ProtocolPartitions) {
    let (mut radio, partitions) = RadioHardware::for_host_test().split(ConcurrentPhy::new());
    {
        let domain = &mut radio;
        let grant = domain.phy_hal().begin_registration();
        let epoch = grant.epoch();
        if stale {
            let _current = domain.phy_hal().begin_registration().epoch();
        }
        *domain.attachment_mut() = ConcurrentPhy::registered_for_test(PhyDomain::new(
            RegisteredPhyState::from_wrapper_test_model(
                PhyState::new(PhyConfig::production()),
                grant,
            ),
            PhyClientState::for_registration(DEFAULT_PLL_TRACK_PERIOD_MICROS, epoch),
        ));
    }
    (radio, partitions)
}

fn task(partitions: ProtocolPartitions) -> TaskOwner {
    let (task, _interrupts) =
        ClockedOwner::for_host_test(partitions.bluetooth).separate_interrupt_owner();
    task
}

fn bluetooth_is_client(radio: &RadioDomain<ConcurrentPhy>) -> bool {
    radio
        .attachment()
        .client_snapshot()
        .is_some_and(|snapshot| snapshot.contains(RadioClient::Bluetooth))
}

#[test]
fn joining_an_unregistered_domain_changes_nothing() {
    let (mut radio, partitions) = RadioHardware::for_host_test().split(ConcurrentPhy::new());
    let task = task(partitions);
    let radio = &mut radio;

    let _ = task;
    // Nothing prepared the domain, so no client can enter it.
    assert!(matches!(
        crate::concurrent::phy_ready(radio).map(|_| ()),
        Err(ConcurrentPhyError::NotRegistered)
    ));
    assert!(!radio.holds_btbb(RadioClient::Bluetooth));
}

#[test]
fn a_stale_registration_is_rejected_before_the_client_set_changes() {
    let (mut radio, partitions) = radio_domain(true);
    let task = task(partitions);
    let domain = &mut radio;

    let _ = &task;
    // The ready proof refuses a registration that no longer describes the
    // domain, before any client or BTBB change.
    assert_eq!(
        crate::concurrent::phy_ready(domain).map(|_| ()),
        Err(ConcurrentPhyError::EpochMismatch)
    );
    assert!(!bluetooth_is_client(&domain));
    assert!(!domain.holds_btbb(RadioClient::Bluetooth));
}

#[test]
fn leaving_without_the_btbb_reference_keeps_the_membership_and_the_client() {
    let (mut radio, partitions) = radio_domain(false);
    let task = task(partitions);
    let domain = &mut radio;
    // The first acquisition is due at once; its tracking completes.
    assert_eq!(
        enter_client(domain, RadioClient::Bluetooth, &VirtualClock::<8>::new()),
        Ok(ConcurrentAcquire::TrackingDue)
    );
    assert!(domain.attachment_mut().complete_tracking_for_test());

    let failure = match leave_bluetooth(domain, &task, BluetoothPhyMembership::for_test()) {
        Ok(_) => panic!("a membership without its BTBB reference cannot leave"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        BluetoothPhyClientError::Btbb(BtbbError::NotAcquired)
    );
    let _membership = failure.into_membership();
    assert!(bluetooth_is_client(&domain));
}

/// Enter `client` with a proof for the domain's registration.
fn enter_client(
    radio: &mut oer_esp32s31_hal::domain::RadioDomain<crate::concurrent::ConcurrentPhy>,
    client: oer_esp32s31_hal::domain::RadioClient,
    clock: &impl oer_time::Clock,
) -> Result<crate::concurrent::ConcurrentAcquire, crate::concurrent::ConcurrentPhyError> {
    crate::concurrent::acquire_client(radio, client, clock)
}

/// Join with a proof for the domain's registration.
fn join_bluetooth_prepared(
    radio: &mut oer_esp32s31_hal::domain::RadioDomain<crate::concurrent::ConcurrentPhy>,
    task: &TaskOwner,
    clock: &impl oer_time::Clock,
) -> Result<(BluetoothPhyMembership, ConcurrentAcquire), BluetoothPhyClientError> {
    let ready = crate::concurrent::PhyClientReady::<BluetoothPhyMembership>::for_test(radio);
    join_bluetooth(ready, task, clock)
        .map(|joined| (joined.membership, joined.acquired))
        .map_err(|refused| refused.error)
}

#[test]
fn btbb_is_taken_only_from_a_settled_registered_state() {
    let (mut radio, partitions) = radio_domain(false);
    let task = task(partitions);
    let domain = &mut radio;
    // Wi-Fi's entry makes the first tracking due: a tracking transition now
    // holds the PHY state, and the domain is not settled.
    assert_eq!(
        enter_client(domain, RadioClient::Wifi, &VirtualClock::<8>::new()),
        Ok(ConcurrentAcquire::TrackingDue)
    );
    assert!(domain.attachment().tracking_pending());
    // Even with a forged ready proof the join publishes no gain and takes no
    // BTBB.
    assert!(matches!(
        join_bluetooth_prepared(domain, &task, &VirtualClock::<8>::new()),
        Err(BluetoothPhyClientError::Phy(
            ConcurrentPhyError::TrackingPending
        ))
    ));
    assert!(!domain.holds_btbb(RadioClient::Bluetooth));
    // The settled acquisition writes BTBB registers, which a host test does
    // not reach; `oer-esp32s31-hal` tests the published gain's admission.
}

#[test]
fn first_due_join_keeps_membership_and_btbb_through_maintenance() {
    let (mut radio, partitions) = radio_domain(false);
    let task = task(partitions);
    // Seed the other baseband reference; the PHY still has no active client.
    radio.hold_btbb_for_test(RadioClient::Ieee802154);
    let clock = VirtualClock::<8>::new();
    let ready = crate::concurrent::phy_ready(&mut radio).unwrap();
    let crate::concurrent::PhyClientAccess::Available(ready) =
        ready.client::<BluetoothPhyMembership>()
    else {
        panic!("first client is available");
    };
    let joined = join_bluetooth(ready, &task, &clock)
        .unwrap_or_else(|error| panic!("first due join failed: {error:?}"));
    assert_eq!(joined.acquired, ConcurrentAcquire::TrackingDue);
    let radio = joined.radio;
    assert!(radio.attachment().tracking_pending());
    assert!(radio.holds_btbb(RadioClient::Bluetooth));
    let failure =
        leave_bluetooth(radio, &task, joined.membership).expect_err("maintenance is still pending");
    assert_eq!(
        failure.error(),
        BluetoothPhyClientError::Phy(ConcurrentPhyError::TrackingPending)
    );
    assert!(radio.holds_btbb(RadioClient::Bluetooth));
    assert!(radio.attachment_mut().complete_tracking_for_test());
    assert!(leave_bluetooth(radio, &task, failure.into_membership()).unwrap());
    assert!(!radio.holds_btbb(RadioClient::Bluetooth));
}

#[test]
fn settled_join_and_rejoin_after_rf_restart_keep_tracking_history() {
    use crate::concurrent::Slot;
    use oer_time::Duration;
    let (mut radio, partitions) = radio_domain(false);
    let task = task(partitions);
    radio.hold_btbb_for_test(RadioClient::Ieee802154);
    let clock = VirtualClock::<8>::new();
    let (membership, acquired) = join_bluetooth_prepared(&mut radio, &task, &clock).unwrap();
    assert_eq!(acquired, ConcurrentAcquire::TrackingDue);
    assert!(radio.attachment_mut().complete_tracking_for_test());
    assert!(leave_bluetooth(&mut radio, &task, membership).unwrap());
    let (membership, acquired) = join_bluetooth_prepared(&mut radio, &task, &clock).unwrap();
    assert_eq!(acquired, ConcurrentAcquire::Settled);
    assert!(!radio.attachment().tracking_pending());
    assert!(radio.holds_btbb(RadioClient::Bluetooth));
    assert!(leave_bluetooth(&mut radio, &task, membership).unwrap());

    // Model RF close/wake with the retained registration and no active client.
    // Target RF transactions themselves are covered by the diagnostic smoke.
    let Slot::Registered(domain) = core::mem::take(radio.attachment_mut().slot_mut()) else {
        panic!("settled registration after last leave");
    };
    assert!(domain.client_snapshot().is_empty());
    *radio.attachment_mut().slot_mut() = Slot::RfClosed(domain);
    assert!(matches!(
        crate::concurrent::phy_ready(&mut radio),
        Err(ConcurrentPhyError::RfClosed)
    ));
    let Slot::RfClosed(domain) = core::mem::take(radio.attachment_mut().slot_mut()) else {
        panic!("RF-closed model");
    };
    *radio.attachment_mut().slot_mut() = Slot::Registered(domain);
    clock
        .advance(Duration::from_micros(DEFAULT_PLL_TRACK_PERIOD_MICROS + 1))
        .unwrap();
    let (membership, acquired) = join_bluetooth_prepared(&mut radio, &task, &clock).unwrap();
    assert_eq!(acquired, ConcurrentAcquire::TrackingDue);
    assert!(radio.holds_btbb(RadioClient::Bluetooth));
    assert!(radio.attachment_mut().complete_tracking_for_test());
    assert!(leave_bluetooth(&mut radio, &task, membership).unwrap());
}

#[test]
fn rejected_clock_and_existing_btbb_return_the_unchanged_client() {
    let (mut radio, partitions) = radio_domain(false);
    let task = task(partitions);
    radio.hold_btbb_for_test(RadioClient::Ieee802154);
    let clock = VirtualClock::<8>::new();
    let (membership, _) = join_bluetooth_prepared(&mut radio, &task, &clock).unwrap();
    assert!(radio.attachment_mut().complete_tracking_for_test());
    assert!(leave_bluetooth(&mut radio, &task, membership).unwrap());
    let before = radio.attachment().client_snapshot();
    let past = VirtualClock::<8>::starting_at(oer_time::Instant::EPOCH);
    assert!(matches!(
        join_bluetooth_prepared(&mut radio, &task, &past),
        Err(BluetoothPhyClientError::Phy(ConcurrentPhyError::Acquire(_)))
    ));
    assert_eq!(radio.attachment().client_snapshot(), before);
    assert!(!radio.attachment().tracking_pending());
    assert!(!radio.holds_btbb(RadioClient::Bluetooth));
    assert!(radio.holds_btbb(RadioClient::Ieee802154));

    radio.hold_btbb_for_test(RadioClient::Bluetooth);
    let ready = crate::concurrent::phy_ready(&mut radio).unwrap();
    let crate::concurrent::PhyClientAccess::Available(ready) =
        ready.client::<BluetoothPhyMembership>()
    else {
        panic!("rejected join did not consume availability");
    };
    let refused = match join_bluetooth(ready, &task, &clock) {
        Err(refused) => refused,
        Ok(_) => panic!("existing BTBB ownership is not acquired twice"),
    };
    assert_eq!(
        refused.error,
        BluetoothPhyClientError::Btbb(BtbbError::AlreadyAcquired)
    );
    assert_eq!(refused.radio.attachment().client_snapshot(), before);
    assert!(refused.radio.holds_btbb(RadioClient::Bluetooth));
    task.release_btbb(refused.radio).unwrap();
    let (membership, acquired) = join_bluetooth_prepared(refused.radio, &task, &clock).unwrap();
    assert_eq!(acquired, ConcurrentAcquire::Settled);
    assert!(leave_bluetooth(refused.radio, &task, membership).unwrap());
}

#[test]
fn refused_baseband_admission_restores_the_prepared_first_client() {
    use crate::concurrent::{BtbbClientAcquireError, acquire_btbb_client};
    let (mut radio, partitions) = radio_domain(false);
    let task = task(partitions);
    radio.hold_btbb_for_test(RadioClient::Ieee802154);
    let clock = VirtualClock::<8>::new();
    let before = radio.attachment().client_snapshot();
    let result = acquire_btbb_client(
        &mut radio,
        RadioClient::Bluetooth,
        &clock,
        |radio, _gain| {
            assert!(!radio.holds_btbb(RadioClient::Bluetooth));
            Err(BtbbError::Stale)
        },
    );
    assert!(matches!(
        result,
        Err(BtbbClientAcquireError::Btbb(BtbbError::Stale))
    ));
    assert_eq!(radio.attachment().client_snapshot(), before);
    assert!(!radio.attachment().tracking_pending());
    assert!(!radio.holds_btbb(RadioClient::Bluetooth));
    assert!(radio.holds_btbb(RadioClient::Ieee802154));
    // The refused prepared transition left no claimed bit or refreshed time.
    let (membership, acquired) = join_bluetooth_prepared(&mut radio, &task, &clock).unwrap();
    assert_eq!(acquired, ConcurrentAcquire::TrackingDue);
    assert!(radio.attachment_mut().complete_tracking_for_test());
    assert!(leave_bluetooth(&mut radio, &task, membership).unwrap());
}
