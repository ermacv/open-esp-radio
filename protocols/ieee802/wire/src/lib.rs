#![no_std]
#![forbid(unsafe_code)]
#![cfg_attr(
    test,
    allow(
        clippy::disallowed_methods,
        clippy::disallowed_macros,
        clippy::unreachable,
        clippy::arithmetic_side_effects
    )
)]

//! What every IEEE 802 wire carries alike: the 48-bit addresses and the
//! EtherType.
//!
//! An [`Eui48`] is an address field's octets, whatever they are: a header
//! from the air may hold any. [`Eui48::kind`] tells an [`IndividualAddress`]
//! from a [`GroupAddress`], and whoever admits a frame keeps the kind its own
//! rule needs.
//!
//! SOURCE: IEEE Std 802-2014, 8.2 (Universal addresses) and 9.2 (EtherTypes).

use oer_portable_wire::{Error, Reader};

/// A 48-bit address, of either kind.
///
/// Octets are no address until they are named one:
///
/// ```compile_fail,E0308
/// use oer_protocols_ieee802_wire::Eui48;
/// let address: Eui48 = [2, 0, 0, 0, 0, 1];
/// ```
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Eui48([u8; Self::LEN]);

impl Eui48 {
    /// The octets of an address.
    pub const LEN: usize = 6;
    /// The address of all zero octets, which names no interface.
    pub const ZERO: Self = Self([0; Self::LEN]);
    /// The broadcast address: all ones.
    ///
    /// SOURCE: IEEE Std 802-2014, 8.2.2 (Assignment by organizations).
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

    /// Read the next address of `reader`.
    ///
    /// # Errors
    /// [`Error`]: fewer octets are left, and none is taken.
    pub fn read(reader: &mut Reader<'_>) -> Result<Self, Error> {
        reader.read().map(Self)
    }

    /// Which kind of address it is.
    pub const fn kind(self) -> Eui48Kind {
        let [first, ..] = self.0;
        if FirstOctet::from_bytes([first]).group() {
            Eui48Kind::Group(GroupAddress(self))
        } else {
            Eui48Kind::Individual(IndividualAddress(self))
        }
    }
}

oer_portable_wire::bitfield! {
    /// The first octet of an address.
    /// SOURCE: IEEE Std 802-2014, 8.2.2 (the I/G and U/L bits).
    struct FirstOctet[1] (little) {
        group, with_group: bool = 0..1;
        local, with_local: bool = 1..2;
    }
}

/// An address by its I/G bit.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Eui48Kind {
    Individual(IndividualAddress),
    Group(GroupAddress),
}

/// An individual address: its I/G bit is clear.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct IndividualAddress(Eui48);

impl IndividualAddress {
    /// `address`, when it is individual.
    pub const fn new(address: Eui48) -> Option<Self> {
        match address.kind() {
            Eui48Kind::Individual(address) => Some(address),
            Eui48Kind::Group(_) => None,
        }
    }

    /// The nonzero individual address an interface has. [`Self::new`] admits
    /// zero octets as well.
    ///
    /// # Errors
    /// [`IndividualAddressError`].
    pub const fn for_interface(address: Eui48) -> Result<Self, IndividualAddressError> {
        if matches!(address.octets(), [0, 0, 0, 0, 0, 0]) {
            return Err(IndividualAddressError::Unspecified);
        }
        match address.kind() {
            Eui48Kind::Individual(address) => Ok(address),
            Eui48Kind::Group(_) => Err(IndividualAddressError::Multicast),
        }
    }

    /// The address, of either kind.
    pub const fn get(self) -> Eui48 {
        self.0
    }
}

/// Why an individual address cannot identify a protocol interface.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndividualAddressError {
    /// All octets are zero.
    Unspecified,
    /// The group bit is set.
    Multicast,
}

impl core::fmt::Display for IndividualAddressError {
    fn fmt(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        formatter.write_str(match self {
            Self::Unspecified => "an interface address cannot be all zero",
            Self::Multicast => "an interface address must be unicast",
        })
    }
}

impl core::error::Error for IndividualAddressError {}

/// A group address: its I/G bit is set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct GroupAddress(Eui48);

impl GroupAddress {
    /// `address`, when it is a group's.
    pub const fn new(address: Eui48) -> Option<Self> {
        match address.kind() {
            Eui48Kind::Group(address) => Some(address),
            Eui48Kind::Individual(_) => None,
        }
    }

    /// The address, of either kind.
    pub const fn get(self) -> Eui48 {
        self.0
    }
}

oer_portable_wire::bitfield! {
    /// The Length/Type field.
    /// SOURCE: IEEE Std 802.3-2022, 3.2.6 (Length/Type field); RFC 1042,
    /// "SNAP header", which carries it after the prefix.
    struct LengthType[2] (big) {
        value, with_value: {u16} = 0..16;
    }
}

/// An EtherType: the Type interpretation of the Length/Type field, as the
/// big-endian number of its registry entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct EtherType(u16);

impl EtherType {
    /// The octets of a type.
    pub const LEN: usize = 2;
    /// Internet Protocol version 4.
    ///
    /// SOURCE: IEEE Registration Authority EtherType registry; RFC 894.
    pub const IPV4: Self = Self(0x0800);
    /// Address Resolution Protocol.
    ///
    /// SOURCE: IEEE Registration Authority EtherType registry; RFC 826.
    pub const ARP: Self = Self(0x0806);
    /// Customer VLAN tag (C-TAG).
    ///
    /// SOURCE: IEEE Std 802.1Q-2022, 9.5 (Tag Protocol Identification), Table 9-1.
    pub const VLAN: Self = Self(0x8100);
    /// Internet Protocol version 6.
    ///
    /// SOURCE: IEEE Registration Authority EtherType registry; RFC 8200.
    pub const IPV6: Self = Self(0x86dd);
    /// Service VLAN tag (S-TAG).
    ///
    /// SOURCE: IEEE Std 802.1Q-2022, 9.5 (Tag Protocol Identification), Table 9-1.
    pub const PROVIDER_VLAN: Self = Self(0x88a8);
    /// EAP over LAN: IEEE 802.1X port access entity frames, which carry the
    /// RSN 4-way and group key handshakes.
    ///
    /// SOURCE: IEEE Std 802.1X-2020, 11.1.4 (PAE EtherType); IEEE Std
    /// 802.11-2020, 12.7.2 (EAPOL-Key frames).
    pub const EAPOL: Self = Self(0x888e);
    /// WLAN Authentication and Privacy Infrastructure.
    ///
    /// SOURCE: IEEE Registration Authority EtherType registry (88-B4).
    pub const WAPI: Self = Self(0x88b4);

    pub const fn new(value: u16) -> Self {
        Self(value)
    }

    /// The type these octets hold, in transmission order.
    pub const fn from_octets(octets: [u8; Self::LEN]) -> Self {
        Self(LengthType::from_bytes(octets).value())
    }

    /// Its octets, in transmission order.
    pub const fn octets(self) -> [u8; Self::LEN] {
        LengthType::from_bytes([0; Self::LEN])
            .with_value(self.0)
            .to_bytes()
    }

    /// Read the next type of `reader`.
    ///
    /// # Errors
    /// [`Error`]: fewer octets are left, and none is taken.
    pub fn read(reader: &mut Reader<'_>) -> Result<Self, Error> {
        reader.read().map(Self::from_octets)
    }

    pub const fn value(self) -> u16 {
        self.0
    }

    /// Whether the field names a type rather than an IEEE 802.3 length.
    ///
    /// SOURCE: IEEE Std 802.3-2022, 3.2.6: values of 1536 (0x0600) and above
    /// are types.
    pub const fn is_type(self) -> bool {
        self.0 >= 0x0600
    }

    /// Whether the type names a VLAN tag (C-TAG or S-TAG).
    pub const fn is_vlan_tag(self) -> bool {
        matches!(self, Self::VLAN | Self::PROVIDER_VLAN)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_group_bit_alone_tells_the_kinds_apart() {
        for first in 0..=u8::MAX {
            let address = Eui48::new([first, 1, 2, 3, 4, 5]);
            assert_eq!(address.octets(), [first, 1, 2, 3, 4, 5]);
            if first & 1 == 1 {
                let group = GroupAddress::new(address).unwrap();
                assert_eq!(group.get(), address);
                assert_eq!(address.kind(), Eui48Kind::Group(group));
                assert_eq!(IndividualAddress::new(address), None);
            } else {
                let individual = IndividualAddress::new(address).unwrap();
                assert_eq!(individual.get(), address);
                assert_eq!(address.kind(), Eui48Kind::Individual(individual));
                assert_eq!(GroupAddress::new(address), None);
            }
        }
    }

    #[test]
    fn each_refusal_says_its_own_reason() {
        extern crate std;
        use std::string::ToString;
        assert_eq!(
            IndividualAddressError::Unspecified.to_string(),
            "an interface address cannot be all zero"
        );
        assert_eq!(
            IndividualAddressError::Multicast.to_string(),
            "an interface address must be unicast"
        );
    }

    #[test]
    fn an_interface_has_a_nonzero_individual_address() {
        assert_eq!(
            IndividualAddress::for_interface(Eui48::ZERO),
            Err(IndividualAddressError::Unspecified)
        );
        assert!(IndividualAddress::new(Eui48::ZERO).is_some());
        assert_eq!(
            IndividualAddress::for_interface(Eui48::BROADCAST),
            Err(IndividualAddressError::Multicast)
        );
        for index in 0..Eui48::LEN {
            let mut octets = [0; Eui48::LEN];
            octets[index] = 2;
            let address = Eui48::new(octets);
            assert_eq!(
                IndividualAddress::for_interface(address).map(IndividualAddress::get),
                Ok(address)
            );
        }
    }

    #[test]
    fn an_ether_type_is_its_two_octets_the_high_one_first() {
        assert_eq!(EtherType::EAPOL.octets(), [0x88, 0x8e]);
        assert_eq!(EtherType::WAPI.octets(), [0x88, 0xb4]);
        assert_eq!(EtherType::from_octets([0x86, 0xdd]), EtherType::IPV6);
        assert_eq!(
            EtherType::read(&mut Reader::new(&[0x08, 0x06, 0xff])),
            Ok(EtherType::ARP)
        );
        assert!(EtherType::read(&mut Reader::new(&[0x08])).is_err());
    }

    #[test]
    fn ether_types_tell_types_from_lengths_and_name_vlan_tags() {
        assert!(EtherType::new(0x0600).is_type());
        assert!(!EtherType::new(0x05dc).is_type());
        assert!(EtherType::VLAN.is_vlan_tag());
        assert!(EtherType::PROVIDER_VLAN.is_vlan_tag());
        assert!(!EtherType::IPV6.is_vlan_tag());
        assert_eq!(EtherType::IPV6.value(), 0x86dd);
        assert_eq!(EtherType::ARP.value(), 0x0806);
    }
}
