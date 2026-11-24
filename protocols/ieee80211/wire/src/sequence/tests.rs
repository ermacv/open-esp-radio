use super::*;

#[test]
fn construction_is_limited_to_twelve_bits() {
    assert_eq!(SequenceNumber::new(0x0fff), Some(SequenceNumber::MAX));
    assert_eq!(SequenceNumber::new(0x1000), None);
}

#[test]
fn sequence_control_carries_the_number_above_the_fragment() {
    let sequence = SequenceNumber::new(0x0abc).unwrap();
    assert_eq!(
        sequence.sequence_control().to_bytes(),
        0xabc0_u16.to_le_bytes()
    );
    assert_eq!(sequence.sequence_control().fragment(), 0);
    assert_eq!(
        SequenceNumber::from_sequence_control(SequenceControl::from_bytes(
            0xabc7_u16.to_le_bytes()
        )),
        sequence
    );
}

/// The sum and difference wrap across 4095 and 0 both ways, and the
/// distance counts forward across the wrap.
#[test]
fn the_arithmetic_wraps_modulo_4096() {
    assert_eq!(SequenceNumber::MAX.wrapping_add(1), SequenceNumber::ZERO);
    assert_eq!(SequenceNumber::ZERO.wrapping_sub(1), SequenceNumber::MAX);
    assert_eq!(seq(4094).wrapping_add(3), seq(1));
    assert_eq!(seq(1).wrapping_sub(3), seq(4094));
    assert_eq!(seq(5).wrapping_add(4096), seq(5));
    assert_eq!(seq(5).wrapping_sub(4096), seq(5));
    assert_eq!(seq(5).wrapping_add(u16::MAX), seq(4));
    assert_eq!(seq(4094).forward_distance(seq(1)), 3);
    assert_eq!(seq(1).forward_distance(seq(4094)), 4093);
    assert_eq!(seq(7).forward_distance(seq(7)), 0);
    assert_eq!(
        SequenceNumber::ZERO.forward_distance(SequenceNumber::MAX),
        4095
    );
}
