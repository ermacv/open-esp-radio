use super::*;

#[test]
fn construction_is_limited_to_twelve_bits() {
    assert_eq!(SequenceNumber::new(0x0fff), Some(SequenceNumber::MAX));
    assert_eq!(SequenceNumber::new(0x1000), None);
    assert_eq!(SequenceNumber::from_low_bits(0xf123).get(), 0x0123);
}

#[test]
fn sequence_control_carries_the_number_above_the_fragment() {
    let sequence = SequenceNumber::new(0x0abc).unwrap();
    assert_eq!(sequence.sequence_control(), 0xabc0);
    assert_eq!(sequence.sequence_control_with_fragment(0x13), 0xabc3);
    assert_eq!(SequenceNumber::from_sequence_control(0xabc7), sequence);
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
