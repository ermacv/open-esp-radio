use std::vec::Vec;

use oer_bluetooth_radio::{
    ConnectionAllowances, EventResult, LeInstant, RadioDuration, RadioTiming, TestChannel, TestPhy,
    TestReport, TxPower,
};

use super::{
    DTM_MAX_PAYLOAD, DtmEvent, DtmPayloadPattern, DtmSession, DtmStartError, DtmStop, DtmTest,
};

const TIMING: RadioTiming = RadioTiming {
    preparation_lead: RadioDuration::from_micros(107),
    admission_guard: RadioDuration::from_micros(40),
    connection: ConnectionAllowances {
        local_sleep_clock_ppm: 500,
        widening_jitter: RadioDuration::from_micros(63),
        receive_guard: RadioDuration::from_micros(10),
        receive_tail: RadioDuration::from_micros(2),
        boundary_guard: oer_bluetooth_radio::NonZeroRadioDuration::from_micros(
            core::num::NonZeroU64::MIN,
        ),
        first_event_guard: RadioDuration::from_micros(16),
        event_length: RadioDuration::from_micros(5047),
        first_event_length: RadioDuration::from_micros(5155),
    },
};

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

fn transmit() -> DtmTest {
    DtmTest::Transmit {
        channel: TestChannel::new(0).unwrap(),
        phy: TestPhy::Le1M,
        pattern: DtmPayloadPattern::Prbs9,
        length: 37,
    }
}

#[test]
fn prbs_patterns_match_the_vendor_table_fingerprints() {
    let mut payload = [0; 255];
    DtmPayloadPattern::Prbs9.fill(&mut payload);
    assert_eq!(
        payload[..16],
        [
            0xff, 0xc1, 0xfb, 0xe8, 0x4c, 0x90, 0x72, 0x8b, 0xe7, 0xb3, 0x51, 0x89, 0x63, 0xab,
            0x23, 0x23
        ]
    );
    assert_eq!(fnv1a64(&payload), 0x94db_648c_b178_dce3);
    DtmPayloadPattern::Prbs15.fill(&mut payload);
    assert_eq!(fnv1a64(&payload), 0x4655_41b8_492c_b9ba);
    for selector in 0..8 {
        let pattern = DtmPayloadPattern::from_hci_selector(selector).unwrap();
        assert_eq!(pattern.payload_type(), selector);
    }
    assert_eq!(DtmPayloadPattern::from_hci_selector(8), None);
}

#[test]
fn transmitter_packets_stay_on_the_interval_grid() {
    let mut session = DtmSession::new(TxPower::from_dbm(0));
    session.start(transmit()).unwrap();
    assert_eq!(session.start(transmit()), Err(DtmStartError::Active));
    let mut payload = [0; DTM_MAX_PAYLOAD];
    let mut anchors = Vec::new();
    let mut now = 1_000;
    for _ in 0..3 {
        let Some(DtmEvent::Transmit(test)) = session
            .next_request(LeInstant::from_micros(now), TIMING, &mut payload)
            .unwrap()
        else {
            panic!("a transmitter plans its next packet")
        };
        assert_eq!(test.window.duration(), RadioDuration::from_micros(376));
        assert_eq!(test.payload.len(), 37);
        assert_eq!(test.payload[..2], [0xff, 0xc1]);
        anchors.push(test.window.start().as_micros());
        let end = test.window.end().as_micros();
        let mut other = [0; DTM_MAX_PAYLOAD];
        assert!(
            session
                .next_request(LeInstant::from_micros(now), TIMING, &mut other)
                .unwrap()
                .is_none()
        );
        session.ended(EventResult::Executed { anchor: None });
        // The next plan happens after the packet: late grid points are skipped.
        now = end + 900;
    }
    // 376 us of air make I(L) = 625 us.
    assert_eq!(anchors[0], 1_000 + 107 + 40 + 500);
    assert_eq!((anchors[1] - anchors[0]) % 625, 0);
    assert_eq!((anchors[2] - anchors[1]) % 625, 0);
    assert!(anchors[1] > anchors[0] + 625);
    assert_eq!(session.end(), DtmStop::Stopped { received: 0 });
    assert_eq!(session.counters.executed, 3);
}

#[test]
fn a_later_le_2m_packet_takes_the_next_admitted_slot() {
    let mut session = DtmSession::new(TxPower::from_dbm(0));
    session
        .start(DtmTest::Transmit {
            channel: TestChannel::new(0).unwrap(),
            phy: TestPhy::Le2M,
            pattern: DtmPayloadPattern::Prbs9,
            length: 37,
        })
        .unwrap();
    let mut payload = [0; DTM_MAX_PAYLOAD];
    let Some(DtmEvent::Transmit(first)) = session
        .next_request(LeInstant::from_micros(1_000), TIMING, &mut payload)
        .unwrap()
    else {
        panic!("the first packet")
    };
    // 192 us of LE 2M air make I(L) = 625 us.
    assert_eq!(first.window.duration(), RadioDuration::from_micros(192));
    let (start, end) = (
        first.window.start().as_micros(),
        first.window.end().as_micros(),
    );
    session.ended(EventResult::Executed { anchor: None });
    // Planned 150 us after the packet, the next slot is still reachable
    // (150 + 107 + 40 + 100 = 397 us < 433 us): the planning slack of the
    // first packet does not push it one interval later.
    let Some(DtmEvent::Transmit(second)) = session
        .next_request(LeInstant::from_micros(end + 150), TIMING, &mut payload)
        .unwrap()
    else {
        panic!("the second packet")
    };
    assert_eq!(second.window.start().as_micros(), start + 625);
    session.ended(EventResult::Executed { anchor: None });
    // Planned too late for the next slot, the packet takes the one after.
    let Some(DtmEvent::Transmit(third)) = session
        .next_request(
            LeInstant::from_micros(start + 625 + 192 + 400),
            TIMING,
            &mut payload,
        )
        .unwrap()
    else {
        panic!("the third packet")
    };
    assert_eq!(third.window.start().as_micros(), start + 3 * 625);
}

#[test]
fn a_receiver_counts_packets_and_drains_before_it_stops() {
    let mut session = DtmSession::new(TxPower::from_dbm(0));
    session
        .start(DtmTest::Receive {
            channel: TestChannel::new(0).unwrap(),
            phy: TestPhy::Le1M,
        })
        .unwrap();
    let mut payload = [0; DTM_MAX_PAYLOAD];
    let mut recurring = Vec::new();
    for report in [
        TestReport::Received { rssi_dbm: -50 },
        TestReport::Nothing,
        TestReport::Received { rssi_dbm: -52 },
    ] {
        let Some(DtmEvent::Receive(test)) = session
            .next_request(LeInstant::from_micros(0), TIMING, &mut payload)
            .unwrap()
        else {
            panic!("a receiver plans its next window")
        };
        recurring.push(test.recurring);
        session.report(report);
        session.ended(EventResult::Executed { anchor: None });
    }
    assert_eq!(recurring, [false, true, true]);
    let Some(DtmEvent::Receive(_)) = session
        .next_request(LeInstant::from_micros(0), TIMING, &mut payload)
        .unwrap()
    else {
        panic!("the receiver keeps listening")
    };
    assert_eq!(session.end(), DtmStop::Draining);
    assert_eq!(session.drained(), None);
    assert!(
        session
            .next_request(LeInstant::from_micros(0), TIMING, &mut payload)
            .unwrap()
            .is_none()
    );
    session.ended(EventResult::NotExecuted);
    assert_eq!(session.drained(), Some(2));
    assert!(!session.is_active());
    assert_eq!(session.counters().empty, 1);
}

#[test]
fn coded_transmitters_are_refused_and_refusals_replan() {
    let mut session = DtmSession::new(TxPower::from_dbm(0));
    assert_eq!(
        session.start(DtmTest::Transmit {
            channel: TestChannel::new(0).unwrap(),
            phy: TestPhy::LeCodedS8,
            pattern: DtmPayloadPattern::Prbs9,
            length: 37,
        }),
        Err(DtmStartError::UnsupportedPhy)
    );
    session.start(transmit()).unwrap();
    let mut payload = [0; DTM_MAX_PAYLOAD];
    assert!(
        session
            .next_request(LeInstant::from_micros(0), TIMING, &mut payload)
            .unwrap()
            .is_some()
    );
    session.refused();
    assert!(
        session
            .next_request(LeInstant::from_micros(0), TIMING, &mut payload)
            .unwrap()
            .is_some()
    );
}

#[test]
fn planning_failure_retains_the_test_history_and_payload() {
    use super::{DtmCalculation, DtmEpochExhausted};
    let mut session = DtmSession::new(TxPower::from_dbm(0));
    session.start(transmit()).unwrap();
    let last = oer_bluetooth_radio::LeWindow::new(
        LeInstant::from_micros(u64::MAX - 400),
        RadioDuration::from_micros(376),
    )
    .unwrap();
    session.last = Some(last);
    let counters = session.counters();
    let mut payload = [0xa5; DTM_MAX_PAYLOAD];
    assert!(matches!(
        session.next_request(LeInstant::from_micros(0), TIMING, &mut payload),
        Err(DtmEpochExhausted {
            calculation: DtmCalculation::NextAnchor,
        })
    ));
    assert_eq!(payload, [0xa5; DTM_MAX_PAYLOAD]);
    assert_eq!(session.last, Some(last));
    assert!(!session.in_progress());
    assert_eq!(session.counters(), counters);
    assert!(session.is_active());
    assert_eq!(session.end(), DtmStop::Stopped { received: 0 });
    assert!(matches!(
        session.next_request(LeInstant::from_micros(u64::MAX), TIMING, &mut payload),
        Ok(None)
    ));
}

#[test]
fn a_recurring_packet_near_epoch_end_needs_only_recurring_slack() {
    let mut session = DtmSession::new(TxPower::from_dbm(0));
    session.start(transmit()).unwrap();
    session.last = Some(
        oer_bluetooth_radio::LeWindow::new(
            LeInstant::from_micros(u64::MAX - 1_001),
            RadioDuration::from_micros(376),
        )
        .unwrap(),
    );
    let mut payload = [0; DTM_MAX_PAYLOAD];
    let Some(DtmEvent::Transmit(test)) = session
        .next_request(LeInstant::from_micros(u64::MAX - 800), TIMING, &mut payload)
        .unwrap()
    else {
        panic!("the final grid point fits")
    };
    assert_eq!(test.window.end(), LeInstant::from_micros(u64::MAX));
}

#[test]
fn failed_reservation_changes_neither_receiver_recurrence_nor_history() {
    let mut session = DtmSession::new(TxPower::from_dbm(0));
    session
        .start(DtmTest::Receive {
            channel: TestChannel::new(0).unwrap(),
            phy: TestPhy::Le1M,
        })
        .unwrap();
    let mut payload = [0x55; DTM_MAX_PAYLOAD];
    let mut timing = TIMING;
    timing.preparation_lead = RadioDuration::from_micros(u64::MAX);
    assert!(
        session
            .next_request(LeInstant::from_micros(0), timing, &mut payload)
            .is_err()
    );
    assert!(!session.recurring);
    assert_eq!(session.last, None);
    assert!(!session.in_progress());
    assert_eq!(payload, [0x55; DTM_MAX_PAYLOAD]);
}

#[test]
fn uncertain_events_are_counted_apart_and_the_next_test_event_follows() {
    for result in [EventResult::Aborted, EventResult::Indeterminate] {
        let mut session = DtmSession::new(TxPower::from_dbm(0));
        session
            .start(DtmTest::Receive {
                channel: TestChannel::new(19).unwrap(),
                phy: TestPhy::Le1M,
            })
            .unwrap();
        let mut payload = [0; 255];
        let DtmEvent::Receive(_) = session
            .next_request(LeInstant::from_micros(1_000), TIMING, &mut payload)
            .unwrap()
            .unwrap()
        else {
            panic!("receiver event")
        };
        session.ended(result);
        assert_eq!(
            session.counters.aborted,
            u32::from(result == EventResult::Aborted)
        );
        assert_eq!(
            session.counters.indeterminate,
            u32::from(result == EventResult::Indeterminate)
        );
        assert_eq!(session.counters.executed, 0);
        assert_eq!(session.counters.not_executed, 0);
        assert!(
            session
                .next_request(LeInstant::from_micros(2_000), TIMING, &mut payload)
                .unwrap()
                .is_some()
        );
    }
}

/// Without an event in progress, a report or an end counts nothing and the
/// session plans as before.
#[test]
fn reports_without_an_event_in_progress_count_nothing() {
    let mut session = DtmSession::new(TxPower::from_dbm(0));
    session
        .start(DtmTest::Receive {
            channel: TestChannel::new(0).unwrap(),
            phy: TestPhy::Le1M,
        })
        .unwrap();
    session.report(TestReport::Received { rssi_dbm: -40 });
    session.ended(EventResult::Executed { anchor: None });
    assert_eq!(session.counters(), super::DtmCounters::default());
    let mut payload = [0; DTM_MAX_PAYLOAD];
    assert!(
        session
            .next_request(LeInstant::from_micros(0), TIMING, &mut payload)
            .unwrap()
            .is_some()
    );
    assert!(session.in_progress());
}
