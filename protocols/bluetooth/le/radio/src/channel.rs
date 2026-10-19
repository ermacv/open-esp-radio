//! Link Layer channel indices.

/// Why a channel index is outside its range.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelError {
    /// The rejected index.
    pub index: u8,
}

/// One primary advertising channel.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AdvertisingChannel {
    /// Channel index 37.
    Channel37,
    /// Channel index 38.
    Channel38,
    /// Channel index 39.
    Channel39,
}

impl AdvertisingChannel {
    /// The Link Layer channel index.
    pub const fn index(self) -> u8 {
        match self {
            Self::Channel37 => 37,
            Self::Channel38 => 38,
            Self::Channel39 => 39,
        }
    }

    const fn bit(self) -> u8 {
        1 << (self.index() - 37)
    }
}

/// A non-empty set of primary advertising channels, used in index order.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct AdvertisingChannels(u8);

impl AdvertisingChannels {
    /// All three primary channels.
    pub const ALL: Self = Self(0b111);

    /// The set of the selected channels, or `None` when none is selected.
    pub const fn new(channel_37: bool, channel_38: bool, channel_39: bool) -> Option<Self> {
        let bits = (channel_37 as u8) | (channel_38 as u8) << 1 | (channel_39 as u8) << 2;
        if bits == 0 { None } else { Some(Self(bits)) }
    }

    /// The set holding one channel.
    pub const fn single(channel: AdvertisingChannel) -> Self {
        Self(channel.bit())
    }

    /// Whether the set holds `channel`.
    pub const fn contains(self, channel: AdvertisingChannel) -> bool {
        self.0 & channel.bit() != 0
    }

    /// Number of channels in the set.
    pub const fn len(self) -> usize {
        self.0.count_ones() as usize
    }

    /// Whether the set is empty; a constructed set never is.
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// The channels in index order.
    pub fn iter(self) -> impl Iterator<Item = AdvertisingChannel> {
        [
            AdvertisingChannel::Channel37,
            AdvertisingChannel::Channel38,
            AdvertisingChannel::Channel39,
        ]
        .into_iter()
        .filter(move |channel| self.contains(*channel))
    }
}

/// One of the 37 data channels.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct DataChannel(u8);

impl DataChannel {
    /// Validate a data channel index.
    pub const fn new(index: u8) -> Result<Self, ChannelError> {
        if index < 37 {
            Ok(Self(index))
        } else {
            Err(ChannelError { index })
        }
    }

    /// The Link Layer channel index.
    pub const fn index(self) -> u8 {
        self.0
    }
}

/// One of the 40 RF channels addressed by Direct Test Mode, `(f - 2402) / 2`.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TestChannel(u8);

impl TestChannel {
    /// Validate an RF channel.
    pub const fn new(rf_channel: u8) -> Result<Self, ChannelError> {
        if rf_channel < 40 {
            Ok(Self(rf_channel))
        } else {
            Err(ChannelError { index: rf_channel })
        }
    }

    /// The RF channel.
    pub const fn rf_channel(self) -> u8 {
        self.0
    }
}

#[cfg(test)]
mod tests {
    use std::vec::Vec;

    use super::{AdvertisingChannel, AdvertisingChannels, ChannelError, DataChannel, TestChannel};

    #[test]
    fn channel_sets_are_non_empty_and_ordered() {
        assert_eq!(AdvertisingChannels::new(false, false, false), None);
        let set = AdvertisingChannels::new(true, false, true).unwrap();
        assert_eq!(set.len(), 2);
        assert_eq!(
            set.iter().collect::<Vec<_>>(),
            [AdvertisingChannel::Channel37, AdvertisingChannel::Channel39]
        );
        assert_eq!(
            AdvertisingChannels::single(AdvertisingChannel::Channel38)
                .iter()
                .collect::<Vec<_>>(),
            [AdvertisingChannel::Channel38]
        );
    }

    #[test]
    fn channel_indices_are_bounded() {
        assert_eq!(DataChannel::new(36).map(DataChannel::index), Ok(36));
        assert_eq!(DataChannel::new(37), Err(ChannelError { index: 37 }));
        assert_eq!(TestChannel::new(39).map(TestChannel::rf_channel), Ok(39));
        assert_eq!(TestChannel::new(40), Err(ChannelError { index: 40 }));
    }
}
