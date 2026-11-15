//! The data module's in-place header move.

use super::*;

#[test]
fn a_shorter_header_moves_the_payload_down_to_it_without_overlap() {
    let mut region = std::vec![0_u8; 200];
    for (index, octet) in region[MSDU_PAYLOAD_OFFSET..MSDU_PAYLOAD_OFFSET + 50]
        .iter_mut()
        .enumerate()
    {
        *octet = index as u8;
    }
    let end = MSDU_PAYLOAD_OFFSET + 50;
    for to in [0, 1, MSDU_PAYLOAD_OFFSET - 1] {
        let mut moved = region.clone();
        move_down(&mut moved, to, end).unwrap();
        assert_eq!(&moved[to..to + 50], &region[MSDU_PAYLOAD_OFFSET..end]);
    }
    assert!(move_down(&mut region, 0, 201).is_none());
}
