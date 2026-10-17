use super::*;
use crate::phy::registration::PhyRegistration;
use oer_esp32s31_pac::{RadioPartitions, SharedRadioParts};

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
fn only_one_lease_exists_at_a_time() {
    let radio = arbiter();
    let lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    assert!(radio.is_held());
    assert_eq!(radio.try_acquire().err(), Some(SharedRadioBusy));
    drop(lease);
    assert!(!radio.is_held());
    let _again = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a dropped lease frees the arbiter"));
}

#[test]
#[allow(
    clippy::mem_forget,
    reason = "the test models a lease that is never dropped"
)]
fn a_validation_lease_holds_and_releases_the_arbiter() {
    let mut radio = arbiter();
    core::mem::forget(radio.lease_for_host_test());
    assert!(radio.is_held());
    assert_eq!(radio.try_acquire().err(), Some(SharedRadioBusy));

    let mut radio = arbiter();
    drop(radio.lease_for_host_test());
    assert!(!radio.is_held());
    let _again = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a dropped validation lease frees the arbiter"));
}

#[test]
#[allow(
    clippy::mem_forget,
    reason = "the test models a lease that is never dropped"
)]
fn a_forgotten_lease_keeps_the_arbiter_busy() {
    let radio = arbiter();
    core::mem::forget(
        radio
            .try_acquire()
            .unwrap_or_else(|_| panic!("a free arbiter grants its lease")),
    );
    assert_eq!(radio.try_acquire().err(), Some(SharedRadioBusy));
}

#[test]
fn registration_epoch_persists_across_leases() {
    use crate::owner::PhyInitializationAccess;
    let radio = arbiter();
    let epoch = {
        let mut lease = radio
            .try_acquire()
            .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
        assert_eq!(lease.registration_epoch(), None);
        // Protocol transactions and PHY operations borrow the same owner.
        let _shared = lease.registers_mut().radio_phy();
        lease.phy_hal().begin_registration_epoch()
    };
    let lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a dropped lease frees the arbiter"));
    assert_eq!(lease.registration_epoch(), Some(epoch));
    drop(lease);
    let (_registers, phy, ()) = radio
        .into_parts()
        .unwrap_or_else(|_| panic!("an idle arbiter leaves arbitration"));
    assert_eq!(phy.registration_epoch(), Some(epoch));
}

#[test]
fn btbb_joins_an_initialized_baseband_without_register_access() {
    use crate::owner::PhyInitializationAccess;
    use oer_esp32s31_pac::Ieee802154TaskRegisters;
    let RadioPartitions {
        bluetooth,
        ieee802154,
        ..
    } = RadioPartitions::for_host_test();
    let bluetooth = oer_esp32s31_pac::BluetoothTaskRegisters::new(bluetooth);
    let (ieee802154, _interrupts) = Ieee802154TaskRegisters::new(ieee802154);
    let mut radio = arbiter();
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));

    // Without a registration no gain parameter can be valid.
    // SAFETY: rejected before any register access.
    #[allow(unsafe_code)]
    let unregistered = unsafe { lease.btbb_acquire(&bluetooth, 0) };
    assert_eq!(unregistered, Err(BtbbError::Unregistered));
    assert_eq!(lease.btbb_release(&bluetooth), Err(BtbbError::NotAcquired));
    let _epoch = lease.phy_hal().begin_registration_epoch();
    drop(lease);

    // Bluetooth already initialized the baseband.
    radio.hold_btbb_for_test(RadioClient::Bluetooth);
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    // SAFETY: the baseband is initialized, so joining touches no register.
    #[allow(unsafe_code)]
    let joined = unsafe { lease.btbb_acquire(&ieee802154, 0) };
    assert_eq!(joined, Ok(BtbbAcquired::Joined));
    // SAFETY: rejected before any register access.
    #[allow(unsafe_code)]
    let again = unsafe { lease.btbb_acquire(&ieee802154, 0) };
    assert_eq!(again, Err(BtbbError::AlreadyAcquired));
    assert!(lease.holds_btbb(RadioClient::Ieee802154));
    assert_eq!(lease.btbb_release(&ieee802154), Ok(()));
    assert_eq!(lease.btbb_release(&bluetooth), Ok(()));
    drop(lease);
    assert!(radio.into_parts().is_ok());
}

#[test]
fn the_arbiter_stays_while_btbb_is_held() {
    let mut radio = arbiter();
    radio.hold_btbb_for_test(RadioClient::Ieee802154);
    let Err((_radio, error)) = radio.into_parts() else {
        panic!("IEEE 802.15.4 still holds BTBB");
    };
    assert_eq!(error, SharedRadioReleaseError::BtbbHeld);
}

#[test]
fn a_client_cannot_disable_modem_clocks_it_never_enabled() {
    let wifi = oer_esp32s31_pac::WifiRadioRegisters::new(RadioPartitions::for_host_test().wifi_mac);
    let radio = arbiter();
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    // Rejected before any register access.
    assert_eq!(
        lease.disable_modem_clocks(&wifi),
        Err(ModemClockError::NotEnabled)
    );
    drop(lease);
    assert!(radio.into_parts().is_ok());
}

#[test]
fn the_phy_domain_cannot_disable_a_clock_module_it_never_enabled() {
    let radio = arbiter();
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    // Rejected before any register access, per module slot.
    for module in [PhyClockModule::Phy, PhyClockModule::Calibration] {
        assert_eq!(
            lease.disable_phy_modem_clocks(module),
            Err(ModemClockError::NotEnabled)
        );
    }
    drop(lease);
    assert!(radio.into_parts().is_ok());
}

#[test]
fn the_arbiter_keeps_one_coexistence_priority_table_across_leases() {
    use crate::coex::{CoexEventId, CoexPti, CoexPtiTable};

    let radio = arbiter();
    let rx_ack = CoexEventId::new(3).unwrap_or_else(|| panic!("event 3 exists"));
    {
        let mut lease = radio
            .try_acquire()
            .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
        assert_eq!(lease.coex_pti_table(), CoexPtiTable::VENDOR);
        lease.set_coex_pti(
            rx_ack,
            CoexPti::new(9).unwrap_or_else(|| panic!("9 is a priority")),
        );
    }
    let lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a released arbiter grants its lease"));
    assert_eq!(lease.coex_pti(rx_ack).value(), 9);
    drop(lease);
    assert!(radio.into_parts().is_ok());
}

#[test]
fn the_phy_grant_protect_request_cannot_be_withdrawn_before_it_is_programmed() {
    let radio = arbiter();
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    assert!(!lease.phy_grant_protected());
    // Rejected before any register access.
    assert_eq!(
        lease.release_phy_grant_protect(),
        Err(crate::coex::PhyGrantProtectError::NotProtected)
    );
    drop(lease);
    assert!(radio.into_parts().is_ok());
}

#[test]
fn a_held_validation_grant_protect_is_owned_by_the_lease() {
    let mut radio = arbiter();
    {
        let mut lease = radio.lease_for_host_test();
        assert!(!lease.phy_grant_protected());
        assert_eq!(
            lease.release_phy_grant_protect(),
            Err(PhyGrantProtectError::NotProtected)
        );
    }
    radio.hold_phy_grant_protect_for_host_test();
    let mut lease = radio.lease_for_host_test();
    assert!(lease.phy_grant_protected());
    assert_eq!(
        lease.acquire_phy_grant_protect(),
        Err(PhyGrantProtectError::AlreadyProtected)
    );
}
