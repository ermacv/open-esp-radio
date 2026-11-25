//! IPv4 and IPv6 addresses.
//!
//! An address is a field's octets, whatever they are: which addresses a host
//! may have is its stack's decision.
//!
//! SOURCE: RFC 791, 3.2 (Addressing); RFC 4291, 2 (IPv6 Addressing).

use oer_portable_wire::{Error, Reader};

/// An IPv4 address.
///
/// Octets are no address until they are named one:
///
/// ```compile_fail,E0308
/// use oer_protocols_network_wire::address::Ipv4Address;
/// let address: Ipv4Address = [192, 0, 2, 1];
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Ipv4Address([u8; Self::LEN]);

oer_portable_wire::bitfield! {
    /// An IPv4 address as the number its network and host bits are counted
    /// in, the most significant first.
    /// SOURCE: RFC 791, 2.3 (Function Description, Addressing).
    struct Ipv4Number[4] (big) {
        value, with_value: {u32} = 0..32;
    }
}

impl Ipv4Address {
    /// The octets of an address.
    pub const LEN: usize = 4;
    /// This host on this network, before it has an address.
    ///
    /// SOURCE: RFC 1122, 3.2.1.3 (a).
    pub const UNSPECIFIED: Self = Self([0; Self::LEN]);
    /// The limited broadcast address.
    ///
    /// SOURCE: RFC 1122, 3.2.1.3 (c).
    pub const BROADCAST: Self = Self([0xff; Self::LEN]);

    /// The address these octets hold, in transmission order.
    pub const fn new(octets: [u8; Self::LEN]) -> Self {
        Self(octets)
    }

    /// Its octets, in transmission order.
    pub const fn octets(self) -> [u8; Self::LEN] {
        self.0
    }

    /// Its octets where they lie, in transmission order.
    pub const fn as_octets(&self) -> &[u8; Self::LEN] {
        &self.0
    }

    /// The number a prefix or a mask is counted in.
    pub const fn bits(self) -> u32 {
        Ipv4Number::from_bytes(self.0).value()
    }

    /// Read the next address of `reader`.
    ///
    /// # Errors
    /// [`Error`]: fewer octets are left, and none is taken.
    pub fn read(reader: &mut Reader<'_>) -> Result<Self, Error> {
        reader.read().map(Self)
    }
}

/// An IPv6 address.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Ipv6Address([u8; Self::LEN]);

impl Ipv6Address {
    /// The octets of an address.
    pub const LEN: usize = 16;

    /// The address these octets hold, in transmission order.
    pub const fn new(octets: [u8; Self::LEN]) -> Self {
        Self(octets)
    }

    /// Its octets, in transmission order.
    pub const fn octets(self) -> [u8; Self::LEN] {
        self.0
    }

    /// Read the next address of `reader`.
    ///
    /// # Errors
    /// [`Error`]: fewer octets are left, and none is taken.
    pub fn read(reader: &mut Reader<'_>) -> Result<Self, Error> {
        reader.read().map(Self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_address_is_its_octets_and_their_number() {
        let address = Ipv4Address::new([192, 0, 2, 1]);
        assert_eq!(address.octets(), [192, 0, 2, 1]);
        assert_eq!(address.as_octets(), &[192, 0, 2, 1]);
        assert_eq!(address.bits(), 0xc000_0201);
        assert_eq!(Ipv4Address::UNSPECIFIED.bits(), 0);
        assert_eq!(Ipv4Address::BROADCAST.bits(), u32::MAX);
    }

    #[test]
    fn an_address_is_read_whole_or_not_at_all() {
        let mut octets = [0; Ipv4Address::LEN + Ipv6Address::LEN];
        for (index, octet) in octets.iter_mut().enumerate() {
            *octet = u8::try_from(index).unwrap();
        }
        let mut reader = Reader::new(&octets);
        assert_eq!(
            Ipv4Address::read(&mut reader),
            Ok(Ipv4Address::new([0, 1, 2, 3]))
        );
        let mut expected = [0; Ipv6Address::LEN];
        expected.copy_from_slice(&octets[4..]);
        let address = Ipv6Address::read(&mut reader).unwrap();
        assert_eq!(address, Ipv6Address::new(expected));
        assert_eq!(address.octets(), expected);
        assert!(Ipv4Address::read(&mut Reader::new(&octets[..3])).is_err());
        assert!(Ipv6Address::read(&mut Reader::new(&octets[..15])).is_err());
    }
}
