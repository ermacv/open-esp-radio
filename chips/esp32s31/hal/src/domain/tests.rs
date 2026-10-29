use super::*;
use crate::phy::registration::PhyRegistration;
use crate::root::RadioPartitions;

fn radio_domain() -> RadioDomain<crate::power::test_clocks::CountingBackend> {
    let RadioPartitions { shared, .. } = RadioPartitions::for_host_test();
    RadioDomain::new(
        shared,
        PhyRouteState::new(PhyRegistration::new()),
        (),
        crate::domain::transaction::PhyIdentity::for_host_test(),
        crate::power::clock::ModemClockPlannerIdentity::new().expect("identity capacity"),
    )
}

fn refuse_poisoned_clock_changes(
    mut radio: RadioDomain<crate::power::test_clocks::CountingBackend>,
) -> RadioDomain<crate::power::test_clocks::CountingBackend> {
    let platform = crate::power::test_clocks::CountingPlatformClocks::logging(|_| {
        panic!("a poisoned domain must not change platform clocks")
    });
    let RadioPartitions { wifi, .. } = RadioPartitions::for_host_test();
    let wifi = WifiRadioRegisters::new(wifi.into_parts().0);
    assert!(radio.state().clocks.any_held());
    assert_eq!(
        radio.enable_slot(0, ModemClockModule::Phy, &platform),
        Err(ModemClockError::Poisoned)
    );
    assert_eq!(radio.disable_slot(0), Err(ModemClockError::Poisoned));
    assert_eq!(
        radio.set_wifi_initialized(&wifi, true),
        Err(ModemClockError::Poisoned)
    );
    let Err((radio, error)) = radio.into_parts() else {
        panic!("the poisoned planner must retain its domain")
    };
    assert_eq!(error, RadioDomainReleaseError::ModemClocksHeld);
    radio
}

#[test]
fn poisoned_acquire_keeps_its_failed_edge_through_refused_changes_and_release() {
    let mut radio = radio_domain();
    let identity = ModemClockPlannerIdentity::new().expect("identity capacity");
    let prepared = ModemClockPlanner::managed(identity)
        .prepare_acquire(crate::power::clock::module_dependencies(
            ModemClockModule::Phy,
        ))
        .unwrap();
    let mut edges = 0;
    let poisoned = oer_espressif_modem_clock::execute_acquire(prepared, |_| {
        edges += 1;
        if edges == 2 { Err(()) } else { Ok(()) }
    })
    .err()
    .expect("the second acquire edge failed");
    let failed = poisoned.dependency();
    assert_eq!(poisoned.completed_edges(), 1);
    radio.state_mut().clocks = ModemClocks::AcquirePoisoned { _owner: poisoned };

    let radio = refuse_poisoned_clock_changes(radio);
    let ModemClocks::AcquirePoisoned { _owner: retained } = &radio.state().clocks else {
        panic!("the failed acquire owner must remain quarantined")
    };
    assert_eq!(retained.dependency(), failed);
    assert_eq!(retained.completed_edges(), 1);
}

#[test]
fn poisoned_release_keeps_its_failed_edge_through_refused_changes_and_release() {
    let mut radio = radio_domain();
    let identity = ModemClockPlannerIdentity::new().expect("identity capacity");
    let prepared = ModemClockPlanner::managed(identity)
        .prepare_acquire(crate::power::clock::module_dependencies(
            ModemClockModule::Phy,
        ))
        .unwrap();
    let (planner, lease) =
        oer_espressif_modem_clock::execute_acquire(prepared, |_| Ok::<(), ()>(())).unwrap();
    let prepared = planner.prepare_release(lease).unwrap();
    let mut edges = 0;
    let poisoned = oer_espressif_modem_clock::execute_release(prepared, |_| {
        edges += 1;
        if edges == 2 { Err(()) } else { Ok(()) }
    })
    .err()
    .expect("the second release edge failed");
    let failed = poisoned.dependency();
    assert_eq!(poisoned.completed_edges(), 1);
    radio.state_mut().clocks = ModemClocks::ReleasePoisoned { _owner: poisoned };

    let radio = refuse_poisoned_clock_changes(radio);
    let ModemClocks::ReleasePoisoned { _owner: retained } = &radio.state().clocks else {
        panic!("the failed release owner must remain quarantined")
    };
    assert_eq!(retained.dependency(), failed);
    assert_eq!(retained.completed_edges(), 1);
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
        domain.begin_registration_for_host_test().epoch()
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
    let RadioPartitions {
        bluetooth,
        ieee802154,
        ..
    } = RadioPartitions::for_host_test();
    let bluetooth = BluetoothTaskRegisters::new(bluetooth.controller);
    let (ieee802154, _interrupts) = crate::ieee802154::mac::split(ieee802154.into_mac());
    let mut radio = radio_domain();
    let domain = &mut radio;

    assert_eq!(domain.btbb_release(&bluetooth), Err(BtbbError::NotAcquired));
    let grant = domain.begin_registration_for_host_test();
    let gain = domain.btbb_publisher().1.publish(&grant, 0x4f).unwrap();
    assert_eq!(gain.value(), 0x4f);
    drop(domain);

    // Bluetooth already initialized the baseband: joining touches no register.
    radio.hold_btbb_for_test(RadioClient::Bluetooth);
    let domain = &mut radio;
    assert_eq!(
        domain.btbb_acquire(&ieee802154, &gain),
        Ok(BtbbAcquired::Joined)
    );
    assert_eq!(
        domain.btbb_acquire(&ieee802154, &gain),
        Err(BtbbError::AlreadyAcquired)
    );
    assert!(domain.holds_btbb(RadioClient::Ieee802154));
    assert_eq!(domain.btbb_release(&ieee802154), Ok(()));
    assert_eq!(domain.btbb_release(&bluetooth), Ok(()));
    drop(domain);
    assert!(radio.into_parts().is_ok());
}

#[test]
fn only_the_latest_gain_of_the_current_registration_admits_btbb() {
    use crate::owner::PhyInitializationAccess;
    let RadioPartitions { ieee802154, .. } = RadioPartitions::for_host_test();
    let (ieee802154, _interrupts) = crate::ieee802154::mac::split(ieee802154.into_mac());
    let mut radio = radio_domain();
    let first = radio.begin_registration_for_host_test();
    let earlier = radio.btbb_publisher().1.publish(&first, 0x4e).unwrap();
    let latest = radio.btbb_publisher().1.publish(&first, 0x4f).unwrap();
    // A later publication replaces an earlier one, before any register access.
    assert_eq!(
        radio.btbb_acquire(&ieee802154, &earlier),
        Err(BtbbError::Stale)
    );
    // A new registration replaces every gain of the earlier one, and the
    // earlier registration's grant publishes nothing.
    let _second = radio.begin_registration_for_host_test();
    assert_eq!(
        radio.btbb_acquire(&ieee802154, &latest),
        Err(BtbbError::Stale)
    );
    assert_eq!(
        radio.btbb_publisher().1.publish(&first, 0x4f),
        Err(BtbbError::Stale)
    );
    assert!(!radio.holds_btbb(RadioClient::Ieee802154));
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
    let wifi = WifiRadioRegisters::new(RadioPartitions::for_host_test().wifi.into_parts().0);
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

#[test]
fn split_coex_ports_borrow_the_actual_domain_priority_table() {
    let mut radio = radio_domain();
    let event = CoexEventId::new(5).unwrap();
    let priority = CoexPti::new(9).unwrap();
    radio.set_coex_pti(event, priority);
    {
        let (timers, _clock) = radio.coex_ports();
        assert_eq!(timers.pti(event), priority);
    }
    radio.set_coex_pti(event, CoexPti::new(3).unwrap());
    let (timers, _clock) = radio.coex_ports();
    assert_eq!(timers.pti(event).value(), 3);
}
