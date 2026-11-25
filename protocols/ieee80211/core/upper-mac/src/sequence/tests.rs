use super::*;
use crate::seq;

#[test]
fn a_counter_wraps_from_4095_to_zero_and_keeps_counting() {
    let mut sequence = SequenceCounter::new(seq(0x0ffe));
    assert_eq!(sequence.take(), seq(0x0ffe));
    assert_eq!(sequence.take(), seq(0x0fff));
    assert_eq!(sequence.take(), seq(0x0000));
    assert_eq!(sequence.peek(), seq(0x0001));
}

#[test]
fn arithmetic_wraps_in_the_sequence_space() {
    assert_eq!(SequenceNumber::MAX.next(), SequenceNumber::ZERO);
    assert_eq!(SequenceNumber::ZERO.wrapping_sub(1), SequenceNumber::MAX);
    assert_eq!(
        SequenceNumber::new(4090).unwrap().wrapping_add(10),
        SequenceNumber::new(4).unwrap()
    );
    assert_eq!(
        SequenceNumber::new(4090)
            .unwrap()
            .forward_distance(SequenceNumber::new(4).unwrap()),
        10
    );
    assert_eq!(
        SequenceNumber::new(4)
            .unwrap()
            .forward_distance(SequenceNumber::new(4090).unwrap()),
        4086
    );
}

#[test]
fn precedence_uses_the_half_space_horizon() {
    let start = SequenceNumber::new(4000).unwrap();
    assert!(start.precedes(start.wrapping_add(1)));
    assert!(start.precedes(start.wrapping_add(SequenceNumber::HALF_SPACE - 1)));
    assert!(!start.precedes(start.wrapping_add(SequenceNumber::HALF_SPACE)));
    assert!(!start.precedes(start));
    assert!(!start.wrapping_add(1).precedes(start));
}
