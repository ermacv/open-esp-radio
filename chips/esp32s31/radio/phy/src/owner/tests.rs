use super::*;
use crate::{PhyConfig, PhyState};
use core::mem::MaybeUninit;
use oer_portable_memory::Occupied;

fn at(micros: u64) -> Instant {
    Instant::from_micros(micros)
}

#[test]
fn a_registered_state_never_tracked_is_due_at_once() {
    let state = PhyState::new(PhyConfig::esp32s31_default());
    assert_eq!(tracking_due(&state, at(0)), Tracking::Due);
}

#[test]
fn a_pass_is_due_once_more_than_the_period_has_passed() {
    let mut state = PhyState::new(PhyConfig::esp32s31_default());
    state.mark_tracked(at(5_000_000));
    assert_eq!(
        tracking_due(&state, at(5_000_000)),
        Tracking::NotBefore(at(6_000_000))
    );
    assert_eq!(
        tracking_due(&state, at(6_000_000)),
        Tracking::NotBefore(at(6_000_000)),
        "exactly the period is not more than it"
    );
    assert_eq!(tracking_due(&state, at(6_000_001)), Tracking::Due);
}

#[test]
fn an_instant_before_the_last_pass_waits_for_the_period() {
    let mut state = PhyState::new(PhyConfig::esp32s31_default());
    state.mark_tracked(at(5_000_000));
    assert_eq!(
        tracking_due(&state, at(1)),
        Tracking::NotBefore(at(6_000_000))
    );
}

type Radio =
    oer_chips_esp32s31_hal::radio::domain::RadioDomain<crate::test_clocks::TestClockBackend>;

/// A test's own memory for a split's domain.
fn domain_place() -> &'static mut MaybeUninit<Radio> {
    std::boxed::Box::leak(std::boxed::Box::new(MaybeUninit::uninit()))
}

/// The one right of a fresh split, with what the reunion takes back.
fn split() -> (
    Occupied<'static, Radio>,
    oer_chips_esp32s31_hal::system::root::ProtocolPartitions,
) {
    oer_chips_esp32s31_hal::system::root::RadioHardware::for_host_test()
        .split::<crate::test_clocks::TestClockBackend>(domain_place())
        .unwrap_or_else(|_| panic!("identity capacity"))
}

/// The proof a completed registration gives, from the fill's.
fn registered(cold: RfCold<ChipPhy>) -> RfOpen<ChipPhy> {
    cold.registered()
}

#[test]
fn the_last_pass_outlives_rf_closed() {
    let (_radio, partitions) = split();
    let mut memory = MaybeUninit::uninit();
    let (mut phy, cold) = PhyPlace::new(partitions.phy, &mut memory).fill();
    phy.state_mut().mark_tracked(at(7));
    let closed = registered(cold).close();
    assert_eq!(phy.state(&closed).tracked_at(), Some(at(7)));
    let open = closed.open();
    assert_eq!(phy.state(&open).tracked_at(), Some(at(7)));
}

#[test]
fn the_calibration_cache_is_the_live_state_s() {
    let (_radio, partitions) = split();
    let mut memory = MaybeUninit::uninit();
    let (mut phy, cold) = PhyPlace::new(partitions.phy, &mut memory).fill();
    let open = registered(cold);
    let identity = crate::PhyCalibrationIdentity {
        rf_cal_version: 7,
        base_mac_address: [1, 2, 3, 4, 5, 6],
        mac_extension: 0x0908,
    };
    let before = *phy.calibration_cache(&open, identity).snapshot();
    // A later write to the registered state is in the next capture: the
    // reading is of the state as it is, not of the registration's end.
    phy.state_mut().apply_rc_calibration(45);
    let after = phy.calibration_cache(&open, identity);
    assert!(after.matches(identity));
    assert_ne!(*after.snapshot(), before);
    assert_eq!(
        after.snapshot(),
        phy.state(&open).calibration_cache(identity).snapshot()
    );
}

#[test]
fn a_failed_operation_s_right_empties_the_place_for_the_next_fill() {
    let (_radio, partitions) = split();
    let mut memory = MaybeUninit::uninit();
    let (mut phy, cold) = PhyPlace::new(partitions.phy, &mut memory).fill();
    phy.state_mut().mark_tracked(at(3));
    // A failed close consumed the proof: the right to empty the place is
    // what is left.
    let vacate = registered(cold).fail();
    let (phy, cold) = phy.vacate(vacate).fill();
    assert_eq!(
        phy.state(&registered(cold)).tracked_at(),
        None,
        "the next fill's state is new"
    );
}

#[test]
fn a_failed_wake_and_a_failed_registration_leave_the_same_right() {
    let (_radio, partitions) = split();
    let mut memory = MaybeUninit::uninit();
    let (phy, cold) = PhyPlace::new(partitions.phy, &mut memory).fill();
    let place = phy.vacate(registered(cold).close().fail());
    let (phy, cold) = place.fill();
    let _place = phy.vacate(cold.fail());
}

#[test]
fn a_place_filled_and_not_registered_empties_with_its_cold_proof() {
    let (_radio, partitions) = split();
    let mut memory = MaybeUninit::uninit();
    let (mut phy, cold) = PhyPlace::new(partitions.phy, &mut memory).fill();
    phy.state_mut().mark_tracked(at(9));
    let (phy, cold) = phy.empty(cold).fill();
    assert_eq!(phy.state(&registered(cold)).tracked_at(), None);
}

#[test]
fn a_threshold_needs_a_registration_and_reaches_the_state() {
    let (_radio, partitions) = split();
    let mut memory = MaybeUninit::uninit();
    let (mut phy, cold) = PhyPlace::new(partitions.phy, &mut memory).fill();
    let open = registered(cold);
    phy.set_calibration_threshold(&open, crate::CalibrationThreshold::Diagnostic(0));
    assert_eq!(
        phy.state(&open)
            .temperature_tracking_debug()
            .calibration_threshold_override(),
        Some(0)
    );
}

#[test]
fn the_right_goes_back_to_the_reunion_and_the_next_split_hands_it_out_again() {
    let (radio, partitions) = split();
    let oer_chips_esp32s31_hal::system::root::ProtocolPartitions {
        wifi,
        wifi_coex,
        phy: right,
    } = partitions;
    let mut memory = MaybeUninit::uninit();
    let (phy, cold) = PhyPlace::new(right, &mut memory).fill();
    let right = phy.empty(cold).into_right();
    let hardware = oer_chips_esp32s31_hal::system::root::RadioHardware::reunite(
        radio,
        oer_chips_esp32s31_hal::system::root::ProtocolPartitions {
            wifi,
            wifi_coex,
            phy: right,
        },
    )
    .unwrap_or_else(|_| panic!("an idle split reunites"));
    let (_radio, partitions) = hardware
        .split::<crate::test_clocks::TestClockBackend>(domain_place())
        .unwrap_or_else(|_| panic!("identity capacity"));
    let _place = PhyPlace::new(partitions.phy, &mut memory);
}
