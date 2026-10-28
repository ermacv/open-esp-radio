use super::*;

#[test]
fn probing_ie_encodes_ready_metrics_and_refuses_short_destinations() {
    let mut ie = [0; ENH_ACK_PROBING_IE_CAPACITY];
    assert_eq!(ProbingData::two(200, 111).write_ie(&mut ie), 8);
    assert_eq!(ie, [6, 0, 0x9b, 0xb8, 0xea, 0, 200, 111]);
    let before = ie;
    assert_eq!(ProbingData::two(200, 111).write_ie(&mut ie[..7]), 0);
    assert_eq!(ie, before);
    assert_eq!(ProbingData::one(200).write_ie(&mut ie), 7);
    assert_eq!(&ie[..7], &[5, 0, 0x9b, 0xb8, 0xea, 0, 200]);
    assert_eq!(ProbingData::one(200).bytes(), &[200]);
    let before = ie;
    assert_eq!(ProbingData::one(200).write_ie(&mut ie[..6]), 0);
    assert_eq!(ie, before);
    assert_eq!(ProbingData::default().write_ie(&mut ie), 0);
    assert_eq!(ie, before);
    assert!(ProbingData::default().bytes().is_empty());
}
