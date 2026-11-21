//! The vocabulary's values: a field's bounds, extraction and insertion, and
//! a route's read selection.

use super::*;

#[test]
fn fields_are_bounded_to_one_byte() {
    assert!(AnalogField::new(0_u8, 7, 0).is_some());
    assert!(AnalogField::new(0_u8, 3, 4).is_none());
    assert!(AnalogField::new(0_u8, 8, 0).is_none());
}

#[test]
fn extraction_and_placement_follow_the_vendor_leaves() {
    let field = AnalogField::new(0_u8, 5, 2).expect("field");
    assert_eq!(field.extract(0b1011_0110), 0b1101);
    let placed = field.place(0b0010).expect("fits");
    assert_eq!(placed.apply(0b1111_1111), 0b1100_1011);
    assert_eq!(placed.value(), 0b0010);
    // A value wider than the field would leak above it in the vendor leaf.
    assert_eq!(field.place(0b1_0000), None);
    let whole = AnalogField::new(0_u8, 7, 0).expect("field");
    assert_eq!(whole.place(0xab).expect("fits").apply(0x12), 0xab);
}

#[test]
fn a_value_read_from_its_field_applies_back_to_the_same_lanes() {
    let field = AnalogField::new(0_u8, 5, 2).expect("field");
    let read = field.value_in(0b1011_0110);
    assert_eq!(read.value(), 0b1101);
    // Applied to another byte, it replaces the field and keeps the rest.
    assert_eq!(read.apply(0b0000_0001), 0b0011_0101);
    assert_eq!(read.apply(0b1100_0010), 0b1111_0110);
    // A field of the same lanes elsewhere takes it alike.
    let twin = AnalogField::new(1_u8, 5, 2).expect("field");
    assert!(field.same_lanes(twin));
    assert!(!field.same_lanes(AnalogField::new(1_u8, 5, 1).expect("field")));
}

/// A constant is placed in a const item.
const PLACED: FieldValue = match AnalogField::new((), 3, 3) {
    Some(field) => match field.place(1) {
        Some(value) => value,
        None => panic!("one fits a bit"),
    },
    None => panic!("a bit lies in its byte"),
};

#[test]
fn a_constant_placed_at_build_time_sets_its_bit_alone() {
    assert_eq!(PLACED.apply(0), 0b1000);
    assert_eq!(PLACED.apply(0xff), 0xff);
}

#[test]
fn explicit_truncation_preserves_the_existing_byte_input_contract() {
    for lsb in 0..8 {
        for msb in lsb..8 {
            let field = AnalogField::new((), msb, lsb).unwrap();
            let mask = (((1_u16 << (msb - lsb + 1)) - 1) << lsb) as u8;
            for sample in 0..=255_u8 {
                for value in 0..=255_u8 {
                    let result = field.insert_truncated(sample, value);
                    assert_eq!(result & !mask, sample & !mask);
                    assert_eq!(
                        u16::from(field.extract(result)),
                        u16::from(value) % (1 << (msb - lsb + 1))
                    );
                    if let Some(checked) = field.place(value) {
                        assert_eq!(checked.apply(sample), result);
                    }
                }
            }
        }
    }
}

#[test]
fn a_field_names_its_bit_range_and_an_address_its_block_and_byte() {
    let field = AnalogField::new((), 5, 2).expect("a field of one byte");
    assert_eq!(field.bit_range(), (5, 2));
    assert_eq!(field.place(0b1111).expect("fits").apply(0), 0b0011_1100);
    let block = BlockCode::new(0x66);
    assert_eq!(block.code(), 0x66);
    let address = block.register(3);
    assert_eq!((address.block(), address.index()), (0x66, 3));
}

#[test]
fn a_route_complements_its_read_selection_within_its_mask() {
    let route = |read_mask_bits| Route {
        request_code: 0x62,
        command_code: 0x66,
        host: Host::Host1,
        read_select: 0x0102,
        read_mask_bits,
    };
    assert_eq!(route(24).complemented_read_select(), 0x00ff_fefd);
    assert_eq!(route(32).complemented_read_select(), 0xffff_fefd);
    let address = route(24).address(7);
    assert_eq!((address.block(), address.index()), (0x66, 7));
}
