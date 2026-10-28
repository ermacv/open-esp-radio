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

/// IEEE public identity supplied to a backend, in canonical display order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LePublicDeviceAddress([u8; 6]);
impl LePublicDeviceAddress {
    /// Construct from most-significant-octet-first canonical bytes.
    pub const fn from_canonical_bytes(bytes: [u8; 6]) -> Self {
        Self(bytes)
    }
    /// Most-significant-octet-first canonical bytes.
    pub const fn canonical_bytes(self) -> [u8; 6] {
        self.0
    }
    /// Least-significant-octet-first air-interface bytes.
    pub const fn wire_bytes(self) -> [u8; 6] {
        let [a, b, c, d, e, f] = self.0;
        [f, e, d, c, b, a]
    }
}
#[cfg(test)]
#[allow(clippy::disallowed_macros)]
mod tests {
    use super::*;
    #[test]
    fn public_identity_preserves_canonical_and_air_byte_orders() {
        let public = LePublicDeviceAddress::from_canonical_bytes([1, 2, 3, 4, 5, 6]);
        assert_eq!(public.canonical_bytes(), [1, 2, 3, 4, 5, 6]);
        assert_eq!(public.wire_bytes(), [6, 5, 4, 3, 2, 1]);
        let air =
            LeDeviceAddress::from_wire_bytes(public.wire_bytes(), LeDeviceAddressKind::Public);
        assert_eq!(air.wire_bytes(), [6, 5, 4, 3, 2, 1]);
        assert_eq!(air.kind(), LeDeviceAddressKind::Public);
    }
}
