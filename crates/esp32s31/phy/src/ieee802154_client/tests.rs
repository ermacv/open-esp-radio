use oer_esp32s31_hal::{
    ieee802154::Ieee802154Clocked,
    owner::PhyInitializationAccess,
    root::{ConcurrentPartitions, RadioHardware},
    shared_radio::{BtbbError, RadioClient, SharedRadio},
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

/// An arbiter whose domain is registered in the arbiter's current epoch, or,
/// when `stale`, in an epoch a later registration retired.
fn arbiter(stale: bool) -> (SharedRadio<ConcurrentPhy>, ConcurrentPartitions) {
    let (radio, partitions) = RadioHardware::for_host_test().into_concurrent(ConcurrentPhy::new());
    {
        let mut lease = radio
            .try_acquire()
            .unwrap_or_else(|_| panic!("a fresh arbiter grants its lease"));
        let epoch = lease.phy_hal().begin_registration_epoch();
        if stale {
            let _current = lease.phy_hal().begin_registration_epoch();
        }
        *lease.attachment_mut() = ConcurrentPhy::registered_for_test(PhyDomain::new(
            RegisteredPhyState::from_wrapper_test_model(PhyState::new(PhyConfig::production())),
            PhyClientState::for_registration(DEFAULT_PLL_TRACK_PERIOD_MICROS, epoch),
        ));
    }
    (radio, partitions)
}

fn ieee802154_is_client(lease: &SharedRadioLease<'_, ConcurrentPhy>) -> bool {
    lease
        .attachment()
        .client_snapshot()
        .is_some_and(|snapshot| snapshot.contains(RadioClient::Ieee802154))
}

#[test]
fn joining_an_unregistered_domain_changes_nothing() {
    let (radio, partitions) = RadioHardware::for_host_test().into_concurrent(ConcurrentPhy::new());
    let clocked = Ieee802154Clocked::for_host_test(partitions.ieee802154);
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));

    let _ = clocked;
    // Nothing prepared the domain, so no client can enter it.
    assert!(matches!(
        crate::concurrent::phy_ready(&mut lease).map(|_| ()),
        Err(ConcurrentPhyError::NotRegistered)
    ));
    assert!(!lease.holds_btbb(RadioClient::Ieee802154));
}

#[test]
fn a_stale_registration_is_rejected_before_the_client_set_changes() {
    let (radio, partitions) = arbiter(true);
    let clocked = Ieee802154Clocked::for_host_test(partitions.ieee802154);
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));

    let _ = &clocked;
    // The ready proof refuses a registration that no longer describes the
    // lease, before any client or BTBB change.
    assert_eq!(
        crate::concurrent::phy_ready(&mut lease).map(|_| ()),
        Err(ConcurrentPhyError::EpochMismatch)
    );
    assert!(!ieee802154_is_client(&lease));
    assert!(!lease.holds_btbb(RadioClient::Ieee802154));
}

#[test]
fn leaving_without_the_btbb_reference_keeps_the_membership_and_the_client() {
    let (radio, partitions) = arbiter(false);
    let clocked = Ieee802154Clocked::for_host_test(partitions.ieee802154);
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    assert_eq!(
        enter_client(
            &mut lease,
            RadioClient::Ieee802154,
            &VirtualClock::<8>::new()
        ),
        Ok(ConcurrentAcquire::Settled)
    );

    let failure = match leave_ieee802154(&mut lease, &clocked, Ieee802154PhyMembership::for_test())
    {
        Ok(_) => panic!("a membership without its BTBB reference cannot leave"),
        Err(failure) => failure,
    };
    assert_eq!(
        failure.error(),
        Ieee802154PhyClientError::Btbb(BtbbError::NotAcquired)
    );
    let _membership = failure.into_membership();
    assert!(ieee802154_is_client(&lease));
}

/// Enter `client` with a proof for the lease's registration.
fn enter_client(
    lease: &mut oer_esp32s31_hal::shared_radio::SharedRadioLease<
        '_,
        crate::concurrent::ConcurrentPhy,
    >,
    client: oer_esp32s31_hal::shared_radio::RadioClient,
    clock: &impl oer_time::Clock,
) -> Result<crate::concurrent::ConcurrentAcquire, crate::concurrent::ConcurrentPhyError> {
    crate::concurrent::acquire_client(lease, client, clock)
}

/// Join with a proof for the lease's registration.
fn join_ieee802154_prepared(
    lease: &mut oer_esp32s31_hal::shared_radio::SharedRadioLease<
        '_,
        crate::concurrent::ConcurrentPhy,
    >,
    clocked: &Ieee802154Clocked,
    clock: &impl oer_time::Clock,
) -> Result<(Ieee802154PhyMembership, ConcurrentAcquire), Ieee802154PhyClientError> {
    let ready = crate::concurrent::PhyReady::for_test(lease);
    join_ieee802154(ready, clocked, clock)
        .map(|joined| (joined.membership, joined.acquired))
        .map_err(|refused| refused.error)
}
