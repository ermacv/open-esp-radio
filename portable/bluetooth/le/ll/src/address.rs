//! LE device addresses at the air-interface boundary.

/// Address class carried by the TxAdd or RxAdd field of an advertising PDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LeDeviceAddressKind {
    /// IEEE-assigned public device address.
    Public,
    /// Controller- or Host-generated random device address.
    Random,
}

/// Six-octet LE device address in over-the-air octet order.
///
/// Keeping the byte order explicit prevents the HCI presentation order from
/// leaking into the packet codec. HCI adapters must perform that conversion at
/// their own boundary.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LeDeviceAddress {
    wire_bytes: [u8; 6],
    kind: LeDeviceAddressKind,
}

impl LeDeviceAddress {
    /// Construct an address from the six octets in air-interface PDU order.
    pub const fn from_wire_bytes(wire_bytes: [u8; 6], kind: LeDeviceAddressKind) -> Self {
        Self { wire_bytes, kind }
    }

    /// Return the address class encoded by TxAdd or RxAdd.
    pub const fn kind(self) -> LeDeviceAddressKind {
        self.kind
    }

    /// Return the six octets in air-interface PDU order.
    pub const fn wire_bytes(self) -> [u8; 6] {
        self.wire_bytes
    }
}
