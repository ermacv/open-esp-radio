//! IEEE 802.15.4 2.4 GHz channels and their MAC frequency codes.

/// Lowest IEEE 802.15.4 channel supported by the 2.4 GHz PHY.
pub const IEEE802154_MIN_CHANNEL: u8 = 11;

/// Highest IEEE 802.15.4 channel supported by the 2.4 GHz PHY.
pub const IEEE802154_MAX_CHANNEL: u8 = 26;

/// One checked IEEE 802.15.4 2.4 GHz channel.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154Channel(u8);

/// An integer outside the IEEE 802.15.4 2.4 GHz channel range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ieee802154ChannelError {
    attempted: u8,
}

impl Ieee802154ChannelError {
    /// Return the rejected channel number.
    pub const fn attempted(self) -> u8 {
        self.attempted
    }
}

impl Ieee802154Channel {
    /// Check and construct one channel in the inclusive range 11 through 26.
    pub const fn new(channel: u8) -> Result<Self, Ieee802154ChannelError> {
        if channel >= IEEE802154_MIN_CHANNEL && channel <= IEEE802154_MAX_CHANNEL {
            Ok(Self(channel))
        } else {
            Err(Ieee802154ChannelError { attempted: channel })
        }
    }

    /// Return the standardized channel number.
    pub const fn number(self) -> u8 {
        self.0
    }

    /// The MAC frequency code of this channel.
    ///
    /// The pinned public vendor utility (`ieee802154_channel_to_freq`) maps
    /// channels 11 through 26 to codes 3 through 78 with
    /// `(channel - 11) * 5 + 3`; every code fits the narrowest frequency
    /// field of a supported chip (seven bits).
    pub const fn frequency_code(self) -> u8 {
        (self.0 - IEEE802154_MIN_CHANNEL) * 5 + 3
    }

    /// `ieee802154_freq_to_channel`: the channel whose frequency code is
    /// `code`, or `None` for a code the vendor utility asserts against.
    pub const fn from_frequency_code(code: u8) -> Option<Self> {
        if code < 3 || !(code - 3).is_multiple_of(5) {
            return None;
        }
        match Self::new((code - 3) / 5 + IEEE802154_MIN_CHANNEL) {
            Ok(channel) => Some(channel),
            Err(_) => None,
        }
    }
}

impl TryFrom<u8> for Ieee802154Channel {
    type Error = Ieee802154ChannelError;

    fn try_from(channel: u8) -> Result<Self, Self::Error> {
        Self::new(channel)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_channel_maps_to_the_reviewed_vendor_frequency_code() {
        for number in IEEE802154_MIN_CHANNEL..=IEEE802154_MAX_CHANNEL {
            let channel = Ieee802154Channel::new(number).expect("2.4 GHz channel");
            assert_eq!(
                channel.frequency_code(),
                (number - IEEE802154_MIN_CHANNEL) * 5 + 3,
            );
        }

        assert_eq!(
            Ieee802154Channel::new(IEEE802154_MIN_CHANNEL)
                .expect("lower boundary")
                .frequency_code(),
            3
        );
        assert_eq!(
            Ieee802154Channel::new(IEEE802154_MAX_CHANNEL)
                .expect("upper boundary")
                .frequency_code(),
            78
        );
    }

    /// `ieee802154_freq_to_channel` inverts `ieee802154_channel_to_freq` and
    /// rejects every other code.
    #[test]
    fn a_frequency_code_names_a_channel_only_on_the_five_code_grid() {
        for number in IEEE802154_MIN_CHANNEL..=IEEE802154_MAX_CHANNEL {
            let channel = Ieee802154Channel::new(number).unwrap();
            assert_eq!(
                Ieee802154Channel::from_frequency_code(channel.frequency_code()),
                Some(channel)
            );
        }
        for code in [0, 2, 4, 7, 83, 255] {
            assert_eq!(Ieee802154Channel::from_frequency_code(code), None);
        }
    }

    #[test]
    fn channel_constructor_is_exhaustive_and_fail_closed() {
        for candidate in u8::MIN..=u8::MAX {
            let result = Ieee802154Channel::new(candidate);
            if (IEEE802154_MIN_CHANNEL..=IEEE802154_MAX_CHANNEL).contains(&candidate) {
                assert_eq!(result.map(Ieee802154Channel::number), Ok(candidate));
            } else {
                assert_eq!(
                    result,
                    Err(Ieee802154ChannelError {
                        attempted: candidate,
                    })
                );
            }
        }
    }
}
