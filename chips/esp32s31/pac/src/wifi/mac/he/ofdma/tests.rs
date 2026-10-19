use super::*;

#[test]
fn traffic_identifier_limits_expose_semantic_membership() {
    let tids: [MacHeTid; MacHeTid::COUNT] =
        core::array::from_fn(|value| MacHeTid::new(value as u8).unwrap());

    assert!(MacHeTbTidLimit::One.contains(tids[0]));
    assert!(!MacHeTbTidLimit::One.contains(tids[1]));
    assert!(MacHeTbTidLimit::Two.contains(tids[0]));
    assert!(MacHeTbTidLimit::Two.contains(tids[7]));
    assert!(!MacHeTbTidLimit::Two.contains(tids[5]));
    assert!(MacHeTbTidLimit::Three.contains(tids[0]));
    assert!(MacHeTbTidLimit::Three.contains(tids[5]));
    assert!(MacHeTbTidLimit::Three.contains(tids[7]));
    assert!(MacHeTbTidLimit::Four.contains(tids[1]));
    assert_eq!(MacHeTid::new(8), None);
    assert_eq!(MacHeTid::new(u8::MAX), None);
    assert_eq!(MacHeTbTidLimit::default(), MacHeTbTidLimit::Three);
}

#[test]
fn beamforming_average_snr_retains_the_blob_quarter_db_transform() {
    assert_eq!(
        MacBeamformingAverageSnr::from_raw_code(0),
        MacBeamformingAverageSnr {
            raw_code: 0,
            quarter_db: 88,
            is_lower_bound: false,
        }
    );
    assert_eq!(
        MacBeamformingAverageSnr::from_raw_code(0x7f),
        MacBeamformingAverageSnr {
            raw_code: 0x7f,
            quarter_db: 215,
            is_lower_bound: true,
        }
    );
}

#[test]
fn default_trigger_link_ranges_partition_all_120_entries() {
    let expected = [(32, 32), (64, 32), (0, 32), (96, 24)];
    let mut seen = [false; 120];
    for (queue, (first, capacity)) in expected.into_iter().enumerate() {
        let reservation =
            MacHeTbLinkReservation::for_queue(MacHeTbTidLimit::Three, queue as u8, capacity)
                .unwrap();
        assert_eq!(reservation.first(), first);
        assert_eq!(reservation.count(), capacity);
        for position in 0..capacity {
            let index = reservation.index(position).unwrap();
            assert!(!seen[usize::from(index)]);
            seen[usize::from(index)] = true;
            assert_eq!(
                reservation.next(position).unwrap(),
                if position + 1 == capacity {
                    MPDU_LENGTH_LINK_END
                } else {
                    index + 1
                }
            );
        }
    }
    assert!(seen.into_iter().all(core::convert::identity));
}

#[test]
fn single_tid_trigger_link_ranges_match_the_blob_tables() {
    let expected = [(64, 18), (82, 18), (0, 64), (100, 18)];
    for (queue, (first, capacity)) in expected.into_iter().enumerate() {
        let reservation =
            MacHeTbLinkReservation::for_queue(MacHeTbTidLimit::One, queue as u8, capacity).unwrap();
        assert_eq!(reservation.first(), first);
        assert_eq!(reservation.count(), capacity);
        assert!(
            MacHeTbLinkReservation::for_queue(MacHeTbTidLimit::One, queue as u8, capacity + 1)
                .is_none()
        );
    }
}
