use super::*;
use crate::ConnectionAllowances;
const TIMING: RadioTiming = RadioTiming {
    preparation_lead: RadioDuration::from_micros(107),
    admission_guard: RadioDuration::from_micros(40),
    connection: ConnectionAllowances {
        local_sleep_clock_ppm: 500,
        widening_jitter: RadioDuration::from_micros(63),
        receive_guard: RadioDuration::from_micros(10),
        receive_tail: RadioDuration::from_micros(2),
        boundary_guard: crate::NonZeroRadioDuration::from_micros(core::num::NonZeroU64::MIN),
        first_event_guard: RadioDuration::from_micros(16),
        event_length: RadioDuration::from_micros(5047),
        first_event_length: RadioDuration::from_micros(5155),
    },
};

fn transmit() -> DtmTest {
    DtmTest::Transmit {
        channel: TestChannel::new(0).unwrap(),
        phy: TestPhy::Le1M,
        pattern: DtmPayloadPattern::Prbs9,
        length: 37,
    }
}

#[test]
fn detached_coded_tx_window_is_refused_without_a_session_or_payload() {
    let test = DtmTest::Transmit {
        channel: TestChannel::new(19).expect("test channel"),
        phy: TestPhy::LeCodedS8,
        pattern: DtmPayloadPattern::Prbs9,
        length: 37,
    };
    assert_eq!(
        next_window(test, LeInstant::from_micros(0), RadioTiming::MINIMAL, None),
        Err(DtmWindowError::UnsupportedPhy)
    );
}
#[test]
fn slot_rounding_handles_maximum_lateness_and_refuses_zero_division() {
    assert_eq!(super::ceil_div(u64::MAX, 625), Some(u64::MAX / 625 + 1));
    assert_eq!(super::ceil_div(u64::MAX, 1), Some(u64::MAX));
    assert_eq!(super::ceil_div(0, 625), Some(0));
    assert_eq!(super::ceil_div(1, 0), None);
}

#[test]
fn transmitter_windows_preserve_the_grid_and_skip_unreachable_slots() {
    let first = next_window(transmit(), LeInstant::from_micros(1_000), TIMING, None).unwrap();
    assert_eq!(first.start().as_micros(), 1_000 + 107 + 40 + 500);
    assert_eq!(first.duration().as_micros(), 376);
    let second = next_window(
        transmit(),
        LeInstant::from_micros(first.end().as_micros() + 900),
        TIMING,
        Some(first),
    )
    .unwrap();
    assert_eq!(
        (second.start().as_micros() - first.start().as_micros()) % 625,
        0
    );
    assert!(second.start().as_micros() > first.start().as_micros() + 625);
    let mut test = transmit();
    if let DtmTest::Transmit { phy, .. } = &mut test {
        *phy = TestPhy::Le2M;
    }
    let first = next_window(test, LeInstant::from_micros(1_000), TIMING, None).unwrap();
    assert_eq!(first.duration().as_micros(), 192);
    let second = next_window(
        test,
        LeInstant::from_micros(first.end().as_micros() + 150),
        TIMING,
        Some(first),
    )
    .unwrap();
    assert_eq!(second.start().as_micros(), first.start().as_micros() + 625);
    let third = next_window(
        test,
        LeInstant::from_micros(second.end().as_micros() + 400),
        TIMING,
        Some(second),
    )
    .unwrap();
    assert_eq!(
        third.start().as_micros(),
        first.start().as_micros() + 3 * 625
    );
}

#[test]
fn every_transmit_length_preserves_specified_airtime_and_interval() {
    for phy in [TestPhy::Le1M, TestPhy::Le2M] {
        for length in 0..=u8::MAX {
            let test = DtmTest::Transmit {
                channel: TestChannel::new(0).unwrap(),
                phy,
                pattern: DtmPayloadPattern::Prbs9,
                length,
            };
            let first = next_window(test, LeInstant::from_micros(0), TIMING, None).unwrap();
            let air = if phy == TestPhy::Le1M {
                (10 + u64::from(length)) * 8
            } else {
                (11 + u64::from(length)) * 4
            };
            assert_eq!(first.duration().as_micros(), air);
            let second = next_window(test, LeInstant::from_micros(0), TIMING, Some(first)).unwrap();
            assert_eq!(
                second.start().as_micros() - first.start().as_micros(),
                (air + 249).div_ceil(625) * 625
            );
            assert!(TIMING.reservation(first).is_ok());
        }
    }
}

#[test]
fn receiver_windows_are_contiguous_until_a_later_admission() {
    let test = DtmTest::Receive {
        channel: TestChannel::new(19).unwrap(),
        phy: TestPhy::Le1M,
    };
    let first = next_window(test, LeInstant::from_micros(0), TIMING, None).unwrap();
    let second = next_window(test, LeInstant::from_micros(0), TIMING, Some(first)).unwrap();
    assert_eq!(first.duration().as_micros(), 5_000);
    assert_eq!(second.start(), first.end());
    let late = next_window(test, second.end(), TIMING, Some(second)).unwrap();
    assert_eq!(
        late.start().as_micros(),
        second.end().as_micros() + 107 + 40 + 500
    );
}

#[test]
fn final_recurring_packet_needs_only_recurring_slack() {
    let last = LeWindow::new(
        LeInstant::from_micros(u64::MAX - 1_001),
        RadioDuration::from_micros(376),
    )
    .unwrap();
    let final_window = next_window(
        transmit(),
        LeInstant::from_micros(u64::MAX - 800),
        TIMING,
        Some(last),
    )
    .unwrap();
    assert_eq!(final_window.end(), LeInstant::from_micros(u64::MAX));
    assert_eq!(
        next_window(
            transmit(),
            LeInstant::from_micros(u64::MAX),
            TIMING,
            Some(last)
        ),
        Err(DtmWindowError::OutsideEpoch(DtmEpochExhausted {
            calculation: DtmCalculation::Admission
        }))
    );
    let last = LeWindow::new(
        LeInstant::from_micros(u64::MAX - 400),
        RadioDuration::from_micros(376),
    )
    .unwrap();
    assert_eq!(
        next_window(transmit(), LeInstant::from_micros(0), TIMING, Some(last)),
        Err(DtmWindowError::OutsideEpoch(DtmEpochExhausted {
            calculation: DtmCalculation::NextAnchor
        }))
    );
}
