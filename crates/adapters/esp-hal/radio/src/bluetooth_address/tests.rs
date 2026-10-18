use super::bluetooth_public_address_from_base;

#[test]
fn bluetooth_uses_the_s31_second_universal_address() {
    assert_eq!(
        bluetooth_public_address_from_base([0x30, 0xed, 0xa0, 0xf3, 0xd3, 0xec]).canonical_bytes(),
        [0x30, 0xed, 0xa0, 0xf3, 0xd3, 0xed]
    );
    assert_eq!(
        bluetooth_public_address_from_base([1, 2, 3, 4, 5, u8::MAX]).canonical_bytes(),
        [1, 2, 3, 4, 5, 0]
    );
}
