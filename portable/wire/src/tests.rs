use crate::{Error, Reader, Writer};

crate::bitfield! {
    enum Kind: u8 { Data = 2, Management = 0 }
}
crate::bitfield! {
    struct Little[2] (little) {
        fragment, with_fragment: u8 = 0..4;
        sequence, with_sequence: u16 = 4..16;
    }
}
crate::bitfield! {
    struct Big[2] (big) {
        prefix, with_prefix: u8 = 0..4;
        value, with_value: u16 = 4..15;
        flag, with_flag: bool = 15..16;
    }
}
crate::bitfield! {
    struct Control[1] (little) {
        kind, with_kind: [Kind] = 2..4;
    }
}
crate::bitfield! {
    struct Full[8] (little) { value, with_value: u64 = 0..64; }
}

#[test]
fn sequence_control_round_trip_and_rejection_preserve_fragment() {
    // SOURCE: IEEE Std 802.11-2020, 9.2.4.4 (Sequence Control field).
    let original = Little::from_bytes([0x5d, 0xa3]);
    assert_eq!(original.sequence(), 0xa35);
    assert_eq!(original.fragment(), 13);
    for sequence in 0..4096 {
        let changed = original
            .with_sequence(sequence)
            .expect("twelve-bit sequence");
        assert_eq!(changed.sequence(), sequence);
        assert_eq!(changed.fragment(), 13);
        assert_eq!(changed.with_fragment(9).expect("fragment").fragment(), 9);
    }
    assert_eq!(original.with_sequence(4096), Err(Error::ValueOutOfRange));
    assert_eq!(original.with_fragment(16), Err(Error::ValueOutOfRange));
    assert_eq!(original.to_bytes(), [0x5d, 0xa3]);
}

#[test]
fn byte_order_boolean_and_full_width_fields_round_trip() {
    let original = Big::from_bytes([0xb4, 0x5a]);
    assert_eq!(original.prefix(), 10);
    assert!(original.flag());
    for value in 0..2048 {
        let changed = original.with_value(value).expect("eleven bits");
        assert_eq!(changed.value(), value);
        assert_eq!(changed.prefix(), 10);
        assert!(!changed.with_flag(false).flag());
        assert_eq!(changed.with_prefix(3).expect("prefix").prefix(), 3);
    }
    assert!(original.with_value(2048).is_err());
    assert_eq!(
        Full::from_bytes([0; 8])
            .with_value(u64::MAX)
            .expect("full width")
            .value(),
        u64::MAX
    );
}

#[test]
fn unknown_enum_values_survive_and_cannot_escape_the_field() {
    for byte in 0..=u8::MAX {
        let original = Control::from_bytes([byte]);
        assert_eq!(original.with_kind(original.kind()), Ok(original));
    }
    assert_eq!(Control::from_bytes([12]).kind(), Kind::Reserved(3));
    assert!(
        Control::from_bytes([0])
            .with_kind(Kind::Reserved(4))
            .is_err()
    );
}

#[test]
fn cursors_do_not_advance_or_write_on_failure() {
    let mut reader = Reader::new(&[1, 2, 3]);
    assert!(reader.array::<4>().is_err());
    assert!(reader.take(usize::MAX).is_err());
    assert_eq!(reader.u16_le(), Ok(513));
    assert_eq!(reader.u8(), Ok(3));
    assert!(reader.is_empty());
    assert_eq!(reader.take(0), Ok([].as_slice()));
    assert!(reader.u8().is_err());
    let mut bytes = [0xa5; 5];
    let mut writer = Writer::new(&mut bytes);
    assert!(writer.write(&[0; 6]).is_err());
    assert_eq!(writer.written(), 0);
    writer.u16_be(0x1234).expect("capacity");
    assert!(writer.write(&[0; 4]).is_err());
    assert_eq!(writer.remaining(), 3);
    writer.u8(9).expect("capacity");
    writer.u16_le(0x5678).expect("capacity");
    assert_eq!(writer.written(), 5);
    assert_eq!(bytes, [0x12, 0x34, 9, 0x78, 0x56]);
}
