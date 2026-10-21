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
fn registered_domain() -> RadioDomain<crate::test_clocks::TestClockBackend, ConcurrentPhy> {
    let (mut radio, _partitions) =
        RadioHardware::for_host_test()
            .split::<crate::test_clocks::TestClockBackend, _>(ConcurrentPhy::new());
    {
        let domain = &mut radio;
        let grant = domain.phy_hal().begin_registration();
        let epoch = grant.epoch();
        *domain.attachment_mut() = ConcurrentPhy::registered_for_test(PhyDomain::new(
            RegisteredPhyState::from_wrapper_test_model(
                PhyState::new(PhyConfig::production()),
                grant,
            ),
            PhyClientState::for_registration(DEFAULT_PLL_TRACK_PERIOD_MICROS, epoch),
        ));
    }
    radio
}

/// A radio domain whose registered PHY has RF closed.
fn rf_closed_domain() -> RadioDomain<crate::test_clocks::TestClockBackend, ConcurrentPhy> {
    let (mut radio, _partitions) =
        RadioHardware::for_host_test()
            .split::<crate::test_clocks::TestClockBackend, _>(ConcurrentPhy::new());
    {
        let domain = &mut radio;
        let grant = domain.phy_hal().begin_registration();
        let epoch = grant.epoch();
        *domain.attachment_mut() = ConcurrentPhy::rf_closed_for_test(PhyDomain::new(
            RegisteredPhyState::from_wrapper_test_model(
                PhyState::new(PhyConfig::production()),
                grant,
            ),
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
        phy_ready(radio)
            .unwrap()
            .client::<crate::wifi_client::WifiPhyMembership>(),
        PhyClientAccess::Member(_)
    ));

    assert_eq!(release_for_test(radio, RadioClient::Wifi), Ok(false));
    assert_eq!(release_for_test(radio, RadioClient::Bluetooth), Ok(true));
    assert!(radio.attachment().client_snapshot().unwrap().is_empty());
}

#[test]
fn an_unregistered_domain_rejects_every_client_operation() {
    let (mut radio, _partitions) =
        RadioHardware::for_host_test()
            .split::<crate::test_clocks::TestClockBackend, _>(ConcurrentPhy::new());
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
    assert_eq!(release_for_test(radio, RadioClient::Bluetooth), Ok(true));
    assert!(radio.attachment_mut().idle_domain().is_ok());
}

#[test]
fn the_final_state_cache_follows_the_registered_domain_across_rf_close() {
    let identity = crate::PhyCalibrationIdentity {
        rf_cal_version: 7,
        base_mac_address: [2, 3, 5, 7, 11, 13],
        mac_extension: 17,
    };
    let (mut radio, _partitions) =
        RadioHardware::for_host_test()
            .split::<crate::test_clocks::TestClockBackend, _>(ConcurrentPhy::new());
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
    radio: &mut oer_esp32s31_hal::domain::RadioDomain<
        crate::test_clocks::TestClockBackend,
        crate::concurrent::ConcurrentPhy,
    >,
    client: oer_esp32s31_hal::domain::RadioClient,
    clock: &impl oer_time::Clock,
) -> Result<crate::concurrent::ConcurrentAcquire, crate::concurrent::ConcurrentPhyError> {
    crate::concurrent::acquire_client(radio, client, clock)
}

fn release_for_test(
    radio: &mut RadioDomain<crate::test_clocks::TestClockBackend, ConcurrentPhy>,
    client: RadioClient,
) -> Result<bool, ConcurrentPhyError> {
    let key = radio.attachment().permission_key();
    release_client(radio, client, key)
}

#[test]
fn cancelling_the_transport_after_a_write_leaves_the_phy_poisoned() {
    use crate::{
        analog::i2c::analog_registers,
        calibration::cold::{PhyColdI2cRequest, PhyColdI2cTransaction},
        executor::step::{Job, drive, tests::Io},
    };
    use core::{
        future::Future,
        task::{Context, Waker},
    };
    use oer_time::Clock;
    let clock = VirtualClock::<1>::new();
    let mut radio = registered_domain();
    assert_eq!(
        enter_client(&mut radio, RadioClient::Wifi, &clock),
        Ok(ConcurrentAcquire::TrackingDue)
    );
    let mut io = Io::default();
    let writes = io.atoms.clone();
    let mut transaction = PhyColdI2cTransaction::new(PhyColdI2cRequest::write_byte(
        analog_registers::TEMPERATURE_SENSOR_DAC.address(),
        15,
    ));
    {
        let future = async {
            let (_registered, _pending) = radio
                .attachment_mut()
                .take_tracking()
                .expect("tracking was admitted");
            let mut job = Job::i2c(&mut transaction, clock.now(), false).expect("deadline fits");
            drive(&mut job, &mut io, &clock, None).await
        };
        let mut future = core::pin::pin!(future);
        assert!(
            future
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
                .is_pending()
        );
        assert_eq!(writes.get(), 1, "the command write precedes cancellation");
    }
    assert!(matches!(radio.attachment().slot, Slot::Poisoned));
    assert_eq!(
        admit_maintenance(&mut radio),
        Err(ConcurrentPhyError::Poisoned)
    );
}
