use super::{FactoryMac, Interface, MacAddress, locally_administered};

const FACTORY: FactoryMac = FactoryMac {
    mac_0: 0x3344_5566,
    mac_1: 0x1122,
    mac_ext: 0x0718,
};

#[test]
fn base_mac_starts_with_the_high_byte_of_mac_1() {
    assert_eq!(FACTORY.base_mac(), [0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);
}

#[test]
fn mac_extension_is_the_field_value() {
    assert_eq!(FACTORY.mac_extension(), 0x0718);
}

#[test]
fn station_uses_the_factory_mac() {
    let base = FACTORY.base_mac();
    assert_eq!(Interface::Station.address(base), base);
}

#[test]
fn access_point_uses_the_locally_administered_mac() {
    let base = FACTORY.base_mac();
    let access_point = Interface::AccessPoint.address(base);
    assert_eq!(access_point, [0x13, 0x22, 0x33, 0x44, 0x55, 0x66]);
    assert_eq!(access_point, locally_administered(base));
}

/// ESP-IDF's `esp_derive_local_mac` as its steps read: set the U/L bit, and
/// flip bit 0x04 when that left the first octet unchanged.
fn idf_derive_local_mac(universal: MacAddress) -> MacAddress {
    let mut local = universal;
    local[0] |= 0x2;
    if local[0] == universal[0] {
        local[0] ^= 0x4;
    }
    local
}

#[test]
fn locally_administered_matches_esp_idf_for_every_first_octet() {
    for first in 0..=u8::MAX {
        let universal = [first, 0xa1, 0xb2, 0xc3, 0xd4, 0xe5];
        let local = locally_administered(universal);
        assert_eq!(
            local,
            idf_derive_local_mac(universal),
            "first octet {first:#04x}"
        );
        assert_ne!(local[0], first, "first octet {first:#04x} must change");
        assert_eq!(local[1..], universal[1..]);
    }
}

#[test]
fn factory_fields_cover_every_octet_at_their_full_width() {
    for byte in 0..=u8::MAX {
        let low = u32::from_be_bytes([byte; 4]);
        let high = u16::from_be_bytes([byte; 2]);
        let factory = FactoryMac {
            mac_0: low,
            mac_1: high,
            mac_ext: high,
        };
        assert_eq!(factory.base_mac(), [byte; 6]);
        assert_eq!(factory.mac_extension(), high);
    }
}
