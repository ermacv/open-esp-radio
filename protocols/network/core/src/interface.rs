//! The one interface of this core and its IPv4 addressing.

/// The all-stations address.
pub(crate) const BROADCAST_HARDWARE: [u8; 6] = [0xFF; 6];
/// The limited broadcast address, RFC 1122, 3.2.1.3 (c).
pub(crate) const BROADCAST: [u8; 4] = [0xFF; 4];
/// RFC 1122, 3.2.1.3 (a): this host on this network, before it has an
/// address.
pub(crate) const UNSPECIFIED: [u8; 4] = [0; 4];

/// An interface's identity, and how it gets its address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Interface {
    hardware: [u8; 6],
    mtu: u16,
    addressing: Addressing,
}

/// How an interface gets its address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Addressing {
    /// Its owner gave it.
    Static(Ipv4),
    /// The core's DHCP client leases it; `seed` starts the client's
    /// transaction identifiers and retransmission jitter.
    Dhcp { seed: u64 },
}

/// One IPv4 address on the interface's network, its prefix and its gateway.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ipv4 {
    address: [u8; 4],
    prefix: u8,
    mask: u32,
    gateway: Option<[u8; 4]>,
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

impl Ipv4 {
    /// # Errors
    /// [`InterfaceError::Prefix`] or [`InterfaceError::Gateway`].
    pub fn new(
        address: [u8; 4],
        prefix: u8,
        gateway: Option<[u8; 4]>,
    ) -> Result<Self, InterfaceError> {
        if prefix > 32 {
            return Err(InterfaceError::Prefix(prefix));
        }
        let mask = u32::MAX
            .checked_shl(32_u32.wrapping_sub(u32::from(prefix)))
            .unwrap_or(0);
        let ipv4 = Self {
            address,
            prefix,
            mask,
            gateway,
        };
        if gateway.is_some_and(|gateway| !ipv4.on_link(gateway)) {
            return Err(InterfaceError::Gateway);
        }
        Ok(ipv4)
    }

    pub const fn address(&self) -> [u8; 4] {
        self.address
    }

    pub const fn prefix(&self) -> u8 {
        self.prefix
    }

    pub const fn gateway(&self) -> Option<[u8; 4]> {
        self.gateway
    }

    fn on_link(&self, address: [u8; 4]) -> bool {
        (u32::from_be_bytes(address) ^ u32::from_be_bytes(self.address)) & self.mask == 0
    }

    /// The network's own broadcast address. Prefixes of 31 and 32 bits have
    /// none, RFC 3021.
    pub(crate) fn is_directed_broadcast(&self, address: [u8; 4]) -> bool {
        !self.mask > 1
            && self.on_link(address)
            && u32::from_be_bytes(address) | self.mask == u32::MAX
    }

    /// The network's own number, all host bits zero, RFC 1122, 3.2.1.3 (b),
    /// for prefixes with host bits to spare, RFC 3021.
    pub(crate) fn is_network(&self, address: [u8; 4]) -> bool {
        !self.mask > 1 && self.on_link(address) && u32::from_be_bytes(address) & !self.mask == 0
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

impl Interface {
    /// An interface whose address its owner gives.
    ///
    /// # Errors
    /// [`InterfaceError`]: what is not valid.
    pub fn new(
        hardware: [u8; 6],
        address: [u8; 4],
        prefix: u8,
        gateway: Option<[u8; 4]>,
        mtu: u16,
    ) -> Result<Self, InterfaceError> {
        let ipv4 = Ipv4::new(address, prefix, gateway)?;
        Self::with(hardware, mtu, Addressing::Static(ipv4))
    }

    /// An interface whose address the core's DHCP client leases, RFC 2131.
    /// `seed` is drawn by the image, from its entropy: it starts the client's
    /// transaction identifiers and retransmission jitter.
    ///
    /// # Errors
    /// [`InterfaceError::Mtu`].
    pub fn dhcp(hardware: [u8; 6], mtu: u16, seed: u64) -> Result<Self, InterfaceError> {
        Self::with(hardware, mtu, Addressing::Dhcp { seed })
    }

    const fn with(
        hardware: [u8; 6],
        mtu: u16,
        addressing: Addressing,
    ) -> Result<Self, InterfaceError> {
        if mtu < 68 {
            return Err(InterfaceError::Mtu(mtu));
        }
        Ok(Self {
            hardware,
            mtu,
            addressing,
        })
    }

    pub(crate) const fn hardware(&self) -> [u8; 6] {
        self.hardware
    }

    pub(crate) const fn mtu(&self) -> u16 {
        self.mtu
    }

    pub(crate) const fn addressing(&self) -> Addressing {
        self.addressing
    }
}

/// Whether `address` may be a host's own: not this network's zero address,
/// not a broadcast, loopback, multicast or reserved one, RFC 1122, 3.2.1.3,
/// and RFC 5771 for multicast.
pub(crate) fn is_unicast(address: [u8; 4]) -> bool {
    let [first, ..] = address;
    first != 0 && first != 127 && first < 224
}
