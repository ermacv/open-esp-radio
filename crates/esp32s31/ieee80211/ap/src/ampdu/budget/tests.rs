use oer_ieee80211_mac::ap::ApProtectedDataFrame;

use super::*;

#[test]
fn prospective_ethernet_admission_matches_encoded_mpdus_and_preserves_rejected_budget() {
    let mut budget = ApAmpduBudget {
        length: HtAmpduLengthAccumulator::new(4, 3200).unwrap(),
    };
    let mut metadata_budget = ApAmpduBudget {
        length: HtAmpduLengthAccumulator::new(4, 3200).unwrap(),
    };
    let mut encoded_lengths = HtAmpduLengthAccumulator::new(4, 3200).unwrap();
    let mut ethernet = [0_u8; 1514];
    let peer = [2, 0, 0, 0, 0, 1];
    ethernet[..6].copy_from_slice(&peer);
    ethernet[6..12].copy_from_slice(&[2, 0, 0, 0, 0, 2]);
    ethernet[12..14].copy_from_slice(&0x0800_u16.to_be_bytes());
    for (index, ethernet_length) in [1514, 1513, 1514, 14].into_iter().enumerate() {
        let mut output = [0_u8; 2048];
        let encoded_length = ApProtectedDataFrame {
            access_point: [2, 0, 0, 0, 0, 3],
            peer,
            sequence_number: SequenceNumber::new(index as u16).unwrap(),
            user_priority: 0,
            peer_qos: true,
            more_data: false,
            ccmp_header: [0; 8],
            ethernet: &ethernet[..ethernet_length],
        }
        .encode(&mut output)
        .unwrap();
        let fits = encoded_lengths
            .push((encoded_length + 8 + 4) as u32, 0)
            .is_ok();
        assert_eq!(
            budget.admit_ethernet(&ethernet[..ethernet_length]).unwrap(),
            fits
        );
        assert_eq!(budget.length.finish(), encoded_lengths.finish());
        assert_eq!(
            metadata_budget.admit_ethernet_len(ethernet_length).unwrap(),
            fits
        );
        assert_eq!(metadata_budget.length.finish(), encoded_lengths.finish());
        assert_eq!(fits, index != 2);
    }
}

#[test]
fn malformed_and_oversized_frames_do_not_consume_the_prefix_budget() {
    let mut budget = ApAmpduBudget {
        length: HtAmpduLengthAccumulator::new(2, u16::MAX).unwrap(),
    };
    assert_eq!(budget.admit_ethernet(&[0; 13]), Err(ApAmpduError::Geometry));
    assert_eq!(
        budget.admit_ethernet(&[0; 16384]),
        Err(ApAmpduError::Geometry)
    );
    assert_eq!(
        budget.admit_ethernet_len(usize::MAX),
        Err(ApAmpduError::Geometry)
    );
    assert!(budget.admit_ethernet(&[0; 60]).unwrap());
    assert!(budget.admit_ethernet(&[0; 60]).unwrap());
    assert!(!budget.admit_ethernet(&[0; 60]).unwrap());
}

#[test]
fn exchange_cap_intersects_geometry_and_rejection_preserves_the_prefix() {
    let mut budget = ApAmpduBudget {
        length: HtAmpduLengthAccumulator::new(4, 3200).unwrap(),
    };
    assert!(budget.admit_ethernet_len(1514).unwrap());
    let prefix = budget.length.finish().unwrap();
    budget.cap_bytes(prefix.bytes).unwrap();
    assert!(!budget.admit_ethernet_len(60).unwrap());
    assert_eq!(budget.length.finish().unwrap(), prefix);
    // A caller cannot reopen a closed burst by supplying a larger later cap.
    budget.cap_bytes(u16::MAX).unwrap();
    assert!(!budget.admit_ethernet_len(60).unwrap());
    assert_eq!(budget.length.finish().unwrap(), prefix);
}

#[test]
fn shrinking_below_prepared_work_fails_without_changing_admission() {
    let mut budget = ApAmpduBudget {
        length: HtAmpduLengthAccumulator::new(4, 3200).unwrap(),
    };
    assert!(budget.admit_ethernet_len(1514).unwrap());
    let prefix = budget.length.finish().unwrap();
    assert!(budget.cap_bytes(0).is_err());
    assert_eq!(budget.length.finish().unwrap(), prefix);
    assert!(budget.admit_ethernet_len(1514).unwrap());
}

#[test]
fn a_budget_below_one_ppdu_refuses_the_pair_without_consuming_geometry() {
    let mut budget = ApAmpduBudget {
        length: HtAmpduLengthAccumulator::new(4, 3200).unwrap(),
    };
    budget.cap_bytes(0).unwrap();
    assert!(!budget.admit_ethernet_len(60).unwrap());
    assert_eq!(budget.length.finish(), Err(HtAmpduLengthError::Empty));
}
