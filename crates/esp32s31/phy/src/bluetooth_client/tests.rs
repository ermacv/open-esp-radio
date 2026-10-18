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
        let epoch = domain.phy_hal().begin_registration_epoch();
        if stale {
            let _current = domain.phy_hal().begin_registration_epoch();
        }
        *domain.attachment_mut() = ConcurrentPhy::registered_for_test(PhyDomain::new(
            RegisteredPhyState::from_wrapper_test_model(PhyState::new(PhyConfig::production())),
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
    let ready = crate::concurrent::PhyReady::for_test(radio);
    join_bluetooth(ready, task, clock)
        .map(|joined| (joined.membership, joined.acquired))
        .map_err(|refused| refused.error)
}
