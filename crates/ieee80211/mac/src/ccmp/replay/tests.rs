//! The replay check of an association's keys.

use super::*;
use crate::ccmp::CcmpPacketNumber;

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
    epoch.install_group(group(2), [0; 8]).unwrap();
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
    epoch.install_group(group(1), [0; 8]).unwrap();
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
    epoch.install_group(group(1), 5u64.to_le_bytes()).unwrap();
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
