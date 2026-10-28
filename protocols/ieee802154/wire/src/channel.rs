use core::fmt;

/// A channel number is outside the 2.4 GHz IEEE 802.15.4 channel set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelError {
    /// Rejected channel number.
    pub channel: u8,
}

/// One validated 2.4 GHz IEEE 802.15.4 channel, 11 through 26 inclusive.
///
/// Direct tuple construction is intentionally unavailable:
///
/// ```compile_fail
/// use oer_ieee802154_wire::Channel;
/// let invalid = Channel(10);
/// ```
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Channel(ChannelNumber);

// Exhaustive variants retain the admitted channel set without a runtime
// impossible branch in infallible frequency and regulatory queries.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
enum ChannelNumber {
    C11 = 11,
    C12 = 12,
    C13 = 13,
    C14 = 14,
    C15 = 15,
    C16 = 16,
    C17 = 17,
    C18 = 18,
    C19 = 19,
    C20 = 20,
    C21 = 21,
    C22 = 22,
    C23 = 23,
    C24 = 24,
    C25 = 25,
    C26 = 26,
}

impl Channel {
    /// Lowest 2.4 GHz channel.
    pub const MIN: u8 = 11;
    /// Highest 2.4 GHz channel.
    pub const MAX: u8 = 26;

    /// Every valid channel in ascending order.
    pub const ALL: [Self; 16] = [
        Self(ChannelNumber::C11),
        Self(ChannelNumber::C12),
        Self(ChannelNumber::C13),
        Self(ChannelNumber::C14),
        Self(ChannelNumber::C15),
        Self(ChannelNumber::C16),
        Self(ChannelNumber::C17),
        Self(ChannelNumber::C18),
        Self(ChannelNumber::C19),
        Self(ChannelNumber::C20),
        Self(ChannelNumber::C21),
        Self(ChannelNumber::C22),
        Self(ChannelNumber::C23),
        Self(ChannelNumber::C24),
        Self(ChannelNumber::C25),
        Self(ChannelNumber::C26),
    ];

    /// Validate one numeric channel.
    pub const fn new(channel: u8) -> Result<Self, ChannelError> {
        let number = match channel {
            11 => ChannelNumber::C11,
            12 => ChannelNumber::C12,
            13 => ChannelNumber::C13,
            14 => ChannelNumber::C14,
            15 => ChannelNumber::C15,
            16 => ChannelNumber::C16,
            17 => ChannelNumber::C17,
            18 => ChannelNumber::C18,
            19 => ChannelNumber::C19,
            20 => ChannelNumber::C20,
            21 => ChannelNumber::C21,
            22 => ChannelNumber::C22,
            23 => ChannelNumber::C23,
            24 => ChannelNumber::C24,
            25 => ChannelNumber::C25,
            26 => ChannelNumber::C26,
            _ => return Err(ChannelError { channel }),
        };
        Ok(Self(number))
    }

    /// Return the standard channel number.
    pub const fn get(self) -> u8 {
        self.0 as u8
    }

    /// Return the zero-based channel index within the 2.4 GHz set.
    pub const fn index(self) -> u8 {
        match self.0 {
            ChannelNumber::C11 => 0,
            ChannelNumber::C12 => 1,
            ChannelNumber::C13 => 2,
            ChannelNumber::C14 => 3,
            ChannelNumber::C15 => 4,
            ChannelNumber::C16 => 5,
            ChannelNumber::C17 => 6,
            ChannelNumber::C18 => 7,
            ChannelNumber::C19 => 8,
            ChannelNumber::C20 => 9,
            ChannelNumber::C21 => 10,
            ChannelNumber::C22 => 11,
            ChannelNumber::C23 => 12,
            ChannelNumber::C24 => 13,
            ChannelNumber::C25 => 14,
            ChannelNumber::C26 => 15,
        }
    }

    /// Return the IEEE center frequency in MHz.
    pub const fn center_frequency_mhz(self) -> u16 {
        match self.0 {
            ChannelNumber::C11 => 2405,
            ChannelNumber::C12 => 2410,
            ChannelNumber::C13 => 2415,
            ChannelNumber::C14 => 2420,
            ChannelNumber::C15 => 2425,
            ChannelNumber::C16 => 2430,
            ChannelNumber::C17 => 2435,
            ChannelNumber::C18 => 2440,
            ChannelNumber::C19 => 2445,
            ChannelNumber::C20 => 2450,
            ChannelNumber::C21 => 2455,
            ChannelNumber::C22 => 2460,
            ChannelNumber::C23 => 2465,
            ChannelNumber::C24 => 2470,
            ChannelNumber::C25 => 2475,
            ChannelNumber::C26 => 2480,
        }
    }
}

impl oer_radio_regulatory::RadioChannel for Channel {
    /// The 2 MHz the channel occupies around its center.
    fn span(&self) -> oer_radio_regulatory::FrequencySpan {
        match self.0 {
            ChannelNumber::C11 => span::<2405000>(),
            ChannelNumber::C12 => span::<2410000>(),
            ChannelNumber::C13 => span::<2415000>(),
            ChannelNumber::C14 => span::<2420000>(),
            ChannelNumber::C15 => span::<2425000>(),
            ChannelNumber::C16 => span::<2430000>(),
            ChannelNumber::C17 => span::<2435000>(),
            ChannelNumber::C18 => span::<2440000>(),
            ChannelNumber::C19 => span::<2445000>(),
            ChannelNumber::C20 => span::<2450000>(),
            ChannelNumber::C21 => span::<2455000>(),
            ChannelNumber::C22 => span::<2460000>(),
            ChannelNumber::C23 => span::<2465000>(),
            ChannelNumber::C24 => span::<2470000>(),
            ChannelNumber::C25 => span::<2475000>(),
            ChannelNumber::C26 => span::<2480000>(),
        }
    }
}

fn span<const CENTER_KHZ: u32>() -> oer_radio_regulatory::FrequencySpan {
    const {
        match oer_radio_regulatory::FrequencySpan::new(CENTER_KHZ - 1000, CENTER_KHZ + 1000) {
            Some(span) => span,
            None => panic!("a channel has a width"),
        }
    }
}

impl TryFrom<u8> for Channel {
    type Error = ChannelError;

    fn try_from(channel: u8) -> Result<Self, Self::Error> {
        Self::new(channel)
    }
}

impl From<Channel> for u8 {
    fn from(channel: Channel) -> Self {
        channel.get()
    }
}

impl fmt::Debug for Channel {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_tuple("Channel").field(&self.get()).finish()
    }
}

#[cfg(test)]
mod tests;
