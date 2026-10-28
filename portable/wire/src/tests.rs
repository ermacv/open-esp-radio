#![allow(clippy::disallowed_macros, clippy::disallowed_methods)]

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
        assert_eq!(Big::from_bytes(changed.to_bytes()).value(), value);
        assert_eq!(changed.prefix(), 10);
        assert!(!changed.with_flag(false).flag());
        assert_eq!(changed.with_prefix(3).expect("prefix").prefix(), 3);
    }
    assert!(original.with_value(2048).is_err());
    assert_eq!(
        Full::from_bytes([0; 8])
            .with_value(u64::MAX)
            .map(|field| Full::from_bytes(field.to_bytes()))
            .expect("full width")
            .value(),
        u64::MAX
    );
}

#[test]
fn unknown_enum_values_survive_and_cannot_escape_the_field() {
    for byte in 0..=u8::MAX {
        let original = Control::from_bytes([byte]);
        assert_eq!(
            original
                .with_kind(original.kind())
                .map(|field| field.to_bytes()),
            Ok(original.to_bytes())
        );
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

#[test]
fn writer_empty_exact_and_refused_writes_retain_the_unwritten_suffix() {
    let mut bytes = [0xa5; 3];
    let mut writer = Writer::new(&mut bytes);
    assert_eq!(writer.write(&[]), Ok(()));
    assert_eq!((writer.written(), writer.remaining()), (0, 3));
    assert_eq!(writer.write(&[1]), Ok(()));
    assert_eq!(
        writer.write(&[2, 3, 4]),
        Err(Error::Truncated {
            required: 3,
            remaining: 2,
        })
    );
    assert_eq!((writer.written(), writer.remaining()), (1, 2));
    assert_eq!(writer.write(&[]), Ok(()));
    assert_eq!((writer.written(), writer.remaining()), (1, 2));
    assert_eq!(writer.write(&[2, 3]), Ok(()));
    assert_eq!((writer.written(), writer.remaining()), (3, 0));
    assert_eq!(writer.write(&[]), Ok(()));
    assert_eq!(
        writer.write(&[4]),
        Err(Error::Truncated {
            required: 1,
            remaining: 0,
        })
    );
    assert_eq!((writer.written(), writer.remaining()), (3, 0));
    assert_eq!(bytes, [1, 2, 3]);

    let mut empty = [];
    let mut writer = Writer::new(&mut empty);
    assert_eq!(writer.write(&[]), Ok(()));
    assert_eq!((writer.written(), writer.remaining()), (0, 0));
}

#[test]
fn refused_write_preserves_previously_written_and_unwritten_bytes() {
    let mut bytes = [0xa5; 4];
    {
        let mut writer = Writer::new(&mut bytes);
        writer.write(&[1]).unwrap();
        assert_eq!(
            writer.write(&[2, 3, 4, 5]),
            Err(Error::Truncated {
                required: 4,
                remaining: 3,
            })
        );
        assert_eq!((writer.written(), writer.remaining()), (1, 3));
    }
    assert_eq!(bytes, [1, 0xa5, 0xa5, 0xa5]);
}

crate::bitfield! {
    struct Span[9] (little) {
        value, with_value: u64 = 7..71;
    }
}

#[test]
fn a_full_width_field_can_cross_nine_octets() {
    let original = Span::from_bytes([0xa5; 9]);
    for value in [0, 1, 0x0123_4567_89ab_cdef, u64::MAX] {
        let changed = original.with_value(value).expect("full width");
        assert_eq!(changed.value(), value);
        let bytes = changed.to_bytes();
        assert_eq!(bytes[0] % 128, original.to_bytes()[0] % 128);
        assert_eq!(bytes[8] / 128, original.to_bytes()[8] / 128);
        assert_eq!(Reader::new(&bytes).bits::<64>(7), Ok(value));
    }
}

#[test]
fn bit_spans_reject_truncation_and_overflow_without_moving() {
    let bytes = [0x96, 0x35, 0xe2, 0x71];
    let mut reader = Reader::new(&bytes);
    assert_eq!(reader.bits::<8>(4), Ok(0x59));
    assert!(reader.bits::<21>(12).is_err());
    assert!(reader.bits::<1>(usize::MAX).is_err());
    assert_eq!(reader.read::<4>(), Ok(bytes));
    assert_eq!(
        Reader::new(&[]).bits::<1>(0),
        Err(Error::Truncated {
            required: 1,
            remaining: 0
        })
    );
    assert_eq!(Reader::new(&[0x80]).bits::<1>(7), Ok(1));
    assert_eq!(
        Reader::new(&bytes).bits::<1>(usize::MAX - 1),
        Err(Error::Truncated {
            required: usize::MAX / 8 + 1,
            remaining: bytes.len(),
        })
    );
    assert_eq!(
        Reader::new(&bytes).bits::<1>(usize::MAX),
        Err(Error::Truncated {
            required: usize::MAX,
            remaining: bytes.len(),
        })
    );
}

crate::bitfield! {
    struct Views[1] (little) {
        combined, with_combined: u8 = 0..8, view;
        low, with_low: u8 = 0..4;
        high, with_high: u8 = 4..8;
        flag, with_flag: bool = 7..8, view;
    }
}

#[test]
fn explicit_views_alias_fields_before_and_after_them() {
    for raw in 0..=u8::MAX {
        let fields = Views::from_bytes([raw]);
        assert_eq!(fields.combined(), raw);
        assert_eq!(fields.low(), raw % 16);
        assert_eq!(fields.high(), raw / 16);
        assert_eq!(fields.flag(), raw >= 128);
        let low = fields.with_low(3).expect("four-bit value");
        assert_eq!(low.combined(), (raw / 16) * 16 + 3);
        let high = fields.with_high(5).expect("four-bit value");
        assert_eq!(high.combined(), 80 + raw % 16);
        assert_eq!(fields.with_flag(false).high(), fields.high() % 8);
        let written = fields.with_combined(raw).expect("full octet");
        assert_eq!(Views::from_bytes(written.to_bytes()), fields);
    }
}

crate::bitfield! {
    struct Proved[2] (little) {
        fragment, with_fragment: (u8) = 0..4;
        sequence, with_sequence: (u16) = 4..16;
    }
}
crate::bitfield! {
    struct BigSpan[9] (big) { value, with_value: u64 = 7..71; }
}

#[test]
fn proved_fields_refuse_bad_input_before_infallible_insertion() {
    assert!(crate::U8::<4>::new(16).is_none());
    assert!(crate::U16::<12>::new(4096).is_none());
    for sequence in 0..4096 {
        let value = Proved::from_bytes([0xa5, 0x5a])
            .with_sequence(crate::U16::new(sequence).unwrap())
            .with_fragment(crate::U8::constant::<13>());
        assert_eq!(
            value
                .with_sequence(value.sequence())
                .with_fragment(value.fragment()),
            value
        );
        assert_eq!(value.sequence().get(), sequence);
        assert_eq!(value.fragment().get(), 13);
        assert_eq!(value.to_bytes(), ((sequence << 4) | 13).to_le_bytes());
    }
}

#[test]
fn owned_prefix_lengths_refuse_overlong_input_and_borrow_exactly() {
    let bytes = [0, 1, 2, 3, 4];
    for length in 0..=bytes.len() {
        let admitted = crate::PrefixLength::new(length).unwrap();
        assert_eq!(admitted.borrow(&bytes), &bytes[..length]);
        assert_eq!(admitted.get(), length);
        let mut destination = [0xa5; 5];
        admitted
            .borrow_mut(&mut destination)
            .copy_from_slice(&bytes[..length]);
        assert_eq!(&destination[..length], &bytes[..length]);
        assert_eq!(&destination[length..], &[0xa5; 5][length..]);
    }
    assert!(crate::PrefixLength::<5>::new(6).is_none());
    assert!(crate::PrefixLength::<5>::new(usize::MAX).is_none());
    assert_eq!(crate::PrefixLength::<0>::ZERO.borrow(&[]), &[]);
}

#[test]
fn unaligned_big_endian_fields_retain_reserved_end_bits() {
    for value in [0, 1, 0x0123_4567_89ab_cdef, u64::MAX] {
        let original = BigSpan::from_bytes([0xa5; 9]);
        let changed = original.with_value(value).unwrap();
        assert_eq!(changed.value(), value);
        let mut little = changed.to_bytes();
        little.reverse();
        assert_eq!(Reader::new(&little).bits::<64>(7), Ok(value));
        assert_eq!(changed.to_bytes()[0] & 0x80, 0x80);
        assert_eq!(changed.to_bytes()[8] & 0x7f, 0x25);
    }
}

crate::bitfield! {
    struct Proved32[4] (little) { value, with_value: (u32) = 0..32; }
}
crate::bitfield! {
    struct ProvedBigSpan[9] (big) { value, with_value: (u64) = 7..71; }
}

#[test]
fn full_width_and_unaligned_getters_retain_proofs_without_truncation() {
    let field = Proved32::from_bytes(u32::MAX.to_le_bytes());
    assert_eq!(field.value().get(), u32::MAX);
    assert_eq!(
        Proved32::from_bytes([0; 4]).with_value(field.value()),
        field
    );
    for raw in [0, 1, 0x0123_4567_89ab_cdef, u64::MAX] {
        let original = ProvedBigSpan::from_bytes([0xa5; 9]);
        let changed = original.with_value(crate::U64::new(raw).unwrap());
        assert_eq!(changed.value().get(), raw);
        assert_eq!(changed.with_value(changed.value()), changed);
        assert_eq!(changed.to_bytes()[0] & 0x80, 0x80);
        assert_eq!(changed.to_bytes()[8] & 0x7f, 0x25);
    }
}

crate::bitfield! {
    struct PaddedLittle[12] (little) { value, with_value: (u64) = 15..79; }
}
crate::bitfield! {
    struct PaddedBig[12] (big) { value, with_value: (u64) = 15..79; }
}

#[test]
fn const_fields_skip_whole_octets_and_preserve_surrounding_data() {
    const VALUE: crate::U64<64> = crate::U64::constant::<0x0123_4567_89ab_cdef>();
    const LITTLE: PaddedLittle = PaddedLittle::from_bytes([0xa5; 12]).with_value(VALUE);
    const BIG: PaddedBig = PaddedBig::from_bytes([0xa5; 12]).with_value(VALUE);
    const DECODED: [u64; 2] = [LITTLE.value().get(), BIG.value().get()];

    assert_eq!(DECODED, [VALUE.get(); 2]);
    assert_eq!(LITTLE.value(), VALUE);
    assert_eq!(BIG.value(), VALUE);
    // The field is the low 64 bits shifted by 15, retaining both partial
    // boundary octets and the untouched octets outside the field.
    let expected = [
        0xa5, 0xa5, 0xf7, 0xe6, 0xd5, 0xc4, 0xb3, 0xa2, 0x91, 0x80, 0xa5, 0xa5,
    ];
    assert_eq!(LITTLE.to_bytes(), expected);
    let mut big = expected;
    big.reverse();
    assert_eq!(BIG.to_bytes(), big);
}
