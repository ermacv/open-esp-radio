use oer_esp32s31_hal::{
    domain::RadioDomain,
    ieee80211::client::{WifiClocked, WifiCold},
    owner::PhyInitializationAccess,
    root::RadioHardware,
};
use oer_time_virtual::VirtualClock;

use super::*;
use crate::concurrent::ConcurrentAcquire;
use crate::{
    PhyConfig, PhyState, RegisteredPhyState,
    domain::PhyDomain,
    state::client::{DEFAULT_PLL_TRACK_PERIOD_MICROS, PhyClientState},
};

fn registered() -> (
    RadioDomain<crate::test_clocks::TestClockBackend, ConcurrentPhy>,
    WifiClocked,
) {
    let (mut radio, partitions) =
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
        crate::concurrent::phy_ready(domain)
            .unwrap()
            .client::<WifiPhyMembership>(),
        crate::concurrent::PhyClientAccess::Member(_)
    ));
    assert_eq!(leave_wifi(domain, &clocked, membership).ok(), Some(true));
}

#[test]
fn an_unregistered_domain_admits_no_wifi_client() {
    let (mut radio, partitions) =
        RadioHardware::for_host_test()
            .split::<crate::test_clocks::TestClockBackend, _>(ConcurrentPhy::new());
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
    leave_suspended_wifi(domain, suspended)
        .unwrap_or_else(|failure| panic!("retirement failed: {failure:?}"));
    assert!(matches!(
        crate::concurrent::phy_ready(domain)
            .unwrap()
            .client::<WifiPhyMembership>(),
        crate::concurrent::PhyClientAccess::Available(_)
    ));
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
    // The RF bit is clear, but the suspended token still owns fresh join
    // authority; a second join cannot obtain the available-client view.
    assert!(matches!(
        crate::concurrent::phy_ready(domain)
            .unwrap()
            .client::<WifiPhyMembership>(),
        crate::concurrent::PhyClientAccess::Member(_)
    ));
    // A backwards external clock is still a typed outcome and returns the
    // exact suspended permission, which can subsequently resume normally.
    let backwards = VirtualClock::<8>::starting_at(oer_time::Instant::EPOCH);
    let failure = resume_wifi_prepared(domain, clocked.clocks_on(), suspended, &backwards)
        .err()
        .unwrap_or_else(|| panic!("backwards clock rejects resume"));
    assert!(matches!(failure.error(), ConcurrentPhyError::Acquire(_)));
    let (membership, _) = resume_wifi_prepared(
        domain,
        clocked.clocks_on(),
        failure.into_suspended(),
        &VirtualClock::<8>::new(),
    )
    .unwrap_or_else(|failure| panic!("retry failed: {failure:?}"));
    assert!(leave_wifi(domain, &clocked, membership).is_ok());
}

/// Join with a proof for the domain's registration.
fn join_wifi_prepared(
    radio: &mut oer_esp32s31_hal::domain::RadioDomain<
        crate::test_clocks::TestClockBackend,
        crate::concurrent::ConcurrentPhy,
    >,
    clocked: &WifiClocked,
    clock: &impl oer_time::Clock,
) -> Result<(WifiPhyMembership, ConcurrentAcquire), ConcurrentPhyError> {
    let ready = match crate::concurrent::phy_ready(radio)?.client::<WifiPhyMembership>() {
        crate::concurrent::PhyClientAccess::Available(available) => available,
        _ => panic!("fixture joins its unclaimed Wi-Fi client"),
    };
    join_wifi(ready, clocked, clock)
        .map(|joined| (joined.membership, joined.acquired))
        .map_err(|refused| refused.error)
}

/// Resume with a proof for the domain's registration.
fn resume_wifi_prepared(
    radio: &mut oer_esp32s31_hal::domain::RadioDomain<
        crate::test_clocks::TestClockBackend,
        crate::concurrent::ConcurrentPhy,
    >,
    clocks: &oer_esp32s31_hal::ieee80211::client::WifiClocksOn,
    suspended: WifiPhySuspended,
    clock: &impl oer_time::Clock,
) -> Result<(WifiPhyMembership, ConcurrentAcquire), WifiPhySuspendedFailure> {
    let ready = match crate::concurrent::phy_ready(radio) {
        Ok(ready) => ready,
        Err(error) => return Err(WifiPhySuspendedFailure { suspended, error }),
    };
    resume_wifi(ready, clocks, suspended, clock)
        .map(|joined| (joined.membership, joined.acquired))
        .map_err(|refused| refused.error)
}

#[test]
#[should_panic(expected = "PHY registration mismatch")]
fn a_foreign_membership_is_an_internal_identity_fault() {
    let (mut first, first_clocked) = registered();
    let (mut second, second_clocked) = registered();
    let (membership, _) = join_wifi_prepared(&mut first, &first_clocked, &VirtualClock::<8>::new())
        .unwrap_or_else(|error| panic!("first join: {error:?}"));
    assert!(first.attachment_mut().complete_tracking_for_test());
    let (_second_membership, _) =
        join_wifi_prepared(&mut second, &second_clocked, &VirtualClock::<8>::new())
            .unwrap_or_else(|error| panic!("second join: {error:?}"));
    assert!(second.attachment_mut().complete_tracking_for_test());
    // Test deliberately violates the private handoff invariant. Neither
    // public construction nor copying can manufacture a matching permission.
    let _ = leave_wifi(&mut second, &second_clocked, membership);
}

#[test]
fn wifi_due_rejoin_and_clock_refusal_preserve_membership_authority() {
    use oer_time::{Duration, Instant};
    let (mut radio, clocked) = registered();
    let clock = VirtualClock::<8>::new();
    let (membership, acquired) =
        join_wifi_prepared(&mut radio, &clocked, &clock).expect("valid PHY lifecycle fixture");
    assert_eq!(acquired, ConcurrentAcquire::TrackingDue);
    assert!(radio.attachment_mut().complete_tracking_for_test());
    assert!(leave_wifi(&mut radio, &clocked, membership).expect("valid PHY lifecycle fixture"));
    let before = radio.attachment().client_snapshot();
    let past = VirtualClock::<8>::starting_at(Instant::EPOCH);
    assert!(matches!(
        join_wifi_prepared(&mut radio, &clocked, &past),
        Err(ConcurrentPhyError::Acquire(_))
    ));
    assert_eq!(radio.attachment().client_snapshot(), before);
    let (membership, acquired) =
        join_wifi_prepared(&mut radio, &clocked, &clock).expect("valid PHY lifecycle fixture");
    assert_eq!(acquired, ConcurrentAcquire::Settled);
    assert!(leave_wifi(&mut radio, &clocked, membership).expect("valid PHY lifecycle fixture"));
    clock
        .advance(Duration::from_micros(DEFAULT_PLL_TRACK_PERIOD_MICROS + 1))
        .expect("valid PHY lifecycle fixture");
    let (membership, acquired) =
        join_wifi_prepared(&mut radio, &clocked, &clock).expect("valid PHY lifecycle fixture");
    assert_eq!(acquired, ConcurrentAcquire::TrackingDue);
    assert!(radio.attachment_mut().complete_tracking_for_test());
    assert!(leave_wifi(&mut radio, &clocked, membership).expect("valid PHY lifecycle fixture"));
    assert!(!radio.holds_btbb(RadioClient::Bluetooth));
    assert!(!radio.holds_btbb(RadioClient::Ieee802154));
}
