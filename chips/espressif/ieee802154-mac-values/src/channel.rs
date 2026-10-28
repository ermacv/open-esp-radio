// SPDX-License-Identifier: Apache-2.0
//! IEEE 802.15.4 2.4 GHz channels and their MAC frequency codes.
//!
//! Ported from ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/ieee802154/driver` and its public MAC low-level vocabulary
//! (Copyright Espressif Systems (Shanghai) Co Ltd, Apache-2.0)
//! and modified by Vladislav Ermachenkov; see the root `NOTICE`.

/// Lowest IEEE 802.15.4 channel supported by the 2.4 GHz PHY.
pub const IEEE802154_MIN_CHANNEL: u8 = 11;

/// Highest IEEE 802.15.4 channel supported by the 2.4 GHz PHY.
pub const IEEE802154_MAX_CHANNEL: u8 = 26;

/// One checked IEEE 802.15.4 2.4 GHz channel.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Ieee802154Channel(ChannelNumber);

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
enum ChannelNumber {
    Channel11 = 11,
    Channel12 = 12,
    Channel13 = 13,
    Channel14 = 14,
    Channel15 = 15,
    Channel16 = 16,
    Channel17 = 17,
    Channel18 = 18,
    Channel19 = 19,
    Channel20 = 20,
    Channel21 = 21,
    Channel22 = 22,
    Channel23 = 23,
    Channel24 = 24,
    Channel25 = 25,
    Channel26 = 26,
}

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
        match channel {
            11 => Ok(Self(ChannelNumber::Channel11)),
            12 => Ok(Self(ChannelNumber::Channel12)),
            13 => Ok(Self(ChannelNumber::Channel13)),
            14 => Ok(Self(ChannelNumber::Channel14)),
            15 => Ok(Self(ChannelNumber::Channel15)),
            16 => Ok(Self(ChannelNumber::Channel16)),
            17 => Ok(Self(ChannelNumber::Channel17)),
            18 => Ok(Self(ChannelNumber::Channel18)),
            19 => Ok(Self(ChannelNumber::Channel19)),
            20 => Ok(Self(ChannelNumber::Channel20)),
            21 => Ok(Self(ChannelNumber::Channel21)),
            22 => Ok(Self(ChannelNumber::Channel22)),
            23 => Ok(Self(ChannelNumber::Channel23)),
            24 => Ok(Self(ChannelNumber::Channel24)),
            25 => Ok(Self(ChannelNumber::Channel25)),
            26 => Ok(Self(ChannelNumber::Channel26)),
            _ => Err(Ieee802154ChannelError { attempted: channel }),
        }
    }

    /// Return the standardized channel number.
    pub const fn number(self) -> u8 {
        self.0 as u8
    }

    /// The MAC frequency code of this channel.
    ///
    /// The pinned public vendor utility (`ieee802154_channel_to_freq`) maps
    /// channels 11 through 26 to codes 3 through 78 with
    /// `(channel - 11) * 5 + 3`; every code fits the narrowest frequency
    /// field of a supported chip (seven bits).
    pub const fn frequency_code(self) -> u8 {
        self.properties().0
    }

    /// `ieee802154_freq_to_channel`: the channel whose frequency code is
    /// `code`, or `None` for a code the vendor utility asserts against.
    pub const fn from_frequency_code(code: u8) -> Option<Self> {
        let Some(offset) = code.checked_sub(3) else {
            return None;
        };
        if !offset.is_multiple_of(5) {
            return None;
        }
        let Some(channel_offset) = offset.checked_div(5) else {
            return None;
        };
        let Some(channel) = channel_offset.checked_add(IEEE802154_MIN_CHANNEL) else {
            return None;
        };
        match Self::new(channel) {
            Ok(channel) => Some(channel),
            Err(_) => None,
        }
    }

    const fn properties(self) -> (u8, oer_radio_regulatory::FrequencySpan) {
        match self.0 {
            ChannelNumber::Channel11 => channel_properties::<11>(),
            ChannelNumber::Channel12 => channel_properties::<12>(),
            ChannelNumber::Channel13 => channel_properties::<13>(),
            ChannelNumber::Channel14 => channel_properties::<14>(),
            ChannelNumber::Channel15 => channel_properties::<15>(),
            ChannelNumber::Channel16 => channel_properties::<16>(),
            ChannelNumber::Channel17 => channel_properties::<17>(),
            ChannelNumber::Channel18 => channel_properties::<18>(),
            ChannelNumber::Channel19 => channel_properties::<19>(),
            ChannelNumber::Channel20 => channel_properties::<20>(),
            ChannelNumber::Channel21 => channel_properties::<21>(),
            ChannelNumber::Channel22 => channel_properties::<22>(),
            ChannelNumber::Channel23 => channel_properties::<23>(),
            ChannelNumber::Channel24 => channel_properties::<24>(),
            ChannelNumber::Channel25 => channel_properties::<25>(),
            ChannelNumber::Channel26 => channel_properties::<26>(),
        }
    }
}

// Each closed channel selects a compile-time calculation. No raw channel or
// frequency arithmetic, or fallible span construction, runs on the radio path.
const fn channel_properties<const CHANNEL: u8>() -> (u8, oer_radio_regulatory::FrequencySpan) {
    const {
        if CHANNEL < IEEE802154_MIN_CHANNEL || CHANNEL > IEEE802154_MAX_CHANNEL {
            panic!("channel properties require a supported 2.4 GHz channel");
        }
        let offset = CHANNEL - IEEE802154_MIN_CHANNEL;
        let frequency_code = offset * 5 + 3;
        let center_khz = (2405 + 5 * offset as u32) * 1000;
        let span =
            match oer_radio_regulatory::FrequencySpan::new(center_khz - 1000, center_khz + 1000) {
                Some(span) => span,
                None => panic!("a channel has a width"),
            };
        (frequency_code, span)
    }
}

impl oer_radio_regulatory::RadioChannel for Ieee802154Channel {
    /// The 2 MHz around the channel's center, `2405 + 5 * (channel - 11)` MHz.
    fn span(&self) -> oer_radio_regulatory::FrequencySpan {
        self.properties().1
    }
}

impl TryFrom<u8> for Ieee802154Channel {
    type Error = Ieee802154ChannelError;

    fn try_from(channel: u8) -> Result<Self, Self::Error> {
        Self::new(channel)
    }
}

#[cfg(test)]
#[allow(
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    reason = "tests assert behavior and compare with reference arithmetic"
)]
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

    #[test]
    fn every_frequency_code_is_either_an_exact_channel_or_refused() {
        for code in u8::MIN..=u8::MAX {
            let expected = (IEEE802154_MIN_CHANNEL..=IEEE802154_MAX_CHANNEL)
                .map(|number| Ieee802154Channel::new(number).unwrap())
                .find(|channel| channel.frequency_code() == code);
            assert_eq!(Ieee802154Channel::from_frequency_code(code), expected);
        }
    }

    #[test]
    fn every_channel_retains_its_two_megahertz_regulatory_span() {
        use oer_radio_regulatory::RadioChannel;
        for number in IEEE802154_MIN_CHANNEL..=IEEE802154_MAX_CHANNEL {
            let channel = Ieee802154Channel::new(number).unwrap();
            let center_khz = (2405 + 5 * u32::from(number - IEEE802154_MIN_CHANNEL)) * 1000;
            assert_eq!(channel.span().low_khz(), center_khz - 1000);
            assert_eq!(channel.span().high_khz(), center_khz + 1000);
            assert_eq!(channel.span().width_khz(), 2000);
        }
    }
}
