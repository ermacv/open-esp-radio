use super::*;
use crate::phy::registration::PhyRegistration;
use oer_esp32s31_pac::{RadioPartitions, SharedRadioParts};

fn radio_domain() -> RadioDomain {
    let RadioPartitions {
        radio_phy,
        coexistence,
        shared_radio,
        ..
    } = RadioPartitions::for_host_test();
    RadioDomain::new(
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
fn registration_epoch_persists_in_the_domain() {
    use crate::owner::PhyInitializationAccess;
    let mut radio = radio_domain();
    let epoch = {
        let domain = &mut radio;
        assert_eq!(domain.registration_epoch(), None);
        // Protocol transactions and PHY operations borrow the same owner.
        let _shared = domain.registers_mut().radio_phy();
        domain.phy_hal().begin_registration_epoch()
    };
    let domain = &mut radio;
    assert_eq!(domain.registration_epoch(), Some(epoch));
    drop(domain);
    let (_registers, phy, ()) = radio
        .into_parts()
        .unwrap_or_else(|_| panic!("an idle domain gives its parts back"));
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
    let mut radio = radio_domain();
    let domain = &mut radio;

    // Without a registration no gain parameter can be valid.
    // SAFETY: rejected before any register access.
    #[allow(unsafe_code)]
    let unregistered = unsafe { domain.btbb_acquire(&bluetooth, 0) };
    assert_eq!(unregistered, Err(BtbbError::Unregistered));
    assert_eq!(domain.btbb_release(&bluetooth), Err(BtbbError::NotAcquired));
    let _epoch = domain.phy_hal().begin_registration_epoch();
    drop(domain);

    // Bluetooth already initialized the baseband.
    radio.hold_btbb_for_test(RadioClient::Bluetooth);
    let domain = &mut radio;
    // SAFETY: the baseband is initialized, so joining touches no register.
    #[allow(unsafe_code)]
    let joined = unsafe { domain.btbb_acquire(&ieee802154, 0) };
    assert_eq!(joined, Ok(BtbbAcquired::Joined));
    // SAFETY: rejected before any register access.
    #[allow(unsafe_code)]
    let again = unsafe { domain.btbb_acquire(&ieee802154, 0) };
    assert_eq!(again, Err(BtbbError::AlreadyAcquired));
    assert!(domain.holds_btbb(RadioClient::Ieee802154));
    assert_eq!(domain.btbb_release(&ieee802154), Ok(()));
    assert_eq!(domain.btbb_release(&bluetooth), Ok(()));
    drop(domain);
    assert!(radio.into_parts().is_ok());
}

#[test]
fn the_domain_stays_while_btbb_is_held() {
    let mut radio = radio_domain();
    radio.hold_btbb_for_test(RadioClient::Ieee802154);
    let Err((_radio, error)) = radio.into_parts() else {
        panic!("IEEE 802.15.4 still holds BTBB");
    };
    assert_eq!(error, RadioDomainReleaseError::BtbbHeld);
}

#[test]
fn a_client_cannot_disable_modem_clocks_it_never_enabled() {
    let wifi = oer_esp32s31_pac::WifiRadioRegisters::new(RadioPartitions::for_host_test().wifi_mac);
    let mut radio = radio_domain();
    let domain = &mut radio;
    // Rejected before any register access.
    assert_eq!(
        domain.disable_modem_clocks(&wifi),
        Err(ModemClockError::NotEnabled)
    );
    drop(domain);
    assert!(radio.into_parts().is_ok());
}

#[test]
fn the_phy_domain_cannot_disable_a_clock_module_it_never_enabled() {
    let mut radio = radio_domain();
    let domain = &mut radio;
    // Rejected before any register access, per module slot.
    for module in [PhyClockModule::Phy, PhyClockModule::Calibration] {
        assert_eq!(
            domain.disable_phy_modem_clocks(module),
            Err(ModemClockError::NotEnabled)
        );
    }
    drop(domain);
    assert!(radio.into_parts().is_ok());
}

#[test]
fn the_domain_keeps_one_coexistence_priority_table() {
    use crate::coex::{CoexEventId, CoexPti, CoexPtiTable};

    let mut radio = radio_domain();
    let rx_ack = CoexEventId::new(3).unwrap_or_else(|| panic!("event 3 exists"));
    {
        let domain = &mut radio;
        assert_eq!(domain.coex_pti_table(), CoexPtiTable::VENDOR);
        domain.set_coex_pti(
            rx_ack,
            CoexPti::new(9).unwrap_or_else(|| panic!("9 is a priority")),
        );
    }
    let domain = &mut radio;
    assert_eq!(domain.coex_pti(rx_ack).value(), 9);
    drop(domain);
    assert!(radio.into_parts().is_ok());
}

#[test]
fn the_phy_grant_protect_request_cannot_be_withdrawn_before_it_is_programmed() {
    let mut radio = radio_domain();
    let domain = &mut radio;
    assert!(!domain.phy_grant_protected());
    // Rejected before any register access.
    assert_eq!(
        domain.release_phy_grant_protect(),
        Err(crate::coex::PhyGrantProtectError::NotProtected)
    );
    drop(domain);
    assert!(radio.into_parts().is_ok());
}

#[test]
fn a_held_host_test_grant_protect_is_owned_by_the_domain() {
    let mut radio = radio_domain();
    {
        let domain = &mut radio;
        assert!(!domain.phy_grant_protected());
        assert_eq!(
            domain.release_phy_grant_protect(),
            Err(PhyGrantProtectError::NotProtected)
        );
    }
    radio.hold_phy_grant_protect_for_host_test();
    let domain = &mut radio;
    assert!(domain.phy_grant_protected());
    assert_eq!(
        domain.acquire_phy_grant_protect(),
        Err(PhyGrantProtectError::AlreadyProtected)
    );
}
