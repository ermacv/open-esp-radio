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
/// use oer_ieee802154::Channel;
/// let invalid = Channel(10);
/// ```
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct Channel(u8);

impl Channel {
    /// Lowest 2.4 GHz channel.
    pub const MIN: u8 = 11;
    /// Highest 2.4 GHz channel.
    pub const MAX: u8 = 26;

    /// Every valid channel in ascending order.
    pub const ALL: [Self; 16] = [
        Self(11),
        Self(12),
        Self(13),
        Self(14),
        Self(15),
        Self(16),
        Self(17),
        Self(18),
        Self(19),
        Self(20),
        Self(21),
        Self(22),
        Self(23),
        Self(24),
        Self(25),
        Self(26),
    ];

    /// Validate one numeric channel.
    pub const fn new(channel: u8) -> Result<Self, ChannelError> {
        if channel >= Self::MIN && channel <= Self::MAX {
            Ok(Self(channel))
        } else {
            Err(ChannelError { channel })
        }
    }

    /// Return the standard channel number.
    pub const fn get(self) -> u8 {
        self.0
    }

    /// Return the zero-based channel index within the 2.4 GHz set.
    pub const fn index(self) -> u8 {
        self.0 - Self::MIN
    }

    /// Return the IEEE center frequency in MHz.
    pub const fn center_frequency_mhz(self) -> u16 {
        2405 + self.index() as u16 * 5
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
        formatter.debug_tuple("Channel").field(&self.0).finish()
    }
}

#[cfg(test)]
mod tests;
