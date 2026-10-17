use oer_esp32s31_hal::{
    owner::PhyInitializationAccess,
    root::RadioHardware,
    shared_radio::{RadioClient, SharedRadio},
};
use oer_time_virtual::VirtualClock;

use super::*;
use crate::{
    PhyConfig, PhyState,
    state::client::{DEFAULT_PLL_TRACK_PERIOD_MICROS, PhyClientState},
};

/// An arbiter whose domain is registered in the arbiter's own epoch.
fn registered_arbiter() -> SharedRadio<ConcurrentPhy> {
    let (radio, _partitions) = RadioHardware::for_host_test().into_concurrent(ConcurrentPhy::new());
    {
        let mut lease = radio
            .try_acquire()
            .unwrap_or_else(|_| panic!("a fresh arbiter grants its lease"));
        let epoch = lease.phy_hal().begin_registration_epoch();
        *lease.attachment_mut() = ConcurrentPhy::registered_for_test(PhyDomain::new(
            RegisteredPhyState::from_wrapper_test_model(PhyState::new(PhyConfig::production())),
            PhyClientState::for_registration(DEFAULT_PLL_TRACK_PERIOD_MICROS, epoch),
        ));
    }
    radio
}

/// An arbiter whose registered domain has RF closed.
fn rf_closed_arbiter() -> SharedRadio<ConcurrentPhy> {
    let (radio, _partitions) = RadioHardware::for_host_test().into_concurrent(ConcurrentPhy::new());
    {
        let mut lease = radio
            .try_acquire()
            .unwrap_or_else(|_| panic!("a fresh arbiter grants its lease"));
        let epoch = lease.phy_hal().begin_registration_epoch();
        *lease.attachment_mut() = ConcurrentPhy::rf_closed_for_test(PhyDomain::new(
            RegisteredPhyState::from_wrapper_test_model(PhyState::new(PhyConfig::production())),
            PhyClientState::for_registration(DEFAULT_PLL_TRACK_PERIOD_MICROS, epoch),
        ));
    }
    radio
}

#[test]
fn clients_share_one_domain_and_leave_it_one_by_one() {
    let radio = registered_arbiter();
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    assert_eq!(
        enter_client(&mut lease, RadioClient::Wifi, &VirtualClock::<8>::new()),
        Ok(ConcurrentAcquire::Settled)
    );
    assert_eq!(
        enter_client(
            &mut lease,
            RadioClient::Bluetooth,
            &VirtualClock::<8>::new()
        ),
        Ok(ConcurrentAcquire::Settled)
    );
    let snapshot = lease
        .attachment()
        .client_snapshot()
        .unwrap_or_else(|| panic!("a registered domain has a client set"));
    assert!(snapshot.contains(RadioClient::Wifi));
    assert!(snapshot.contains(RadioClient::Bluetooth));
    assert!(matches!(
        enter_client(&mut lease, RadioClient::Wifi, &VirtualClock::<8>::new()),
        Err(ConcurrentPhyError::Acquire(_))
    ));

    assert_eq!(release_client(&mut lease, RadioClient::Wifi), Ok(false));
    assert_eq!(release_client(&mut lease, RadioClient::Bluetooth), Ok(true));
    assert!(matches!(
        release_client(&mut lease, RadioClient::Bluetooth),
        Err(ConcurrentPhyError::Release(_))
    ));
}

#[test]
fn an_unregistered_domain_rejects_every_client_operation() {
    let (radio, _partitions) = RadioHardware::for_host_test().into_concurrent(ConcurrentPhy::new());
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a fresh arbiter grants its lease"));
    assert_eq!(
        phy_ready(&mut lease).map(|_| ()),
        Err(ConcurrentPhyError::NotRegistered)
    );
    assert_eq!(
        evaluate_periodic_tracking(&mut lease, &VirtualClock::<8>::new()),
        Err(ConcurrentPhyError::NotRegistered)
    );
    assert_eq!(
        admit_maintenance(&lease),
        Err(ConcurrentPhyError::NotRegistered)
    );
}

#[test]
fn due_tracking_blocks_clients_until_maintenance_is_admitted() {
    let radio = registered_arbiter();
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    assert_eq!(
        enter_client(&mut lease, RadioClient::Wifi, &VirtualClock::<8>::new()),
        Ok(ConcurrentAcquire::Settled)
    );
    assert_eq!(
        admit_maintenance(&lease),
        Err(ConcurrentPhyError::NoTrackingPending)
    );
    // One period later the periodic callback requests tracking.
    assert_eq!(
        evaluate_periodic_tracking(
            &mut lease,
            &VirtualClock::<8>::starting_at(oer_time::Instant::from_micros(
                DEFAULT_PLL_TRACK_PERIOD_MICROS
            ))
        ),
        Ok(true)
    );
    assert!(lease.attachment().tracking_pending());
    assert_eq!(
        enter_client(
            &mut lease,
            RadioClient::Bluetooth,
            &VirtualClock::<8>::new()
        ),
        Err(ConcurrentPhyError::TrackingPending)
    );
    // Pending tracking is admitted with the client running, as the vendor's.
    assert_eq!(admit_maintenance(&lease), Ok(()));
}

#[test]
fn a_closed_domain_admits_no_client_until_rf_wakes() {
    let radio = rf_closed_arbiter();
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    assert!(lease.attachment().rf_closed());
    assert_eq!(
        lease
            .attachment()
            .client_snapshot()
            .map(|set| set.is_empty()),
        Some(true)
    );
    assert_eq!(
        enter_client(
            &mut lease,
            RadioClient::Ieee802154,
            &VirtualClock::<8>::new()
        ),
        Err(ConcurrentPhyError::RfClosed)
    );
    assert_eq!(
        evaluate_periodic_tracking(&mut lease, &VirtualClock::<8>::new()),
        Err(ConcurrentPhyError::RfClosed)
    );
    assert_eq!(admit_maintenance(&lease), Err(ConcurrentPhyError::RfClosed));
    // RF close takes only an open domain; the closed one stays closed.
    assert!(matches!(
        lease.attachment_mut().idle_domain(),
        Err(ConcurrentPhyError::RfClosed)
    ));
    assert!(lease.attachment().rf_closed());
    assert!(lease.attachment_mut().closed_domain().is_ok());
}

#[test]
fn rf_closes_only_after_the_last_client_left() {
    let radio = registered_arbiter();
    let mut lease = radio
        .try_acquire()
        .unwrap_or_else(|_| panic!("a free arbiter grants its lease"));
    assert!(matches!(
        lease.attachment_mut().closed_domain(),
        Err(ConcurrentPhyError::RfOpen)
    ));
    assert_eq!(
        enter_client(
            &mut lease,
            RadioClient::Bluetooth,
            &VirtualClock::<8>::new()
        ),
        Ok(ConcurrentAcquire::Settled)
    );
    assert!(matches!(
        lease.attachment_mut().idle_domain(),
        Err(ConcurrentPhyError::ClientsActive)
    ));
    // The rejected close leaves the client in the open domain.
    assert_eq!(release_client(&mut lease, RadioClient::Bluetooth), Ok(true));
    assert!(lease.attachment_mut().idle_domain().is_ok());
}

#[test]
fn the_final_state_cache_follows_the_registered_domain_across_rf_close() {
    let identity = crate::PhyCalibrationIdentity {
        rf_cal_version: 7,
        base_mac_address: [2, 3, 5, 7, 11, 13],
        mac_extension: 17,
    };
    let (radio, _partitions) = RadioHardware::for_host_test().into_concurrent(ConcurrentPhy::new());
    {
        let lease = radio
            .try_acquire()
            .unwrap_or_else(|_| panic!("a fresh arbiter grants its lease"));
        assert!(matches!(
            lease.attachment().calibration_cache(identity),
            Err(ConcurrentPhyError::NotRegistered)
        ));
    }
    for radio in [registered_arbiter(), rf_closed_arbiter()] {
        let lease = radio
            .try_acquire()
            .unwrap_or_else(|_| panic!("a fresh arbiter grants its lease"));
        let cache = lease
            .attachment()
            .calibration_cache(identity)
            .unwrap_or_else(|_| panic!("a registered domain exports its state"));
        assert_eq!(cache.identity(), identity);
    }
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
