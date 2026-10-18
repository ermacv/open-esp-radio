use oer_esp32s31_hal::{
    domain::{RadioClient, RadioDomain},
    owner::PhyInitializationAccess,
    root::RadioHardware,
};
use oer_time_virtual::VirtualClock;

use super::*;
use crate::{
    PhyConfig, PhyState,
    state::client::{DEFAULT_PLL_TRACK_PERIOD_MICROS, PhyClientState},
};

/// A radio domain whose PHY is registered in the domain's own epoch.
fn registered_domain() -> RadioDomain<ConcurrentPhy> {
    let (mut radio, _partitions) = RadioHardware::for_host_test().split(ConcurrentPhy::new());
    {
        let domain = &mut radio;
        let epoch = domain.phy_hal().begin_registration_epoch();
        *domain.attachment_mut() = ConcurrentPhy::registered_for_test(PhyDomain::new(
            RegisteredPhyState::from_wrapper_test_model(PhyState::new(PhyConfig::production())),
            PhyClientState::for_registration(DEFAULT_PLL_TRACK_PERIOD_MICROS, epoch),
        ));
    }
    radio
}

/// A radio domain whose registered PHY has RF closed.
fn rf_closed_domain() -> RadioDomain<ConcurrentPhy> {
    let (mut radio, _partitions) = RadioHardware::for_host_test().split(ConcurrentPhy::new());
    {
        let domain = &mut radio;
        let epoch = domain.phy_hal().begin_registration_epoch();
        *domain.attachment_mut() = ConcurrentPhy::rf_closed_for_test(PhyDomain::new(
            RegisteredPhyState::from_wrapper_test_model(PhyState::new(PhyConfig::production())),
            PhyClientState::for_registration(DEFAULT_PLL_TRACK_PERIOD_MICROS, epoch),
        ));
    }
    radio
}

#[test]
fn clients_share_one_domain_and_leave_it_one_by_one() {
    let mut radio = registered_domain();
    let radio = &mut radio;
    // Each tracking class is due at once on its first acquisition.
    assert_eq!(
        enter_client(radio, RadioClient::Wifi, &VirtualClock::<8>::new()),
        Ok(ConcurrentAcquire::TrackingDue)
    );
    assert!(radio.attachment_mut().complete_tracking_for_test());
    assert_eq!(
        enter_client(radio, RadioClient::Bluetooth, &VirtualClock::<8>::new()),
        Ok(ConcurrentAcquire::TrackingDue)
    );
    assert!(radio.attachment_mut().complete_tracking_for_test());
    let snapshot = radio
        .attachment()
        .client_snapshot()
        .unwrap_or_else(|| panic!("a registered domain has a client set"));
    assert!(snapshot.contains(RadioClient::Wifi));
    assert!(snapshot.contains(RadioClient::Bluetooth));
    assert!(matches!(
        enter_client(radio, RadioClient::Wifi, &VirtualClock::<8>::new()),
        Err(ConcurrentPhyError::Acquire(_))
    ));

    assert_eq!(release_client(radio, RadioClient::Wifi), Ok(false));
    assert_eq!(release_client(radio, RadioClient::Bluetooth), Ok(true));
    assert!(matches!(
        release_client(radio, RadioClient::Bluetooth),
        Err(ConcurrentPhyError::Release(_))
    ));
}

#[test]
fn an_unregistered_domain_rejects_every_client_operation() {
    let (mut radio, _partitions) = RadioHardware::for_host_test().split(ConcurrentPhy::new());
    let domain = &mut radio;
    assert_eq!(
        phy_ready(domain).map(|_| ()),
        Err(ConcurrentPhyError::NotRegistered)
    );
    assert_eq!(
        evaluate_periodic_tracking(domain, &VirtualClock::<8>::new()),
        Err(ConcurrentPhyError::NotRegistered)
    );
    assert_eq!(
        admit_maintenance(&domain),
        Err(ConcurrentPhyError::NotRegistered)
    );
}

#[test]
fn due_tracking_blocks_clients_until_maintenance_is_admitted() {
    let mut radio = registered_domain();
    let domain = &mut radio;
    // The first acquisition is due at once; its tracking completes.
    assert_eq!(
        enter_client(domain, RadioClient::Wifi, &VirtualClock::<8>::new()),
        Ok(ConcurrentAcquire::TrackingDue)
    );
    assert_eq!(admit_maintenance(domain), Ok(()));
    assert!(domain.attachment_mut().complete_tracking_for_test());
    assert_eq!(
        admit_maintenance(&domain),
        Err(ConcurrentPhyError::NoTrackingPending)
    );
    // One period later the periodic callback requests tracking.
    let later = oer_time_virtual::START
        .checked_add(oer_time::Duration::from_micros(
            DEFAULT_PLL_TRACK_PERIOD_MICROS,
        ))
        .unwrap();
    assert_eq!(
        evaluate_periodic_tracking(domain, &VirtualClock::<8>::starting_at(later)),
        Ok(true)
    );
    assert!(domain.attachment().tracking_pending());
    assert_eq!(
        enter_client(domain, RadioClient::Bluetooth, &VirtualClock::<8>::new()),
        Err(ConcurrentPhyError::TrackingPending)
    );
    // Pending tracking is admitted with the client running, as the vendor's.
    assert_eq!(admit_maintenance(&domain), Ok(()));
}

#[test]
fn a_closed_domain_admits_no_client_until_rf_wakes() {
    let mut radio = rf_closed_domain();
    let radio = &mut radio;
    assert!(radio.attachment().rf_closed());
    assert_eq!(
        radio
            .attachment()
            .client_snapshot()
            .map(|set| set.is_empty()),
        Some(true)
    );
    assert_eq!(
        enter_client(radio, RadioClient::Ieee802154, &VirtualClock::<8>::new()),
        Err(ConcurrentPhyError::RfClosed)
    );
    assert_eq!(
        evaluate_periodic_tracking(radio, &VirtualClock::<8>::new()),
        Err(ConcurrentPhyError::RfClosed)
    );
    assert_eq!(admit_maintenance(&radio), Err(ConcurrentPhyError::RfClosed));
    // RF close takes only an open domain; the closed one stays closed.
    assert!(matches!(
        radio.attachment_mut().idle_domain(),
        Err(ConcurrentPhyError::RfClosed)
    ));
    assert!(radio.attachment().rf_closed());
    assert!(radio.attachment_mut().closed_domain().is_ok());
}

#[test]
fn rf_closes_only_after_the_last_client_left() {
    let mut radio = registered_domain();
    let radio = &mut radio;
    assert!(matches!(
        radio.attachment_mut().closed_domain(),
        Err(ConcurrentPhyError::RfOpen)
    ));
    assert_eq!(
        enter_client(radio, RadioClient::Bluetooth, &VirtualClock::<8>::new()),
        Ok(ConcurrentAcquire::TrackingDue)
    );
    assert!(radio.attachment_mut().complete_tracking_for_test());
    assert!(matches!(
        radio.attachment_mut().idle_domain(),
        Err(ConcurrentPhyError::ClientsActive)
    ));
    // The rejected close leaves the client in the open domain.
    assert_eq!(release_client(radio, RadioClient::Bluetooth), Ok(true));
    assert!(radio.attachment_mut().idle_domain().is_ok());
}

#[test]
fn the_final_state_cache_follows_the_registered_domain_across_rf_close() {
    let identity = crate::PhyCalibrationIdentity {
        rf_cal_version: 7,
        base_mac_address: [2, 3, 5, 7, 11, 13],
        mac_extension: 17,
    };
    let (mut radio, _partitions) = RadioHardware::for_host_test().split(ConcurrentPhy::new());
    {
        let radio = &mut radio;
        assert!(matches!(
            radio.attachment().calibration_cache(identity),
            Err(ConcurrentPhyError::NotRegistered)
        ));
    }
    for mut radio in [registered_domain(), rf_closed_domain()] {
        let radio = &mut radio;
        let cache = radio
            .attachment()
            .calibration_cache(identity)
            .unwrap_or_else(|_| panic!("a registered domain exports its state"));
        assert_eq!(cache.identity(), identity);
    }
}

/// Enter `client` with a proof for the domain's registration.
fn enter_client(
    radio: &mut oer_esp32s31_hal::domain::RadioDomain<crate::concurrent::ConcurrentPhy>,
    client: oer_esp32s31_hal::domain::RadioClient,
    clock: &impl oer_time::Clock,
) -> Result<crate::concurrent::ConcurrentAcquire, crate::concurrent::ConcurrentPhyError> {
    crate::concurrent::acquire_client(radio, client, clock)
}
