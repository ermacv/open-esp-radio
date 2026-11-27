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

/// The one right of a fresh split, with what the reunion takes back.
fn split() -> (
    oer_chips_esp32s31_hal::radio::domain::RadioDomain<crate::test_clocks::TestClockBackend>,
    oer_chips_esp32s31_hal::system::root::ProtocolPartitions,
) {
    oer_chips_esp32s31_hal::system::root::RadioHardware::for_host_test()
        .split::<crate::test_clocks::TestClockBackend>(
            oer_chips_esp32s31_hal::radio::domain::transaction::PhyIdentity::for_host_test(),
        )
        .unwrap_or_else(|_| panic!("identity capacity"))
}

/// A place filled with a new state, as a registration that completed leaves
/// it.
fn registered<'s>(
    right: PhyStateRight,
    memory: &'s mut MaybeUninit<PhyState>,
) -> RegisteredPhy<'s> {
    RegisteredPhy::for_test(
        PhyPlace::new(right, memory).fill(PhyState::new(PhyConfig::esp32s31_default())),
    )
}

#[test]
fn the_last_pass_outlives_rf_closed() {
    let (_radio, partitions) = split();
    let mut memory = MaybeUninit::uninit();
    let mut phy = registered(partitions.phy, &mut memory);
    phy.state_mut().mark_tracked(at(7));
    let closed = RfOpen::opened().close();
    let _open = closed.open();
    assert_eq!(phy.state().tracked_at(), Some(at(7)));
}

#[test]
fn the_calibration_cache_is_the_live_state_s() {
    let (_radio, partitions) = split();
    let mut memory = MaybeUninit::uninit();
    let mut phy = registered(partitions.phy, &mut memory);
    let identity = crate::PhyCalibrationIdentity {
        rf_cal_version: 7,
        base_mac_address: [1, 2, 3, 4, 5, 6],
        mac_extension: 0x0908,
    };
    let before = *phy.calibration_cache(identity).snapshot();
    // A later write to the registered state is in the next capture: the
    // reading is of the state as it is, not of the registration's end.
    phy.state_mut().apply_rc_calibration(45);
    let after = phy.calibration_cache(identity);
    assert!(after.matches(identity));
    assert_ne!(*after.snapshot(), before);
    assert_eq!(
        after.snapshot(),
        phy.state().calibration_cache(identity).snapshot()
    );
}

#[test]
fn a_failed_operation_s_right_empties_the_place_for_the_next_registration() {
    let (_radio, partitions) = split();
    let mut memory = MaybeUninit::uninit();
    let mut phy = registered(partitions.phy, &mut memory);
    phy.state_mut().mark_tracked(at(3));
    // A failed close consumed the proof: the right to empty the place is
    // what is left.
    let vacate = RfOpen::opened().fail();
    let place = phy.vacate(vacate);
    let phy = RegisteredPhy::for_test(place.fill(PhyState::new(PhyConfig::esp32s31_default())));
    assert_eq!(
        phy.state().tracked_at(),
        None,
        "the next registration's state is new"
    );
}

#[test]
fn a_failed_wake_leaves_the_same_right() {
    let (_radio, partitions) = split();
    let mut memory = MaybeUninit::uninit();
    let phy = registered(partitions.phy, &mut memory);
    let vacate = RfOpen::opened().close().fail();
    let _place = phy.vacate(vacate);
}

#[test]
fn a_registration_that_does_not_complete_gives_the_place_back_empty() {
    let (_radio, partitions) = split();
    let mut memory = MaybeUninit::uninit();
    let mut filled = PhyPlace::new(partitions.phy, &mut memory)
        .fill(PhyState::new(PhyConfig::esp32s31_default()));
    filled.state.mark_tracked(at(9));
    let place = filled.empty();
    let phy = RegisteredPhy::for_test(place.fill(PhyState::new(PhyConfig::esp32s31_default())));
    assert_eq!(phy.state().tracked_at(), None);
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
    let place = PhyPlace::new(right, &mut memory);
    let right = place.into_right();
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
        .split::<crate::test_clocks::TestClockBackend>(
            oer_chips_esp32s31_hal::radio::domain::transaction::PhyIdentity::for_host_test(),
        )
        .unwrap_or_else(|_| panic!("identity capacity"));
    let _place = PhyPlace::new(partitions.phy, &mut memory);
}

impl<'s> RegisteredPhy<'s> {
    /// A registered handle over a model state, as a completed registration
    /// leaves it.
    fn for_test(filled: Filled<'s>) -> Self {
        let Filled { state, right } = filled;
        Self { state, right }
    }
}
