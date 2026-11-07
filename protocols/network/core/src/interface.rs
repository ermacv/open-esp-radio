//! The one interface of this core.

/// The all-stations address.
pub(crate) const BROADCAST_HARDWARE: [u8; 6] = [0xFF; 6];
const BROADCAST: [u8; 4] = [0xFF; 4];

/// An interface's identity and addressing.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Interface {
    hardware: [u8; 6],
    address: [u8; 4],
    mask: u32,
    gateway: Option<[u8; 4]>,
    mtu: u16,
}

/// An interface that cannot be.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InterfaceError {
    /// An IPv4 prefix has at most 32 bits.
    Prefix(u8),
    /// The MTU does not carry the IPv4 minimum of RFC 791, 3.1.
    Mtu(u16),
    /// The gateway does not lie on the interface's network.
    Gateway,
}

impl Interface {
    /// # Errors
    /// [`InterfaceError`]: what is not valid.
    pub fn new(
        hardware: [u8; 6],
        address: [u8; 4],
        prefix: u8,
        gateway: Option<[u8; 4]>,
        mtu: u16,
    ) -> Result<Self, InterfaceError> {
        if prefix > 32 {
            return Err(InterfaceError::Prefix(prefix));
        }
        if mtu < 68 {
            return Err(InterfaceError::Mtu(mtu));
        }
        let mask = u32::MAX
            .checked_shl(32_u32.wrapping_sub(u32::from(prefix)))
            .unwrap_or(0);
        let interface = Self {
            hardware,
            address,
            mask,
            gateway,
            mtu,
        };
        if gateway.is_some_and(|gateway| !interface.on_link(gateway)) {
            return Err(InterfaceError::Gateway);
        }
        Ok(interface)
    }

    pub(crate) const fn hardware(&self) -> [u8; 6] {
        self.hardware
    }

    pub(crate) const fn address(&self) -> [u8; 4] {
        self.address
    }

    pub(crate) const fn mtu(&self) -> u16 {
        self.mtu
    }

    fn on_link(&self, address: [u8; 4]) -> bool {
        (u32::from_be_bytes(address) ^ u32::from_be_bytes(self.address)) & self.mask == 0
    }

    /// The network's broadcast address, or the limited one. Prefixes of 31
    /// and 32 bits have no broadcast address of their own, RFC 3021.
    pub(crate) fn is_broadcast(&self, address: [u8; 4]) -> bool {
        address == BROADCAST
            || (!self.mask > 1
                && self.on_link(address)
                && u32::from_be_bytes(address) | self.mask == u32::MAX)
    }

    /// The neighbor a datagram to `address` goes through; `None` when no
    /// route leads there.
    pub(crate) fn next_hop(&self, address: [u8; 4]) -> Option<[u8; 4]> {
        if self.on_link(address) {
            Some(address)
        } else {
            self.gateway
        }
    }
}
