#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_macros,
        clippy::disallowed_methods,
        clippy::arithmetic_side_effects
    )
)]
//! Espressif eFuse values as pure logic over the fields the chip's eFuse
//! controller presents in its read registers.
//!
//! The controller mirrors every eFuse block into read-only `RD_*` words after
//! reset; reading one needs no command and no wait. The chip HAL reads the
//! fields and this crate turns them into values: the factory MAC address, its
//! extension and the addresses the Wi-Fi interfaces use. It contains no
//! address and no register access.
//!
//! [`FactoryMac`] holds the fields of block 1 that carry the factory MAC
//! (`RD_MAC_SYS0` and `RD_MAC_SYS1`), which the ESP32-S31 and the ESP32-C5
//! lay out alike (SOURCE: esp-hal 1f646c5 `esp-hal/src/efuse/esp32s31/fields.rs:191-195`
//! and `efuse/esp32c5/fields.rs:179-183`); the chip PAC splits the words into
//! these fields.
#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests;

/// An EUI-48 address, first transmitted octet first.
pub type MacAddress = [u8; 6];

/// The factory MAC fields of eFuse block 1, as the chip PAC reads them.
///
/// SOURCE: ESP-IDF 4d80c1b2
/// `components/soc/esp32s31/register/soc/efuse_reg.h:1239-1268`:
/// `RD_MAC_SYS0` bits 31:0 are the low 32 bits of the MAC (`MAC_0`),
/// `RD_MAC_SYS1` bits 15:0 its high 16 bits (`MAC_1`) and bits 31:16 the
/// extended bits (`MAC_EXT`). `components/efuse/esp32s31/esp_efuse_table.c:818-825`
/// lists the six `MAC_FACTORY` octets from block-1 bit 40 down to bit 0, so
/// the first octet is the high byte of `MAC_1` and the last the low byte of
/// `MAC_0`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FactoryMac {
    /// `RD_MAC_SYS0.MAC_0`: MAC bits 31:0.
    pub mac_0: u32,
    /// `RD_MAC_SYS1.MAC_1`: MAC bits 47:32.
    pub mac_1: u16,
    /// `RD_MAC_SYS1.MAC_EXT`: the extended bits.
    pub mac_ext: u16,
}

impl FactoryMac {
    /// The factory (universally administered) MAC address.
    pub const fn base_mac(self) -> MacAddress {
        let [a, b] = self.mac_1.to_be_bytes();
        let [c, d, e, f] = self.mac_0.to_be_bytes();
        [a, b, c, d, e, f]
    }

    /// `MAC_EXT` as the integer value of its field.
    ///
    /// SOURCE: esp-hal 1f646c5 `esp-hal/src/efuse/mod.rs:90-152` and
    /// `efuse/esp32s31/fields.rs:195`: `read_field_le::<u16>(MAC_EXT)` reads
    /// block-1 bits 63:48 little-endian, which is this integer; the PHY
    /// calibration identity (`mac_extension`) records it. ESP-IDF's
    /// `ESP_EFUSE_MAC_EXT` blob (`esp_efuse_table.c:827-830`) lists the same two
    /// bytes high byte first, which is not the order used here.
    pub const fn mac_extension(self) -> u16 {
        self.mac_ext
    }
}

/// A Wi-Fi interface whose address derives from the factory MAC.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interface {
    /// The station uses the factory MAC itself.
    Station,
    /// The access point uses the [`locally_administered`] address.
    AccessPoint,
}

impl Interface {
    /// This interface's address for the factory MAC `base`.
    pub const fn address(self, base: MacAddress) -> MacAddress {
        match self {
            Self::Station => base,
            Self::AccessPoint => locally_administered(base),
        }
    }
}

/// The locally administered address derived from `universal`: the first
/// octet gains the U/L bit (0x02); when that bit is already set, bit 0x04 of
/// the first octet flips instead. The other five octets are unchanged.
///
/// SOURCE: ESP-IDF 4d80c1b2 `components/esp_hw_support/mac_addr.c:303-321`
/// (`esp_derive_local_mac`), which esp-hal 1f646c5
/// `esp-hal/src/efuse/mod.rs:468-480` reproduces for the access point.
pub const fn locally_administered(universal: MacAddress) -> MacAddress {
    const UNIVERSAL_LOCAL: u8 = 0x02;
    const FLIP_WHEN_LOCAL: u8 = 0x04;
    let [first, b, c, d, e, f] = universal;
    let first = if first & UNIVERSAL_LOCAL == 0 {
        first | UNIVERSAL_LOCAL
    } else {
        first ^ FLIP_WHEN_LOCAL
    };
    [first, b, c, d, e, f]
}
