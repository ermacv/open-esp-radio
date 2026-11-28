use super::{RadioHardware, RadioPhyReleaseError};
use crate::{
    radio::domain::{RadioDomain, RadioDomainReleaseError},
    radio::phy::restore::PhyRouteState,
};

#[derive(Clone, Copy)]
enum Restore {
    TxDcPwdet,
    TxIqToneControl,
    RxDcoControl,
    BluetoothTxPowerControl,
}

impl Restore {
    const ALL: [Self; 4] = [
        Self::TxDcPwdet,
        Self::TxIqToneControl,
        Self::RxDcoControl,
        Self::BluetoothTxPowerControl,
    ];

    const fn error(self) -> RadioPhyReleaseError {
        match self {
            Self::TxDcPwdet => RadioPhyReleaseError::TxDcPwdetRestorePending,
            Self::TxIqToneControl => RadioPhyReleaseError::TxIqToneControlRestorePending,
            Self::RxDcoControl => RadioPhyReleaseError::RxDcoControlRestorePending,
            Self::BluetoothTxPowerControl => {
                RadioPhyReleaseError::BluetoothTxPowerControlRestorePending
            }
        }
    }

    fn occupy(self, slot: &mut PhyRouteState) {
        match self {
            Self::TxDcPwdet => slot.occupy_txdc_for_test(),
            Self::TxIqToneControl => slot.occupy_txiq_for_test(),
            Self::RxDcoControl => slot.occupy_rx_dco_for_test(),
            Self::BluetoothTxPowerControl => slot.occupy_bluetooth_tx_power_control_for_test(),
        }
    }
}

/// Begin one registration in `shared`.
fn register(shared: &mut RadioDomain) -> crate::system::owner::PhyRegistrationEpoch {
    shared.begin_registration_for_host_test().epoch()
}

#[test]
fn every_pending_restore_blocks_the_reunion() {
    for restore in Restore::ALL {
        let (mut shared, partitions) = RadioHardware::for_host_test()
            .split(crate::system::root::host_place())
            .expect("identity capacity");
        restore.occupy(shared.phy_state_mut_for_test());
        let Err(failure) = RadioHardware::reunite(shared, partitions) else {
            panic!("the split reunited over a pending restore");
        };
        assert_eq!(
            failure.error(),
            super::ReunionError::Shared(RadioDomainReleaseError::Restore(restore.error()))
        );
    }
}

#[test]
fn a_split_reunites_and_retires_the_registration() {
    let (mut shared, partitions) = RadioHardware::for_host_test()
        .split(crate::system::root::host_place())
        .expect("identity capacity");
    let first = register(&mut shared);
    let second = register(&mut shared);
    assert_ne!(first, second);
    let hardware = RadioHardware::reunite(shared, partitions)
        .unwrap_or_else(|_| panic!("an idle split reunites"));

    // Returning to the neutral root retires the registration.
    let (mut shared, _partitions) = hardware
        .split(crate::system::root::host_place())
        .expect("identity capacity");
    {
        let radio = &mut shared;
        assert_eq!(radio.registration_epoch(), None);
    }
    let third = register(&mut shared);
    assert!(third != first && third != second);
}

#[test]
fn reunion_waits_for_common_power() {
    use crate::radio::domain::RadioClient;
    let (mut shared, partitions) = RadioHardware::for_host_test()
        .split(crate::system::root::host_place())
        .expect("identity capacity");
    shared.hold_common_power_for_test(RadioClient::Bluetooth);
    let Err(failure) = RadioHardware::reunite(shared, partitions) else {
        panic!("a client still holds common power");
    };
    assert_eq!(
        failure.error(),
        super::ReunionError::Shared(RadioDomainReleaseError::CommonPowerHeld)
    );
    let (_shared, _partitions) = failure.into_parts();
}

#[test]
fn exhausted_clock_identity_returns_every_split_owner_before_construction() {
    use oer_chips_family_radio_modem_clock::IdentityExhausted;
    let failure = RadioHardware::for_host_test()
        .split_with_clock_identity(super::host_place(), Err(IdentityExhausted))
        .err()
        .expect("exhausted identity refuses split");
    assert_eq!(failure.error(), IdentityExhausted);
    let (domain, partitions) = failure
        .into_hardware()
        .split(super::host_place())
        .expect("new identity available");
    let _hardware = RadioHardware::reunite(domain, partitions)
        .unwrap_or_else(|_| panic!("unchanged root reunites"));
}
