use crate::{
    domain::{BtbbError, CommonRadioPowerError, ModemClockError, RadioClient, RadioDomain},
    phy::{registration::PhyRegistration, restore::PhyRouteState},
    root::{RadioHardware, RadioPartitions},
};

use super::{Ieee802154Clocked, Ieee802154Cold};

fn radio_domain() -> RadioDomain<crate::power::test_clocks::CountingBackend> {
    let RadioPartitions { shared, .. } = RadioPartitions::for_host_test();
    RadioDomain::new(
        shared,
        PhyRouteState::new(PhyRegistration::new()),
        (),
        crate::domain::transaction::PhyIdentity::for_host_test(),
    )
}

#[test]
fn the_cold_client_returns_its_partition_to_the_concurrent_split() {
    let (shared, mut partitions) = RadioHardware::for_host_test()
        .split::<crate::power::test_clocks::CountingBackend, _>(
        (),
        crate::domain::transaction::PhyIdentity::for_host_test(),
    );
    let cold = Ieee802154Cold::from_partition(partitions.ieee802154);
    partitions.ieee802154 = cold.into_partition();
    let _hardware = RadioHardware::reunite(shared, partitions)
        .unwrap_or_else(|_| panic!("an untouched split reunites"));
}

#[test]
fn a_rejected_power_entry_returns_the_unchanged_cold_client() {
    let mut radio = radio_domain();
    radio.hold_common_power_for_test(RadioClient::Ieee802154);
    let (_shared, partitions) = RadioHardware::for_host_test()
        .split::<crate::power::test_clocks::CountingBackend, _>(
        (),
        crate::domain::transaction::PhyIdentity::for_host_test(),
    );
    let domain = &mut radio;

    let failure = match Ieee802154Cold::from_partition(partitions.ieee802154).power_up(
        domain,
        &crate::power::test_clocks::CountingPlatformClocks::new(),
    ) {
        Ok(_) => panic!("a client cannot enter common power twice"),
        Err(failure) => failure,
    };
    assert_eq!(failure.error(), CommonRadioPowerError::AlreadyEntered);
    let _cold: Ieee802154Cold = failure.into_owner();
}

#[test]
fn the_btbb_steps_require_the_ieee802154_reference() {
    let mut radio = radio_domain();
    let (_shared, partitions) = RadioHardware::for_host_test()
        .split::<crate::power::test_clocks::CountingBackend, _>(
        (),
        crate::domain::transaction::PhyIdentity::for_host_test(),
    );
    let clocked = Ieee802154Clocked::for_host_test(partitions.ieee802154);
    let domain = &mut radio;

    assert_eq!(clocked.release_btbb(domain), Err(BtbbError::NotAcquired));
    let grant = domain.begin_registration_for_host_test();
    let replaced = domain.btbb_publisher().1.publish(&grant, 0x4f).unwrap();
    let _latest = domain.btbb_publisher().1.publish(&grant, 0x4f).unwrap();
    // The replaced gain is rejected before any register access.
    assert_eq!(
        clocked.acquire_btbb(domain, &replaced),
        Err(BtbbError::Stale)
    );
    assert!(!domain.holds_btbb(RadioClient::Ieee802154));
}

#[test]
fn releasing_clocks_the_planner_never_granted_keeps_the_clocked_owner() {
    let mut radio = radio_domain();
    let (_shared, partitions) = RadioHardware::for_host_test()
        .split::<crate::power::test_clocks::CountingBackend, _>(
        (),
        crate::domain::transaction::PhyIdentity::for_host_test(),
    );
    let clocked = Ieee802154Clocked::for_host_test(partitions.ieee802154);
    let domain = &mut radio;

    // Rejected by the planner before any register access.
    let failure = match clocked.disable_clocks(domain) {
        Ok(_) => panic!("clocks the planner never granted cannot be released"),
        Err(failure) => failure,
    };
    assert_eq!(failure.error(), ModemClockError::NotEnabled);
    let _clocked: Ieee802154Clocked = failure.into_owner();
}
