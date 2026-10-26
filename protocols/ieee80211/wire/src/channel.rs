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
pub struct Channel(ChannelIndex);

impl Channel {
    /// A channel of `band`, or why the geometry is invalid.
    pub const fn new(band: Band, number: u8, width: ChannelWidth) -> Result<Self, ChannelError> {
        let mut entries = CHANNELS.as_slice();
        let mut number_exists = false;
        while let Some((entry, remaining)) = entries.split_first() {
            if entry.band as u8 == band as u8 && entry.number == number {
                number_exists = true;
                if entry.width as u8 == width as u8 {
                    return Ok(Self(entry.index));
                }
            }
            entries = remaining;
        }
        if number_exists {
            Err(ChannelError::InvalidWidth {
                band,
                number,
                width,
            })
        } else {
            Err(ChannelError::InvalidNumber { band, number })
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
        Self(channel.0)
    }

    /// The 2.4 GHz definition of this channel; `None` for a 5 GHz channel.
    pub const fn wifi_channel(self) -> Option<WifiChannel> {
        match self.band() {
            Band::Ghz2_4 => Some(WifiChannel(self.0)),
            Band::Ghz5 => None,
        }
    }

    pub const fn band(self) -> Band {
        self.0.entry().band
    }

    /// The primary channel number.
    pub const fn number(self) -> u8 {
        self.0.entry().number
    }
    pub const fn width(self) -> ChannelWidth {
        self.0.entry().width
    }
    pub const fn bandwidth_mhz(self) -> u16 {
        self.width().bandwidth_mhz()
    }
    pub const fn primary_frequency_mhz(self) -> u16 {
        self.0.entry().primary_mhz
    }
    pub const fn center_frequency_mhz(self) -> u16 {
        self.0.entry().center_mhz
    }
}

/// Each fieldless index names exactly one compile-time-validated geometry.
struct ChannelEntry {
    index: ChannelIndex,
    band: Band,
    number: u8,
    width: ChannelWidth,
    span: oer_radio_regulatory::FrequencySpan,
    primary_mhz: u16,
    center_mhz: u16,
}

macro_rules! channel_table {
    (@count $($slot:literal),+) => { <[()]>::len(&[$(channel_table!(@unit $slot)),+]) };
    (@unit $slot:literal) => { () };
    ($($index:ident = $slot:literal: $band:ident, $number:literal, $width:ident;)+) => {
        #[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
        #[repr(u8)]
        enum ChannelIndex { $($index = $slot,)+ }

        impl ChannelIndex {
            const fn entry(self) -> &'static ChannelEntry {
                // Every array subscript is a constant, checked at compilation.
                match self { $(Self::$index => &CHANNELS[$slot],)+ }
            }
        }

        static CHANNELS: [ChannelEntry; channel_table!(@count $($slot),+)] = [$(const {
            let band = Band::$band;
            let number = $number;
            let width = ChannelWidth::$width;
            let primary_mhz: u16 = match band {
                Band::Ghz2_4 if number == 14 => 2_484,
                Band::Ghz2_4 => 2_407 + number as u16 * 5,
                Band::Ghz5 => 5_000 + number as u16 * 5,
            };
            let center_mhz = match width {
                ChannelWidth::Mhz20 => primary_mhz,
                ChannelWidth::Mhz40Above => primary_mhz + 10,
                ChannelWidth::Mhz40Below => primary_mhz - 10,
            };
            let half_khz = width.bandwidth_mhz() as u32 * 500;
            let center_khz = center_mhz as u32 * 1_000;
            let span = match oer_radio_regulatory::FrequencySpan::new(
                center_khz - half_khz, center_khz + half_khz,
            ) {
                Some(span) => span,
                None => panic!("channel table contains an invalid span"),
            };
            ChannelEntry {
                index: ChannelIndex::$index, band, number, width,
                span, primary_mhz, center_mhz,
            }
        },)+];

        const _: () = {
            let mut entries = CHANNELS.as_slice();
            let mut slot = 0;
            let mut wifi_count = 0;
            while let Some((entry, remaining)) = entries.split_first() {
                assert!(entry.index as usize == slot);
                slot += 1;

                // Independent rule admission: IEEE Std 802.11-2020 Table
                // E-4 and 9.4.2.56. Each row must name a permitted primary
                // and secondary-channel relationship, not just a valid span.
                let valid = match entry.band {
                    Band::Ghz2_4 => {
                        wifi_count += 1;
                        match entry.width {
                            ChannelWidth::Mhz20 => entry.number >= 1 && entry.number <= 14,
                            ChannelWidth::Mhz40Above => entry.number >= 1 && entry.number <= 9,
                            ChannelWidth::Mhz40Below => entry.number >= 5 && entry.number <= 13,
                        }
                    }
                    Band::Ghz5 => {
                        let base = match entry.number {
                            36..=64 => 36,
                            100..=144 => 100,
                            149..=177 => 149,
                            _ => panic!("channel outside Table E-4"),
                        };
                        let offset = entry.number - base;
                        offset % 4 == 0 && match entry.width {
                            ChannelWidth::Mhz20 => true,
                            ChannelWidth::Mhz40Above => offset % 8 == 0,
                            ChannelWidth::Mhz40Below => offset % 8 == 4,
                        }
                    }
                };
                assert!(valid);
                let primary = match entry.band {
                    Band::Ghz2_4 if entry.number == 14 => 2_484,
                    Band::Ghz2_4 => 2_407 + 5 * entry.number as u16,
                    Band::Ghz5 => 5_000 + 5 * entry.number as u16,
                };
                let center = match entry.width {
                    ChannelWidth::Mhz20 => primary,
                    ChannelWidth::Mhz40Above => primary + 10,
                    ChannelWidth::Mhz40Below => primary - 10,
                };
                let half_khz = entry.width.bandwidth_mhz() as u32 * 500;
                assert!(entry.primary_mhz == primary && entry.center_mhz == center);
                assert!(entry.span.low_khz() == center as u32 * 1_000 - half_khz);
                assert!(entry.span.high_khz() == center as u32 * 1_000 + half_khz);
                let mut others = remaining;
                while let Some((other, tail)) = others.split_first() {
                    assert!(entry.band as u8 != other.band as u8
                        || entry.number != other.number
                        || entry.width as u8 != other.width as u8);
                    others = tail;
                }
                entries = remaining;
            }
            // Valid, unique rows of these rule-derived cardinalities cover
            // every geometry: 32 in 2.4 GHz and 56 in 5 GHz.
            assert!(wifi_count == 14 + 9 + 9);
            assert!(CHANNELS.len() == 14 + 9 + 9 + 2 * (8 + 12 + 8));
        };
    };
}

// IEEE Std 802.11-2020 Table E-4 (global operating classes), and 9.4.2.56
// (secondary channel offset). The full table is admitted at compilation.
channel_table! {
    C24Ch1Mhz20 = 0: Ghz2_4, 1, Mhz20;
    C24Ch2Mhz20 = 1: Ghz2_4, 2, Mhz20;
    C24Ch3Mhz20 = 2: Ghz2_4, 3, Mhz20;
    C24Ch4Mhz20 = 3: Ghz2_4, 4, Mhz20;
    C24Ch5Mhz20 = 4: Ghz2_4, 5, Mhz20;
    C24Ch6Mhz20 = 5: Ghz2_4, 6, Mhz20;
    C24Ch7Mhz20 = 6: Ghz2_4, 7, Mhz20;
    C24Ch8Mhz20 = 7: Ghz2_4, 8, Mhz20;
    C24Ch9Mhz20 = 8: Ghz2_4, 9, Mhz20;
    C24Ch10Mhz20 = 9: Ghz2_4, 10, Mhz20;
    C24Ch11Mhz20 = 10: Ghz2_4, 11, Mhz20;
    C24Ch12Mhz20 = 11: Ghz2_4, 12, Mhz20;
    C24Ch13Mhz20 = 12: Ghz2_4, 13, Mhz20;
    C24Ch14Mhz20 = 13: Ghz2_4, 14, Mhz20;
    C24Ch1Mhz40Above = 14: Ghz2_4, 1, Mhz40Above;
    C24Ch2Mhz40Above = 15: Ghz2_4, 2, Mhz40Above;
    C24Ch3Mhz40Above = 16: Ghz2_4, 3, Mhz40Above;
    C24Ch4Mhz40Above = 17: Ghz2_4, 4, Mhz40Above;
    C24Ch5Mhz40Above = 18: Ghz2_4, 5, Mhz40Above;
    C24Ch6Mhz40Above = 19: Ghz2_4, 6, Mhz40Above;
    C24Ch7Mhz40Above = 20: Ghz2_4, 7, Mhz40Above;
    C24Ch8Mhz40Above = 21: Ghz2_4, 8, Mhz40Above;
    C24Ch9Mhz40Above = 22: Ghz2_4, 9, Mhz40Above;
    C24Ch5Mhz40Below = 23: Ghz2_4, 5, Mhz40Below;
    C24Ch6Mhz40Below = 24: Ghz2_4, 6, Mhz40Below;
    C24Ch7Mhz40Below = 25: Ghz2_4, 7, Mhz40Below;
    C24Ch8Mhz40Below = 26: Ghz2_4, 8, Mhz40Below;
    C24Ch9Mhz40Below = 27: Ghz2_4, 9, Mhz40Below;
    C24Ch10Mhz40Below = 28: Ghz2_4, 10, Mhz40Below;
    C24Ch11Mhz40Below = 29: Ghz2_4, 11, Mhz40Below;
    C24Ch12Mhz40Below = 30: Ghz2_4, 12, Mhz40Below;
    C24Ch13Mhz40Below = 31: Ghz2_4, 13, Mhz40Below;
    C5Ch36Mhz20 = 32: Ghz5, 36, Mhz20;
    C5Ch40Mhz20 = 33: Ghz5, 40, Mhz20;
    C5Ch44Mhz20 = 34: Ghz5, 44, Mhz20;
    C5Ch48Mhz20 = 35: Ghz5, 48, Mhz20;
    C5Ch52Mhz20 = 36: Ghz5, 52, Mhz20;
    C5Ch56Mhz20 = 37: Ghz5, 56, Mhz20;
    C5Ch60Mhz20 = 38: Ghz5, 60, Mhz20;
    C5Ch64Mhz20 = 39: Ghz5, 64, Mhz20;
    C5Ch100Mhz20 = 40: Ghz5, 100, Mhz20;
    C5Ch104Mhz20 = 41: Ghz5, 104, Mhz20;
    C5Ch108Mhz20 = 42: Ghz5, 108, Mhz20;
    C5Ch112Mhz20 = 43: Ghz5, 112, Mhz20;
    C5Ch116Mhz20 = 44: Ghz5, 116, Mhz20;
    C5Ch120Mhz20 = 45: Ghz5, 120, Mhz20;
    C5Ch124Mhz20 = 46: Ghz5, 124, Mhz20;
    C5Ch128Mhz20 = 47: Ghz5, 128, Mhz20;
    C5Ch132Mhz20 = 48: Ghz5, 132, Mhz20;
    C5Ch136Mhz20 = 49: Ghz5, 136, Mhz20;
    C5Ch140Mhz20 = 50: Ghz5, 140, Mhz20;
    C5Ch144Mhz20 = 51: Ghz5, 144, Mhz20;
    C5Ch149Mhz20 = 52: Ghz5, 149, Mhz20;
    C5Ch153Mhz20 = 53: Ghz5, 153, Mhz20;
    C5Ch157Mhz20 = 54: Ghz5, 157, Mhz20;
    C5Ch161Mhz20 = 55: Ghz5, 161, Mhz20;
    C5Ch165Mhz20 = 56: Ghz5, 165, Mhz20;
    C5Ch169Mhz20 = 57: Ghz5, 169, Mhz20;
    C5Ch173Mhz20 = 58: Ghz5, 173, Mhz20;
    C5Ch177Mhz20 = 59: Ghz5, 177, Mhz20;
    C5Ch36Mhz40Above = 60: Ghz5, 36, Mhz40Above;
    C5Ch40Mhz40Below = 61: Ghz5, 40, Mhz40Below;
    C5Ch44Mhz40Above = 62: Ghz5, 44, Mhz40Above;
    C5Ch48Mhz40Below = 63: Ghz5, 48, Mhz40Below;
    C5Ch52Mhz40Above = 64: Ghz5, 52, Mhz40Above;
    C5Ch56Mhz40Below = 65: Ghz5, 56, Mhz40Below;
    C5Ch60Mhz40Above = 66: Ghz5, 60, Mhz40Above;
    C5Ch64Mhz40Below = 67: Ghz5, 64, Mhz40Below;
    C5Ch100Mhz40Above = 68: Ghz5, 100, Mhz40Above;
    C5Ch104Mhz40Below = 69: Ghz5, 104, Mhz40Below;
    C5Ch108Mhz40Above = 70: Ghz5, 108, Mhz40Above;
    C5Ch112Mhz40Below = 71: Ghz5, 112, Mhz40Below;
    C5Ch116Mhz40Above = 72: Ghz5, 116, Mhz40Above;
    C5Ch120Mhz40Below = 73: Ghz5, 120, Mhz40Below;
    C5Ch124Mhz40Above = 74: Ghz5, 124, Mhz40Above;
    C5Ch128Mhz40Below = 75: Ghz5, 128, Mhz40Below;
    C5Ch132Mhz40Above = 76: Ghz5, 132, Mhz40Above;
    C5Ch136Mhz40Below = 77: Ghz5, 136, Mhz40Below;
    C5Ch140Mhz40Above = 78: Ghz5, 140, Mhz40Above;
    C5Ch144Mhz40Below = 79: Ghz5, 144, Mhz40Below;
    C5Ch149Mhz40Above = 80: Ghz5, 149, Mhz40Above;
    C5Ch153Mhz40Below = 81: Ghz5, 153, Mhz40Below;
    C5Ch157Mhz40Above = 82: Ghz5, 157, Mhz40Above;
    C5Ch161Mhz40Below = 83: Ghz5, 161, Mhz40Below;
    C5Ch165Mhz40Above = 84: Ghz5, 165, Mhz40Above;
    C5Ch169Mhz40Below = 85: Ghz5, 169, Mhz40Below;
    C5Ch173Mhz40Above = 86: Ghz5, 173, Mhz40Above;
    C5Ch177Mhz40Below = 87: Ghz5, 177, Mhz40Below;
}

impl oer_radio_regulatory::RadioChannel for Channel {
    fn span(&self) -> oer_radio_regulatory::FrequencySpan {
        self.0.entry().span
    }
}

impl oer_radio_regulatory::RadioChannel for WifiChannel {
    fn span(&self) -> oer_radio_regulatory::FrequencySpan {
        self.0.entry().span
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
            band: channel.band(),
            number: channel.number(),
        })
    }
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
pub struct WifiChannel(ChannelIndex);

impl WifiChannel {
    /// Construct a 2.4 GHz channel supported by the current radio family.
    ///
    /// Channel 14 is admitted only at 20 MHz. Forty-megahertz definitions
    /// require the complete secondary channel to remain in channels 1..=13.
    pub const fn new_2_4_ghz(
        primary: u8,
        width: WifiChannelWidth,
    ) -> Result<Self, WifiChannelError> {
        match Channel::ghz2_4(primary, width) {
            Ok(channel) => Ok(Self(channel.0)),
            Err(ChannelError::InvalidNumber { .. }) => {
                Err(WifiChannelError::InvalidPrimary(primary))
            }
            Err(ChannelError::InvalidWidth { .. }) => {
                Err(WifiChannelError::InvalidSecondary { primary, width })
            }
        }
    }

    pub const fn mhz20(primary: u8) -> Result<Self, WifiChannelError> {
        Self::new_2_4_ghz(primary, WifiChannelWidth::Mhz20)
    }
    pub const fn primary(self) -> u8 {
        self.0.entry().number
    }
    pub const fn width(self) -> WifiChannelWidth {
        self.0.entry().width
    }
    pub const fn bandwidth_mhz(self) -> u16 {
        self.width().bandwidth_mhz()
    }
    pub const fn primary_frequency_mhz(self) -> u16 {
        self.0.entry().primary_mhz
    }
    pub const fn center_frequency_mhz(self) -> u16 {
        self.0.entry().center_mhz
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
