//! Typed IEEE 802.11 channel definitions.
//!
//! Protocol and application code uses a primary channel plus an explicit
//! bandwidth relationship. Chip PHY leaves may lower this value into their
//! recovered register encodings, but those encodings do not cross this
//! portable boundary.
//!
//! [`Channel`] names a channel in either band a radio port serves;
//! [`WifiChannel`] is the 2.4 GHz channel the station and access-point
//! owners configure today, and converts into it without loss.

use core::fmt;

/// Channel width and secondary-channel relationship of one channel.
///
/// Forty-megahertz channels name the side of the primary channel their
/// secondary twenty-megahertz channel lies on, as the HT Operation element
/// does (IEEE 802.11-2020 9.4.2.56).
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ChannelWidth {
    Mhz20,
    /// Forty megahertz with the secondary channel above the primary.
    Mhz40Above,
    /// Forty megahertz with the secondary channel below the primary.
    Mhz40Below,
}

impl ChannelWidth {
    pub const fn bandwidth_mhz(self) -> u16 {
        match self {
            Self::Mhz20 => 20,
            Self::Mhz40Above | Self::Mhz40Below => 40,
        }
    }
}

/// The width of a 2.4 GHz [`WifiChannel`]: the same values as [`ChannelWidth`].
pub type WifiChannelWidth = ChannelWidth;

/// Frequency band of a channel.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Band {
    /// The 2.4 GHz band, channels 1 through 14.
    Ghz2_4,
    /// The 5 GHz band.
    Ghz5,
}

/// Validated IEEE 802.11 channel in the 2.4 GHz or 5 GHz band.
///
/// 2.4 GHz channels follow [`WifiChannel`]: channels 1 through 14, channel 14
/// at 20 MHz only and 40 MHz only where the secondary channel stays in 1
/// through 13. 5 GHz channels are those of the global operating classes of
/// IEEE 802.11-2020 Table E-4: 20 MHz channels 36-64 and 100-144 in steps
/// of four (classes 115, 118, 121) and 149-177 in steps of four (class 125);
/// 40 MHz channels pair them as 36/40, 44/48, …, 140/144, 149/153, …,
/// 173/177, with the lower channel of a pair as the primary of
/// [`ChannelWidth::Mhz40Above`] (classes 116, 119, 122, 126) and the upper
/// one as the primary of [`ChannelWidth::Mhz40Below`] (classes 117, 120,
/// 123, 127). Regulatory domains admit subsets of these; the channel does
/// not encode a regulatory decision.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Channel {
    band: Band,
    number: u8,
    width: ChannelWidth,
}

impl Channel {
    /// A channel of `band`, or why the geometry is invalid.
    pub const fn new(band: Band, number: u8, width: ChannelWidth) -> Result<Self, ChannelError> {
        match band {
            Band::Ghz2_4 => match WifiChannel::new_2_4_ghz(number, width) {
                Ok(channel) => Ok(Self::from_wifi_channel(channel)),
                Err(WifiChannelError::InvalidPrimary(number)) => {
                    Err(ChannelError::InvalidNumber { band, number })
                }
                Err(WifiChannelError::InvalidSecondary { primary, width }) => {
                    Err(ChannelError::InvalidWidth {
                        band,
                        number: primary,
                        width,
                    })
                }
            },
            Band::Ghz5 => {
                let Some(lower_of_pair) = ghz5_pair_position(number) else {
                    return Err(ChannelError::InvalidNumber { band, number });
                };
                let valid = match width {
                    ChannelWidth::Mhz20 => true,
                    ChannelWidth::Mhz40Above => lower_of_pair,
                    ChannelWidth::Mhz40Below => !lower_of_pair,
                };
                if valid {
                    Ok(Self {
                        band,
                        number,
                        width,
                    })
                } else {
                    Err(ChannelError::InvalidWidth {
                        band,
                        number,
                        width,
                    })
                }
            }
        }
    }

    /// A 2.4 GHz channel.
    pub const fn ghz2_4(number: u8, width: ChannelWidth) -> Result<Self, ChannelError> {
        Self::new(Band::Ghz2_4, number, width)
    }

    /// A 5 GHz channel.
    pub const fn ghz5(number: u8, width: ChannelWidth) -> Result<Self, ChannelError> {
        Self::new(Band::Ghz5, number, width)
    }

    /// The channel of a validated 2.4 GHz definition.
    pub const fn from_wifi_channel(channel: WifiChannel) -> Self {
        Self {
            band: Band::Ghz2_4,
            number: channel.primary(),
            width: channel.width(),
        }
    }

    /// The 2.4 GHz definition of this channel; `None` for a 5 GHz channel.
    pub const fn wifi_channel(self) -> Option<WifiChannel> {
        match self.band {
            Band::Ghz2_4 => match WifiChannel::new_2_4_ghz(self.number, self.width) {
                Ok(channel) => Some(channel),
                Err(_) => None,
            },
            Band::Ghz5 => None,
        }
    }

    pub const fn band(self) -> Band {
        self.band
    }

    /// The primary channel number.
    pub const fn number(self) -> u8 {
        self.number
    }

    pub const fn width(self) -> ChannelWidth {
        self.width
    }

    pub const fn bandwidth_mhz(self) -> u16 {
        self.width.bandwidth_mhz()
    }

    pub const fn primary_frequency_mhz(self) -> u16 {
        match self.band {
            Band::Ghz2_4 if self.number == 14 => 2_484,
            Band::Ghz2_4 => 2_407 + self.number as u16 * 5,
            Band::Ghz5 => 5_000 + self.number as u16 * 5,
        }
    }

    pub const fn center_frequency_mhz(self) -> u16 {
        match self.width {
            ChannelWidth::Mhz20 => self.primary_frequency_mhz(),
            ChannelWidth::Mhz40Above => self.primary_frequency_mhz() + 10,
            ChannelWidth::Mhz40Below => self.primary_frequency_mhz() - 10,
        }
    }
}

impl From<WifiChannel> for Channel {
    fn from(channel: WifiChannel) -> Self {
        Self::from_wifi_channel(channel)
    }
}

impl TryFrom<Channel> for WifiChannel {
    type Error = ChannelError;

    /// The 2.4 GHz definition; a 5 GHz channel has none.
    fn try_from(channel: Channel) -> Result<Self, ChannelError> {
        channel.wifi_channel().ok_or(ChannelError::InvalidNumber {
            band: channel.band,
            number: channel.number,
        })
    }
}

/// Whether a 5 GHz channel number is the lower (`Some(true)`) or upper
/// (`Some(false)`) channel of its 40 MHz pair; `None` for a number no
/// global operating class defines.
const fn ghz5_pair_position(number: u8) -> Option<bool> {
    let base = match number {
        36..=64 => 36,
        100..=144 => 100,
        149..=177 => 149,
        _ => return None,
    };
    let offset = number - base;
    if !offset.is_multiple_of(4) {
        return None;
    }
    Some(offset.is_multiple_of(8))
}

/// Invalid channel geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelError {
    /// No channel of this number exists in the band.
    InvalidNumber { band: Band, number: u8 },
    /// The channel exists but not with this width.
    InvalidWidth {
        band: Band,
        number: u8,
        width: ChannelWidth,
    },
}

impl fmt::Display for ChannelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidNumber { band, number } => {
                write!(formatter, "no {band:?} channel {number}")
            }
            Self::InvalidWidth {
                band,
                number,
                width,
            } => write!(
                formatter,
                "{band:?} channel {number} cannot use {width:?} geometry"
            ),
        }
    }
}

impl core::error::Error for ChannelError {}

/// Validated IEEE 802.11 2.4 GHz channel definition.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiChannel {
    primary: u8,
    width: WifiChannelWidth,
}

impl WifiChannel {
    /// Construct a 2.4 GHz channel supported by the current radio family.
    ///
    /// Channel 14 is admitted only at 20 MHz. Forty-megahertz definitions
    /// require the complete secondary channel to remain in channels 1..=13.
    pub const fn new_2_4_ghz(
        primary: u8,
        width: WifiChannelWidth,
    ) -> Result<Self, WifiChannelError> {
        if primary == 0 || primary > 14 {
            return Err(WifiChannelError::InvalidPrimary(primary));
        }
        let secondary_valid = match width {
            WifiChannelWidth::Mhz20 => true,
            WifiChannelWidth::Mhz40Above => primary <= 9,
            WifiChannelWidth::Mhz40Below => primary >= 5 && primary <= 13,
        };
        if !secondary_valid {
            return Err(WifiChannelError::InvalidSecondary { primary, width });
        }
        Ok(Self { primary, width })
    }

    pub const fn mhz20(primary: u8) -> Result<Self, WifiChannelError> {
        Self::new_2_4_ghz(primary, WifiChannelWidth::Mhz20)
    }

    pub const fn primary(self) -> u8 {
        self.primary
    }

    pub const fn width(self) -> WifiChannelWidth {
        self.width
    }

    pub const fn bandwidth_mhz(self) -> u16 {
        match self.width {
            WifiChannelWidth::Mhz20 => 20,
            WifiChannelWidth::Mhz40Above | WifiChannelWidth::Mhz40Below => 40,
        }
    }

    pub const fn primary_frequency_mhz(self) -> u16 {
        if self.primary == 14 {
            2_484
        } else {
            2_407 + self.primary as u16 * 5
        }
    }

    pub const fn center_frequency_mhz(self) -> u16 {
        match self.width {
            WifiChannelWidth::Mhz20 => self.primary_frequency_mhz(),
            WifiChannelWidth::Mhz40Above => self.primary_frequency_mhz() + 10,
            WifiChannelWidth::Mhz40Below => self.primary_frequency_mhz() - 10,
        }
    }
}

/// Invalid portable channel geometry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiChannelError {
    InvalidPrimary(u8),
    InvalidSecondary {
        primary: u8,
        width: WifiChannelWidth,
    },
}

impl fmt::Display for WifiChannelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidPrimary(primary) => {
                write!(formatter, "invalid 2.4 GHz primary channel {primary}")
            }
            Self::InvalidSecondary { primary, width } => write!(
                formatter,
                "primary channel {primary} cannot use {width:?} secondary-channel geometry"
            ),
        }
    }
}

impl core::error::Error for WifiChannelError {}

#[cfg(test)]
mod tests;
