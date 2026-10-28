//! The replay check of an association's keys.

use super::*;
use oer_ieee80211_wire::ccmp::CcmpPacketNumber;

fn header(packet_number: u64, key_id: u8) -> CcmpHeader {
    CcmpHeader::new(
        CcmpPacketNumber::new(packet_number).unwrap(),
        CcmpKeyId::new(key_id).unwrap(),
    )
}

fn group(key_id: u8) -> GroupKeyId {
    GroupKeyId::new(key_id).unwrap()
}

fn epoch() -> CcmpRxReplayEpoch {
    CcmpRxReplayEpoch::new([0; 8], group(1), [0; 8]).unwrap()
}

#[test]
fn a_frame_sent_again_is_refused_in_its_lane_and_other_lanes_go_on() {
    let mut epoch = epoch();
    assert_eq!(
        epoch.check(CcmpRxProtection::Pairwise, Some(0), header(5, 0)),
        Ok(())
    );
    assert!(matches!(
        epoch.check(CcmpRxProtection::Pairwise, Some(0), header(5, 0)),
        Err(CcmpRxReplayError::Replay(CcmpReplayError::Replayed { .. }))
    ));
    // A lower number in its lane is refused too; another TID's and the
    // non-QoS lane's go on.
    assert!(
        epoch
            .check(CcmpRxProtection::Pairwise, Some(0), header(4, 0))
            .is_err()
    );
    assert_eq!(
        epoch.check(CcmpRxProtection::Pairwise, Some(6), header(1, 0)),
        Ok(())
    );
    assert_eq!(
        epoch.check(CcmpRxProtection::Pairwise, None, header(1, 0)),
        Ok(())
    );
}

#[test]
fn the_old_and_the_new_group_key_are_both_taken_until_the_old_is_removed() {
    let mut epoch = epoch();
    assert_eq!(
        epoch.check(CcmpRxProtection::Group, None, header(900, 1)),
        Ok(())
    );
    // The access point's new group key, index 2, beside the old one.
    assert!(
        epoch
            .exchange_group(
                group(2),
                CcmpRxReplayState::from_receive_sequence([0; 8]).unwrap()
            )
            .is_none()
    );
    assert_eq!(
        epoch.check(CcmpRxProtection::Group, None, header(1, 2)),
        Ok(())
    );
    assert_eq!(
        epoch.check(CcmpRxProtection::Group, None, header(901, 1)),
        Ok(())
    );
    // Each key keeps its own frontier.
    assert!(
        epoch
            .check(CcmpRxProtection::Group, None, header(1, 2))
            .is_err()
    );
    assert_eq!(
        epoch.check(CcmpRxProtection::Group, None, header(2, 2)),
        Ok(())
    );
    // The old key removed: its frames are refused, the new key's go on.
    epoch.remove_group(group(1));
    assert_eq!(
        epoch.check(CcmpRxProtection::Group, None, header(902, 1)),
        Err(CcmpRxReplayError::UninstalledGroupKey(
            CcmpKeyId::new(1).unwrap()
        ))
    );
    assert_eq!(
        epoch.check(CcmpRxProtection::Group, None, header(3, 2)),
        Ok(())
    );
}

#[test]
fn cancelled_old_admission_cannot_advance_the_reinstalled_group_key() {
    let mut epoch = epoch();
    {
        let _cancelled = epoch
            .prepare(CcmpRxProtection::Group, None, header(900, 1))
            .unwrap();
    }
    epoch.remove_group(group(1));
    assert!(
        epoch
            .exchange_group(
                group(1),
                CcmpRxReplayState::from_receive_sequence([0; 8]).unwrap()
            )
            .is_none()
    );
    assert_eq!(
        epoch.check(CcmpRxProtection::Group, None, header(1, 1)),
        Ok(())
    );
    assert_eq!(
        epoch.check(CcmpRxProtection::Group, None, header(2, 1)),
        Ok(())
    );
}

#[test]
fn pairwise_admission_advances_only_its_original_association() {
    let mut old = epoch();
    let mut current = epoch();
    old.prepare(CcmpRxProtection::Pairwise, None, header(900, 0))
        .unwrap()
        .commit();
    assert!(
        old.check(CcmpRxProtection::Pairwise, None, header(900, 0))
            .is_err()
    );
    assert_eq!(
        current.check(CcmpRxProtection::Pairwise, None, header(1, 0)),
        Ok(())
    );
    assert_eq!(
        current.check(CcmpRxProtection::Group, None, header(1, 1)),
        Ok(())
    );
}

#[test]
fn an_uninstalled_group_id_zero_is_refused() {
    let mut epoch = epoch();
    assert_eq!(
        epoch.check(CcmpRxProtection::Group, None, header(1, 0)),
        Err(CcmpRxReplayError::UninstalledGroupKey(CcmpKeyId::PAIRWISE))
    );
}

#[test]
fn gtk_zero_and_ptk_zero_have_distinct_receive_frontiers() {
    let zero = group(0);
    let mut epoch = CcmpRxReplayEpoch::new([0; 8], zero, [9, 0, 0, 0, 0, 0, 0, 0]).unwrap();
    assert_eq!(
        epoch.check(CcmpRxProtection::Pairwise, None, header(1, 0)),
        Ok(())
    );
    assert_eq!(
        epoch.check(CcmpRxProtection::Group, None, header(10, 0)),
        Ok(())
    );
    assert!(
        epoch
            .check(CcmpRxProtection::Group, None, header(9, 0))
            .is_err()
    );
    assert_eq!(
        epoch.check(CcmpRxProtection::Pairwise, None, header(2, 0)),
        Ok(())
    );
    epoch.remove_group(zero);
    assert!(
        epoch
            .check(CcmpRxProtection::Group, None, header(11, 0))
            .is_err()
    );
    assert_eq!(
        epoch.check(CcmpRxProtection::Pairwise, None, header(3, 0)),
        Ok(())
    );
}

#[test]
fn new_group_material_in_the_same_id_starts_from_its_key_rsc() {
    let mut epoch = epoch();
    assert_eq!(
        epoch.check(CcmpRxProtection::Group, None, header(900, 1)),
        Ok(())
    );
    assert!(
        epoch
            .exchange_group(
                group(1),
                CcmpRxReplayState::from_receive_sequence(5u64.to_le_bytes()).unwrap()
            )
            .is_some()
    );
    assert!(
        epoch
            .check(CcmpRxProtection::Group, None, header(5, 1))
            .is_err()
    );
    assert_eq!(
        epoch.check(CcmpRxProtection::Group, None, header(6, 1)),
        Ok(())
    );
}

/// The input's first two octets pick the group key and destination; then
/// each record is a TID octet (none above 0x7f) and a CCMP header.
fn check_all(bytes: &[u8]) {
    let Some((&[key_id, rsc], mut records)) = bytes.split_first_chunk::<2>() else {
        return;
    };
    let Some(group_key_id) = GroupKeyId::new(key_id & 3) else {
        return;
    };
    let Ok(mut epoch) = CcmpRxReplayEpoch::new([rsc, 0, 0, 0, 0, 0, 0, 0], group_key_id, [0; 8])
    else {
        return;
    };
    let _ = epoch.has_group(group(key_id >> 2 & 3));
    while let Some((&tid, rest)) = records.split_first() {
        let Some((header, rest)) = rest.split_first_chunk::<8>() else {
            return;
        };
        records = rest;
        let Ok(header) = CcmpHeader::parse(*header) else {
            continue;
        };
        let protection = match tid >> 6 {
            0 => CcmpRxProtection::Pairwise,
            1 => CcmpRxProtection::Group,
            _ => CcmpRxProtection::Other,
        };
        let tid = (tid & 0x20 == 0).then_some(tid & 0x1f);
        match epoch.prepare(protection, tid, header) {
            Ok(candidate) if rsc & 1 == 0 => candidate.commit(),
            Ok(_) | Err(_) => {}
        }
        let _ = epoch.check(protection, tid, header);
    }
}

fn run(count: usize) {
    let mut seed = std::vec![1, 0];
    for (tid, packet_number, key_id) in [(0, 1, 0), (0x47, 2, 1), (0x05, 2, 0), (0x90, 3, 0)] {
        seed.push(tid);
        seed.extend_from_slice(&header(packet_number, key_id).encode());
    }
    oer_ieee80211_wire::fuzz::inputs(&[&seed], count, 96, check_all);
}

#[test]
fn replay_check_never_panics() {
    run(oer_ieee80211_wire::fuzz::QUICK);
}

#[test]
#[ignore = "the full run"]
fn replay_check_never_panics_full() {
    run(oer_ieee80211_wire::fuzz::FULL);
}

#[test]
fn a_new_epoch_installs_exactly_its_requested_group_frontier() {
    for key_id in 0..4 {
        let mut epoch = CcmpRxReplayEpoch::new(
            [3, 0, 0, 0, 0, 0, 0, 0],
            group(key_id),
            [7, 0, 0, 0, 0, 0, 0, 0],
        )
        .unwrap();
        for candidate in 0..4 {
            assert_eq!(epoch.has_group(group(candidate)), candidate == key_id);
        }
        assert!(
            epoch
                .check(CcmpRxProtection::Group, None, header(7, key_id))
                .is_err()
        );
        assert_eq!(
            epoch.check(CcmpRxProtection::Group, None, header(8, key_id)),
            Ok(())
        );
        assert!(
            epoch
                .check(CcmpRxProtection::Pairwise, None, header(3, 0))
                .is_err()
        );
        assert_eq!(
            epoch.check(CcmpRxProtection::Pairwise, None, header(4, 0)),
            Ok(())
        );
    }
}
