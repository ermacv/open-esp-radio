//! The laboratory's Wi-Fi settings: the IPv4 policy of a station network and
//! the channel width and security of the fixture access point.

/// The IPv4 policy of a network: DHCP or a static address.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NetworkIpv4Configuration {
    Dhcp,
    Static {
        address: [u8; 4],
        prefix_length: u8,
        gateway: Option<[u8; 4]>,
    },
}

impl NetworkIpv4Configuration {
    /// Whether a static address, its prefix and its gateway are usable.
    pub fn validate(self) -> bool {
        match self {
            Self::Dhcp => true,
            Self::Static {
                address,
                prefix_length,
                gateway,
            } => {
                prefix_length <= 32
                    && address != [0, 0, 0, 0]
                    && address != [255, 255, 255, 255]
                    && match gateway {
                        Some(gateway) => gateway != [0, 0, 0, 0] && gateway != [255, 255, 255, 255],
                        None => true,
                    }
            }
        }
    }
}

/// The channel width of an access point: 20 MHz, or 40 MHz with the
/// secondary channel above or below the primary.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum WifiChannelWidth {
    Mhz20,
    Mhz40Above,
    Mhz40Below,
}

impl WifiChannelWidth {
    pub const fn bandwidth_mhz(self) -> u16 {
        match self {
            Self::Mhz20 => 20,
            Self::Mhz40Above | Self::Mhz40Below => 40,
        }
    }

    /// Whether `channel` can be the primary channel of this width.
    pub const fn admits_primary(self, channel: u8) -> bool {
        match self {
            Self::Mhz20 => channel >= 1 && channel <= 13,
            Self::Mhz40Above => channel >= 1 && channel <= 9,
            Self::Mhz40Below => channel >= 5 && channel <= 13,
        }
    }
}

/// The security of an access point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiAccessPointSecurity {
    Open,
    Wpa2Personal,
}
