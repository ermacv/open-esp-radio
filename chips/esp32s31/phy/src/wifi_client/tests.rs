use oer_base_time_virtual::VirtualClock;
use oer_esp32s31_hal::{
    domain::RadioDomain,
    ieee80211::client::{WifiClocked, WifiCold},
    owner::PhyInitializationAccess,
    root::RadioHardware,
};

use super::*;
use crate::concurrent::ConcurrentAcquire;
use crate::{
    PhyConfig, PhyState, RegisteredPhyState,
    domain::PhyDomain,
    state::client::{DEFAULT_PLL_TRACK_PERIOD_MICROS, PhyClientState},
};

fn registered() -> (RadioDomain<ConcurrentPhy>, WifiClocked) {
    let (mut radio, partitions) = RadioHardware::for_host_test().split(ConcurrentPhy::new());
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
    (
        radio,
        WifiClocked::for_host_test(WifiCold::from_partition(partitions.wifi)),
    )
}

#[test]
fn wifi_joins_and_leaves_the_shared_domain_as_its_last_client() {
    let (mut radio, clocked) = registered();
    let domain = &mut radio;
    let (membership, acquired) = join_wifi_prepared(domain, &clocked, &VirtualClock::<8>::new())
        .unwrap_or_else(|error| panic!("join failed: {error:?}"));
    // The first acquisition is due at once; its tracking completes.
    assert_eq!(acquired, ConcurrentAcquire::TrackingDue);
    assert!(domain.attachment_mut().complete_tracking_for_test());
    assert!(
        domain
            .attachment()
            .client_snapshot()
            .is_some_and(|clients| clients.contains(RadioClient::Wifi))
    );
    // A second membership would double the client bit.
    assert!(matches!(
        join_wifi_prepared(domain, &clocked, &VirtualClock::<8>::new()),
        Err(ConcurrentPhyError::Acquire(_))
    ));
    assert_eq!(leave_wifi(domain, &clocked, membership).ok(), Some(true));
}

#[test]
fn an_unregistered_domain_admits_no_wifi_client() {
    let (mut radio, partitions) = RadioHardware::for_host_test().split(ConcurrentPhy::new());
    let clocked = WifiClocked::for_host_test(WifiCold::from_partition(partitions.wifi));
    let domain = &mut radio;
    let _ = clocked;
    assert!(matches!(
        crate::concurrent::phy_ready(domain),
        Err(ConcurrentPhyError::NotRegistered)
    ));
}

#[test]
fn a_suspended_wifi_leaves_the_domain_and_resumes_as_a_client() {
    let (mut radio, clocked) = registered();
    let domain = &mut radio;
    let (membership, acquired) = join_wifi_prepared(domain, &clocked, &VirtualClock::<8>::new())
        .unwrap_or_else(|error| panic!("join failed: {error:?}"));
    assert_eq!(acquired, ConcurrentAcquire::TrackingDue);
    assert!(domain.attachment_mut().complete_tracking_for_test());
    let (suspended, last) = suspend_wifi(domain, clocked.clocks_on(), membership)
        .unwrap_or_else(|failure| panic!("suspend failed: {failure:?}"));
    assert!(last, "Wi-Fi was the only client");
    assert!(
        domain
            .attachment()
            .client_snapshot()
            .is_some_and(|clients| !clients.contains(RadioClient::Wifi))
    );
    let (membership, acquired) = resume_wifi_prepared(
        domain,
        clocked.clocks_on(),
        suspended,
        &VirtualClock::<8>::new(),
    )
    .unwrap_or_else(|failure| panic!("resume failed: {failure:?}"));
    assert_eq!(acquired, ConcurrentAcquire::Settled);
    assert!(
        domain
            .attachment()
            .client_snapshot()
            .is_some_and(|clients| clients.contains(RadioClient::Wifi))
    );
    let (suspended, _) = suspend_wifi(domain, clocked.clocks_on(), membership)
        .unwrap_or_else(|failure| panic!("second suspend failed: {failure:?}"));
    leave_suspended_wifi(suspended);
}

#[test]
fn a_rejected_resume_returns_the_suspended_client() {
    let (mut radio, clocked) = registered();
    let domain = &mut radio;
    let (membership, acquired) = join_wifi_prepared(domain, &clocked, &VirtualClock::<8>::new())
        .unwrap_or_else(|error| panic!("join failed: {error:?}"));
    assert_eq!(acquired, ConcurrentAcquire::TrackingDue);
    assert!(domain.attachment_mut().complete_tracking_for_test());
    let (suspended, _) = suspend_wifi(domain, clocked.clocks_on(), membership)
        .unwrap_or_else(|failure| panic!("suspend failed: {failure:?}"));
    // Another membership holds the client bit, so the resume is rejected.
    let (other, _) = join_wifi_prepared(domain, &clocked, &VirtualClock::<8>::new())
        .unwrap_or_else(|error| panic!("join failed: {error:?}"));
    let failure = resume_wifi_prepared(
        domain,
        clocked.clocks_on(),
        suspended,
        &VirtualClock::<8>::new(),
    )
    .err()
    .unwrap_or_else(|| panic!("a held client bit rejects the resume"));
    assert!(matches!(failure.error(), ConcurrentPhyError::Acquire(_)));
    leave_suspended_wifi(failure.into_suspended());
    assert!(leave_wifi(domain, &clocked, other).is_ok());
}

/// Join with a proof for the domain's registration.
fn join_wifi_prepared(
    radio: &mut oer_esp32s31_hal::domain::RadioDomain<crate::concurrent::ConcurrentPhy>,
    clocked: &WifiClocked,
    clock: &impl oer_base_time::Clock,
) -> Result<(WifiPhyMembership, ConcurrentAcquire), ConcurrentPhyError> {
    let ready = PhyReady::for_test(radio);
    join_wifi(ready, clocked, clock)
        .map(|joined| (joined.membership, joined.acquired))
        .map_err(|refused| refused.error)
}

/// Resume with a proof for the domain's registration.
fn resume_wifi_prepared(
    radio: &mut oer_esp32s31_hal::domain::RadioDomain<crate::concurrent::ConcurrentPhy>,
    clocks: &oer_esp32s31_hal::ieee80211::client::WifiClocksOn,
    suspended: WifiPhySuspended,
    clock: &impl oer_base_time::Clock,
) -> Result<(WifiPhyMembership, ConcurrentAcquire), WifiPhySuspendedFailure> {
    let ready = PhyReady::for_test(radio);
    resume_wifi(ready, clocks, suspended, clock)
        .map(|joined| (joined.membership, joined.acquired))
        .map_err(|refused| refused.error)
}
