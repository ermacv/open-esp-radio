use super::{RadioHardware, RadioPhyReleaseError};
use crate::{
    owner::PhyInitializationAccess,
    phy::restore::PhyRouteState,
    shared_radio::{SharedRadio, SharedRadioReleaseError},
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

/// Begin one registration through a fresh lease of `shared`.
fn register(shared: &SharedRadio<()>) -> crate::owner::PhyRegistrationEpoch {
    let mut lease = shared
        .try_acquire()
        .unwrap_or_else(|_| panic!("a fresh arbiter grants its lease"));
    lease.phy_hal().begin_registration_epoch()
}

#[test]
fn every_pending_restore_blocks_the_reunion() {
    for restore in Restore::ALL {
        let (mut shared, partitions) = RadioHardware::for_validation().into_concurrent(());
        restore.occupy(shared.phy_state_mut_for_test());
        let Err(failure) = RadioHardware::from_concurrent(shared, partitions) else {
            panic!("the split reunited over a pending restore");
        };
        assert_eq!(
            failure.error(),
            super::ConcurrentReunionError::Shared(SharedRadioReleaseError::Restore(
                restore.error()
            ))
        );
    }
}

#[test]
fn a_concurrent_split_reunites_and_retires_the_registration() {
    let (shared, partitions) = RadioHardware::for_validation().into_concurrent(());
    let first = register(&shared);
    let second = register(&shared);
    assert_ne!(first, second);
    let (hardware, ()) = RadioHardware::from_concurrent(shared, partitions)
        .unwrap_or_else(|_| panic!("an idle split reunites"));

    // Returning to the neutral root retires the registration.
    let (shared, _partitions) = hardware.into_concurrent(());
    {
        let lease = shared
            .try_acquire()
            .unwrap_or_else(|_| panic!("a fresh arbiter grants its lease"));
        assert_eq!(lease.registration_epoch(), None);
    }
    let third = register(&shared);
    assert!(third != first && third != second);
}

/// The checked teardown waits for a failed power sequence: while common
/// power still holds the sources it took, the shared radio is not reunited,
/// so it never drops those references.
#[test]
fn reunion_waits_for_platform_clocks_a_failed_power_sequence_kept() {
    use crate::power::{PlatformClock, test_clocks::CountingPlatformClocks};
    let platform = CountingPlatformClocks::new();
    let (mut shared, partitions) = RadioHardware::for_validation().into_concurrent(());
    shared.hold_platform_clock_for_test(PlatformClock::Pll160m, &platform);
    let Err(failure) = RadioHardware::from_concurrent(shared, partitions) else {
        panic!("common power still holds the 160 MHz source");
    };
    assert_eq!(
        failure.error(),
        super::ConcurrentReunionError::Shared(SharedRadioReleaseError::PlatformClocksHeld)
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
    use crate::shared_radio::RadioClient;
    let (mut shared, partitions) = RadioHardware::for_validation().into_concurrent(());
    shared.hold_common_power_for_test(RadioClient::Bluetooth);
    let Err(failure) = RadioHardware::from_concurrent(shared, partitions) else {
        panic!("a client still holds common power");
    };
    assert_eq!(
        failure.error(),
        super::ConcurrentReunionError::Shared(SharedRadioReleaseError::CommonPowerHeld)
    );
    let (_shared, _partitions) = failure.into_parts();
}
