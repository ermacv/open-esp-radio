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
