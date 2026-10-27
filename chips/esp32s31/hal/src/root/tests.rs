use super::{RadioHardware, RadioPhyReleaseError};
use crate::{
    domain::{RadioDomain, RadioDomainReleaseError},
    owner::PhyInitializationAccess,
    phy::restore::PhyRouteState,
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
fn register(
    shared: &mut RadioDomain<crate::power::test_clocks::CountingBackend>,
) -> crate::owner::PhyRegistrationEpoch {
    shared.begin_registration_for_host_test().epoch()
}

#[test]
fn every_pending_restore_blocks_the_reunion() {
    for restore in Restore::ALL {
        let (mut shared, partitions) = RadioHardware::for_host_test()
            .split::<crate::power::test_clocks::CountingBackend, _>(
            (),
            crate::domain::transaction::PhyIdentity::for_host_test(),
        );
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
        .split::<crate::power::test_clocks::CountingBackend, _>(
        (),
        crate::domain::transaction::PhyIdentity::for_host_test(),
    );
    let first = register(&mut shared);
    let second = register(&mut shared);
    assert_ne!(first, second);
    let (hardware, ()) = RadioHardware::reunite(shared, partitions)
        .unwrap_or_else(|_| panic!("an idle split reunites"));

    // Returning to the neutral root retires the registration.
    let (mut shared, _partitions) = hardware
        .split::<crate::power::test_clocks::CountingBackend, _>(
            (),
            crate::domain::transaction::PhyIdentity::for_host_test(),
        );
    {
        let radio = &mut shared;
        assert_eq!(radio.registration_epoch(), None);
    }
    let third = register(&mut shared);
    assert!(third != first && third != second);
}

/// The checked teardown waits for a failed power sequence: while common
/// power still holds the sources it took, the shared radio is not reunited,
/// so it never drops those references.
#[test]
fn reunion_waits_for_platform_clocks_a_failed_power_sequence_kept() {
    use crate::power::{PlatformClock, test_clocks::CountingPlatformClocks};
    let platform = CountingPlatformClocks::new();
    let (mut shared, partitions) = RadioHardware::for_host_test()
        .split::<crate::power::test_clocks::CountingBackend, _>(
        (),
        crate::domain::transaction::PhyIdentity::for_host_test(),
    );
    shared.hold_platform_clock_for_test(PlatformClock::Pll160m, &platform);
    let Err(failure) = RadioHardware::reunite(shared, partitions) else {
        panic!("common power still holds the 160 MHz source");
    };
    assert_eq!(
        failure.error(),
        super::ReunionError::Shared(RadioDomainReleaseError::PlatformClocksHeld)
    );
    let (mut shared, _partitions) = failure.into_parts();
    shared.abandon_platform_clocks();
    assert_eq!(
        crate::power::test_clocks::abandoned().0,
        crate::power::test_clocks::holding(&[PlatformClock::Pll160m])
    );
}

#[test]
fn reunion_waits_for_common_power() {
    use crate::domain::RadioClient;
    let (mut shared, partitions) = RadioHardware::for_host_test()
        .split::<crate::power::test_clocks::CountingBackend, _>(
        (),
        crate::domain::transaction::PhyIdentity::for_host_test(),
    );
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
