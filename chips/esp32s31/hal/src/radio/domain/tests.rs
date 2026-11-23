use super::*;
use crate::power::clock::ModemClockPlanner;
use crate::radio::phy::registration::PhyRegistration;
use crate::system::root::RadioPartitions;

fn radio_domain() -> RadioDomain<crate::power::test_clocks::CountingBackend> {
    let RadioPartitions { shared, .. } = RadioPartitions::for_host_test();
    RadioDomain::new(
        shared,
        PhyRouteState::new(PhyRegistration::new()),
        (),
        crate::radio::domain::transaction::PhyIdentity::for_host_test(),
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
        radio.set_wifi_initialized(
            &wifi,
            crate::radio::ieee80211::client::WifiInitialization::Initialized,
        ),
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
    let poisoned = oer_chips_family_radio_modem_clock::execute_acquire(prepared, |_| {
        edges += 1;
        if edges == 2 { Err(()) } else { Ok(()) }
    })
    .err()
    .expect("the second acquire edge failed");
    let failed = poisoned.dependency();
    assert_eq!(poisoned.completed_edges(), 1);
    radio.state_mut().clocks = ModemClocks::AcquirePoisoned { owner: poisoned };

    let radio = radio.detach(transaction::Operation::Maintain).reunite();
    let radio = refuse_poisoned_clock_changes(radio);
    let ModemClocks::AcquirePoisoned { owner: retained } = &radio.state().clocks else {
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
        oer_chips_family_radio_modem_clock::execute_acquire(prepared, |_| Ok::<(), ()>(()))
            .unwrap();
    let prepared = planner.prepare_release(lease).unwrap();
    let mut edges = 0;
    let poisoned = oer_chips_family_radio_modem_clock::execute_release(prepared, |_| {
        edges += 1;
        if edges == 2 { Err(()) } else { Ok(()) }
    })
    .err()
    .expect("the second release edge failed");
    let failed = poisoned.dependency();
    assert_eq!(poisoned.completed_edges(), 1);
    radio.state_mut().clocks = ModemClocks::ReleasePoisoned { owner: poisoned };

    let radio = radio.detach(transaction::Operation::Maintain).reunite();
    let radio = refuse_poisoned_clock_changes(radio);
    let ModemClocks::ReleasePoisoned { owner: retained } = &radio.state().clocks else {
        panic!("the failed release owner must remain quarantined")
    };
    assert_eq!(retained.dependency(), failed);
    assert_eq!(retained.completed_edges(), 1);
}

#[test]
fn registration_epoch_persists_in_the_domain() {
    use crate::system::owner::PhyInitializationAccess;
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
    use crate::radio::coex::{CoexEventId, CoexPti, CoexPtiTable};

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
        Err(crate::radio::coex::PhyGrantProtectError::NotProtected)
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

/// The block's reset writes the reset bit set, then cleared, then the
/// foreign words: none of the write-back precedes the pulse that clears it.
#[test]
fn the_mac_block_reset_pulses_before_it_writes_back() {
    #[derive(Debug, PartialEq)]
    enum Write {
        Reset(oer_chips_esp32s31_pac::dictionaries::Reset),
        WriteBack(u8),
    }
    struct Port(std::vec::Vec<Write>);
    impl super::MacBlockResetPort for Port {
        fn set_wifi_mac_reset(&mut self, reset: oer_chips_esp32s31_pac::dictionaries::Reset) {
            self.0.push(Write::Reset(reset));
        }
        fn write_back_wifi_mac_foreign_words(&mut self, cbw: u8) {
            self.0.push(Write::WriteBack(cbw));
        }
    }
    let mut port = Port(std::vec::Vec::new());
    let crate::radio::ieee80211::init::MacBlockReset { .. } =
        super::reset_wifi_mac_block(&mut port, 3);
    assert_eq!(
        port.0,
        [
            Write::Reset(oer_chips_esp32s31_pac::dictionaries::Reset::Asserted),
            Write::Reset(oer_chips_esp32s31_pac::dictionaries::Reset::Released),
            Write::WriteBack(3)
        ]
    );
}

/// The reset after a stop is the same pulse and write-back, made once for
/// the stop request it takes, with the release's proof beside the block's.
#[test]
fn the_reset_after_a_stop_is_the_same_pulse_and_gives_the_release_proof() {
    #[derive(Debug, PartialEq)]
    enum Write {
        Reset(oer_chips_esp32s31_pac::dictionaries::Reset),
        WriteBack(u8),
    }
    struct Port(std::vec::Vec<Write>);
    impl super::MacBlockResetPort for Port {
        fn set_wifi_mac_reset(&mut self, reset: oer_chips_esp32s31_pac::dictionaries::Reset) {
            self.0.push(Write::Reset(reset));
        }
        fn write_back_wifi_mac_foreign_words(&mut self, cbw: u8) {
            self.0.push(Write::WriteBack(cbw));
        }
    }
    let mut port = Port(std::vec::Vec::new());
    let stop = crate::radio::ieee80211::mac::StopRequested { _private: () };
    let (
        crate::radio::ieee80211::init::MacBlockReset { .. },
        crate::radio::ieee80211::init::MacStopReset { .. },
    ) = super::reset_stopped_wifi_mac_block(&mut port, stop, 1);
    assert_eq!(
        port.0,
        [
            Write::Reset(oer_chips_esp32s31_pac::dictionaries::Reset::Asserted),
            Write::Reset(oer_chips_esp32s31_pac::dictionaries::Reset::Released),
            Write::WriteBack(1)
        ]
    );
}

/// The domain gives the planner its Wi-Fi initialization: while Wi-Fi is
/// initialized, disabling the Wi-Fi module keeps the Wi-Fi clock group.
#[test]
fn wifi_initialization_reaches_the_planner_and_keeps_the_wifi_clocks() {
    use crate::power::clock::ModemClockPort;
    use crate::radio::ieee80211::client::WifiInitialization;
    use crate::radio::modem::ModemClockDevice;

    #[derive(Default)]
    struct Port(std::vec::Vec<(ModemClockDevice, bool)>);
    impl ModemClockPort for Port {
        fn prepare_power_state(&mut self) {}
        fn enable_device(&mut self, device: ModemClockDevice) {
            self.0.push((device, true));
        }
        fn disable_device(&mut self, device: ModemClockDevice) {
            self.0.push((device, false));
        }
    }

    for (initialization, mac_disabled) in [
        (WifiInitialization::Uninitialized, true),
        (WifiInitialization::Initialized, false),
    ] {
        let wifi = WifiRadioRegisters::new(RadioPartitions::for_host_test().wifi.into_parts().0);
        let mut radio = radio_domain();
        assert_eq!(radio.set_wifi_initialized(&wifi, initialization), Ok(()));
        let platform = crate::power::test_clocks::CountingPlatformClocks::new();
        let mut port = Port::default();
        let state = radio.state_mut();
        let index = client_index(RadioClient::Wifi);
        assert!(
            state
                .clocks
                .enable(
                    index,
                    crate::power::clock::module_dependencies(ModemClockModule::Wifi),
                    &mut ModemExecution {
                        port: &mut port,
                        held: &mut state.modem_platform_clocks,
                        platform: &platform,
                    },
                )
                .is_ok()
        );
        port.0.clear();
        assert!(
            state
                .clocks
                .disable(
                    index,
                    &mut ModemExecution {
                        port: &mut port,
                        held: &mut state.modem_platform_clocks,
                        platform: (),
                    },
                )
                .is_ok()
        );
        assert_eq!(
            port.0.contains(&(ModemClockDevice::WifiMac, false)),
            mac_disabled,
            "{initialization:?}"
        );
    }
}
