use oer_esp32s31_pac::{RadioPartitions, SharedRadioParts, SharedRadioRegisters};

use crate::{
    phy::{registration::PhyRegistration, restore::PhyRouteState},
    root::RadioHardware,
    shared_radio::{BtbbError, CommonRadioPowerError, ModemClockError, RadioClient, SharedRadio},
};

use super::{Ieee802154Clocked, Ieee802154Cold};

fn arbiter() -> SharedRadio {
    let RadioPartitions {
        radio_phy,
        coexistence,
        shared_radio,
        ..
    } = RadioPartitions::for_host_test();
    SharedRadio::new(
        SharedRadioRegisters::new(SharedRadioParts {
            radio_phy,
            coexistence,
            shared_radio,
        }),
        PhyRouteState::new(PhyRegistration::new()),
        (),
    )
}

#[test]
fn the_cold_client_returns_its_partition_to_the_concurrent_split() {
    let (shared, mut partitions) = RadioHardware::for_host_test().into_concurrent(());
    let cold = Ieee802154Cold::from_partition(partitions.ieee802154);
    partitions.ieee802154 = cold.into_partition();
    let _hardware = RadioHardware::from_concurrent(shared, partitions)
        .unwrap_or_else(|_| panic!("an untouched split reunites"));
}

#[test]
fn a_rejected_power_entry_returns_the_unchanged_cold_client() {
    let mut radio = arbiter();
    radio.hold_common_power_for_test(RadioClient::Ieee802154);
    let (_shared, partitions) = RadioHardware::for_host_test().into_concurrent(());
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));

    let failure = match Ieee802154Cold::from_partition(partitions.ieee802154).power_up(
        &mut lease,
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
    let radio = arbiter();
    let (_shared, partitions) = RadioHardware::for_host_test().into_concurrent(());
    let clocked = Ieee802154Clocked::for_host_test(partitions.ieee802154);
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));

    assert_eq!(
        clocked.release_btbb(&mut lease),
        Err(BtbbError::NotAcquired)
    );
    // SAFETY: the unregistered PHY is rejected before any register access.
    #[allow(unsafe_code, reason = "exercises the rejection before MMIO")]
    let acquired = unsafe { clocked.acquire_btbb(&mut lease, 0) };
    assert_eq!(acquired, Err(BtbbError::Unregistered));
    assert!(!lease.holds_btbb(RadioClient::Ieee802154));
}

#[test]
fn releasing_clocks_the_planner_never_granted_keeps_the_clocked_owner() {
    let radio = arbiter();
    let (_shared, partitions) = RadioHardware::for_host_test().into_concurrent(());
    let clocked = Ieee802154Clocked::for_host_test(partitions.ieee802154);
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));

    // Rejected by the planner before any register access.
    let failure = match clocked.disable_clocks(&mut lease) {
        Ok(_) => panic!("clocks the planner never granted cannot be released"),
        Err(failure) => failure,
    };
    assert_eq!(failure.error(), ModemClockError::NotEnabled);
    let _clocked: Ieee802154Clocked = failure.into_owner();
}
