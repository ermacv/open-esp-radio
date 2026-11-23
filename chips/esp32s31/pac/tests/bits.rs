#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]
//! The PAC's field value type: checked values and the conversions that
//! lose nothing. Its cut is the PAC's own (`pub(crate)`).
use oer_chips_esp32s31_pac::{Bits, ValueError};
use oer_portable_bounded::Index;

#[test]
fn a_value_wider_than_its_field_is_an_error() {
    assert_eq!(Bits::<7>::new(0x7f).map(Bits::get), Ok(0x7f));
    assert_eq!(
        Bits::<7>::new(0x80),
        Err(ValueError {
            value: 0x80,
            width: 7
        })
    );
    assert_eq!(Bits::<32>::new(u32::MAX).map(Bits::get), Ok(u32::MAX));
}

#[test]
fn a_byte_a_halfword_and_an_index_convert_into_a_field_that_holds_them() {
    assert_eq!(Bits::<8>::from(0xff_u8).get(), 0xff);
    assert_eq!(Bits::<10>::from(0x80_u8).get(), 0x80);
    assert_eq!(Bits::<32>::from(7_u8).get(), 7);
    assert_eq!(Bits::<16>::from(0xffff_u16).get(), 0xffff);
    assert_eq!(Bits::<26>::from(0x1234_u16).get(), 0x1234);
    // An array of 128 elements fills seven bits; one of four, two.
    assert_eq!(Bits::<7>::from(Index::<128>::constant::<127>()).get(), 127);
    assert_eq!(Bits::<7>::from(Index::<120>::constant::<119>()).get(), 119);
    assert_eq!(Bits::<2>::from(Index::<4>::constant::<3>()).get(), 3);
    assert_eq!(Bits::<32>::from(Index::<4>::constant::<0>()).get(), 0);
}

#[test]
fn affine_values_are_the_constant_times_the_number_plus_the_constant() {
    assert_eq!(
        Bits::<8>::affine::<1, 0xf5, 11>(Index::constant::<0>()).get(),
        0xf5
    );
    assert_eq!(
        Bits::<8>::affine::<1, 0xf5, 11>(Index::constant::<10>()).get(),
        0xff
    );
    assert_eq!(
        Bits::<9>::affine::<32, 31, 16>(Index::constant::<0>()).get(),
        31
    );
    assert_eq!(
        Bits::<9>::affine::<32, 31, 16>(Index::constant::<15>()).get(),
        511
    );
    assert_eq!(
        Bits::<11>::affine::<7, 6, 256>(Index::from_byte(u8::MAX)).get(),
        255 * 7 + 6
    );
}

#[test]
fn a_split_gives_the_low_and_high_parts_whole() {
    let (low, high) = Bits::<11>::constant::<0x7ff>().split::<10, 1>();
    assert_eq!((low.get(), high.get()), (0x3ff, 1));
    let (low, high) = Bits::<11>::constant::<0x3ff>().split::<10, 1>();
    assert_eq!((low.get(), high.get()), (0x3ff, 0));
    let (low, high) = Bits::<11>::constant::<0x400>().split::<10, 1>();
    assert_eq!((low.get(), high.get()), (0, 1));
}

#[test]
fn little_endian_bytes_are_one_value() {
    assert_eq!(
        Bits::<24>::from_le_bytes([0xfd, 0xfe, 0xff]).get(),
        0x00ff_fefd
    );
    assert_eq!(Bits::<32>::from_le_bytes([1, 2, 3, 4]).get(), 0x0403_0201);
    assert_eq!(Bits::<8>::from_le_bytes([0x80]).get(), 0x80);
}
