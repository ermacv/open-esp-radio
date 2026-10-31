//! The VLAN tag after a C-TAG or S-TAG EtherType.
//!
//! SOURCE: IEEE Std 802.1Q-2022, 9.5 (Tag Protocol Identification) and 9.6
//! (Tag Control Information).

use crate::ethernet::EtherType;
use oer_wire::{Error, Reader};

/// Length of the tag's Tag Control Information and inner EtherType; the Tag
/// Protocol Identifier is the outer EtherType.
pub const TAG_LEN: usize = 4;

oer_wire::bitfield! {
    /// VLAN tag control information, including Priority Code Point.
    /// SOURCE: IEEE Std 802.1Q-2022, 9.6 (Tag Control Information).
    pub struct VlanTci[2] (big) {
        pub vid, with_vid: u16 = 0..12;
        pub dei, with_dei: bool = 12..13;
        pub pcp, with_pcp: u8 = 13..16;
    }
}

/// One VLAN tag: its control information and the EtherType it encloses.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VlanTag {
    pub tci: VlanTci,
    pub ether_type: EtherType,
}

impl VlanTag {
    /// The tag at the front of a tagged frame's payload and the payload after
    /// it.
    pub fn parse(payload: &[u8]) -> Result<(Self, &[u8]), Error> {
        let mut reader = Reader::new(payload);
        let tci = VlanTci::from_bytes(reader.read::<2>()?);
        let ether_type = EtherType::new(reader.u16_be()?);
        Ok((Self { tci, ether_type }, reader.remaining()))
    }
}
