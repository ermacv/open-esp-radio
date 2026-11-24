//! The channels a standalone monitor visits: one until its stop, or a
//! bounded hopping cycle.
//!
//! This file is outside the build: no `mod` names it
//! (`docs/repository-layout.md`). No role of the build visits channels as a
//! monitor; it returns with one.

use core::{fmt, num::NonZeroU16};

use oer_protocols_ieee80211_wire::channel::Ghz2_4Channel;

/// Maximum number of ordered 2.4-GHz channels in one monitor hopping cycle.
///
/// This is a bounded product policy, not a claim that fourteen entries exhaust
/// every valid 20/40-MHz channel geometry. Each retained entry carries the
/// complete validated [`Ghz2_4Channel`], including its width relationship.
pub const MONITOR_CHANNEL_SEQUENCE_CAPACITY: usize = 14;

/// Allocation-free ordered channel cycle with one nonzero dwell per channel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MonitorChannelSequence {
    channels: [Ghz2_4Channel; MONITOR_CHANNEL_SEQUENCE_CAPACITY],
    length: oer_portable_wire::PrefixLength<MONITOR_CHANNEL_SEQUENCE_CAPACITY>,
    dwell_millis: NonZeroU16,
}

impl MonitorChannelSequence {
    pub fn new(
        channels: &[Ghz2_4Channel],
        dwell_millis: NonZeroU16,
    ) -> Result<Self, MonitorChannelSequenceError> {
        let Some(&first) = channels.first() else {
            return Err(MonitorChannelSequenceError::Empty);
        };
        let Some(length) = oer_portable_wire::PrefixLength::new(channels.len()) else {
            return Err(MonitorChannelSequenceError::TooMany {
                requested: channels.len(),
                capacity: MONITOR_CHANNEL_SEQUENCE_CAPACITY,
            });
        };
        for (index, &channel) in channels.iter().enumerate() {
            if channels
                .iter()
                .take(index)
                .any(|earlier| *earlier == channel)
            {
                return Err(MonitorChannelSequenceError::Duplicate(channel));
            }
        }
        let mut stored = [first; MONITOR_CHANNEL_SEQUENCE_CAPACITY];
        for (slot, &channel) in stored.iter_mut().zip(channels) {
            *slot = channel;
        }
        Ok(Self {
            channels: stored,
            length,
            dwell_millis,
        })
    }

    pub fn channels(&self) -> &[Ghz2_4Channel] {
        self.length.borrow(&self.channels)
    }

    /// How many channels the cycle visits; at least one.
    pub const fn count(self) -> usize {
        self.length.get()
    }

    pub const fn first(self) -> Ghz2_4Channel {
        let [first, ..] = self.channels;
        first
    }

    pub const fn dwell(self) -> NonZeroU16 {
        self.dwell_millis
    }

    pub const fn dwell_millis(self) -> u16 {
        self.dwell_millis.get()
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorChannelSequenceError {
    Empty,
    TooMany { requested: usize, capacity: usize },
    Duplicate(Ghz2_4Channel),
}

impl fmt::Display for MonitorChannelSequenceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("a monitor hopping cycle cannot be empty"),
            Self::TooMany {
                requested,
                capacity,
            } => write!(
                formatter,
                "monitor hopping requested {requested} channels but capacity is {capacity}"
            ),
            Self::Duplicate(channel) => {
                write!(formatter, "monitor hopping repeats channel {channel:?}")
            }
        }
    }
}

impl core::error::Error for MonitorChannelSequenceError {}

/// Physical-channel policy for one exclusive standalone monitor role.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MonitorChannelPolicy {
    /// Preserve the historical behavior: capture on one channel until stop.
    Fixed(Ghz2_4Channel),
    /// Repeatedly traverse one bounded ordered cycle.
    Hopping(MonitorChannelSequence),
}

impl MonitorChannelPolicy {
    pub const fn fixed(channel: Ghz2_4Channel) -> Self {
        Self::Fixed(channel)
    }

    pub const fn hopping(sequence: MonitorChannelSequence) -> Self {
        Self::Hopping(sequence)
    }

    pub const fn initial_channel(self) -> Ghz2_4Channel {
        match self {
            Self::Fixed(channel) => channel,
            Self::Hopping(sequence) => sequence.first(),
        }
    }

    pub const fn hopping_sequence(self) -> Option<MonitorChannelSequence> {
        match self {
            Self::Fixed(_) => None,
            Self::Hopping(sequence) => Some(sequence),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A hopping cycle holds its channels in order, at least one and at most
    /// its capacity, none twice.
    #[test]
    fn a_channel_sequence_holds_its_channels_in_order() {
        let channel = |number| Ghz2_4Channel::mhz20(number).unwrap();
        let dwell = NonZeroU16::new(50).unwrap();
        let sequence =
            MonitorChannelSequence::new(&[channel(6), channel(1), channel(11)], dwell).unwrap();
        assert_eq!(sequence.channels(), &[channel(6), channel(1), channel(11)]);
        assert_eq!(sequence.count(), 3);
        assert_eq!(sequence.first(), channel(6));
        assert_eq!(sequence.dwell(), dwell);
        assert_eq!(
            MonitorChannelSequence::new(&[], dwell),
            Err(MonitorChannelSequenceError::Empty)
        );
        assert_eq!(
            MonitorChannelSequence::new(&[channel(1), channel(1)], dwell),
            Err(MonitorChannelSequenceError::Duplicate(channel(1)))
        );
        let full: [Ghz2_4Channel; MONITOR_CHANNEL_SEQUENCE_CAPACITY] =
            core::array::from_fn(|index| channel(u8::try_from(index).unwrap() + 1));
        assert_eq!(
            MonitorChannelSequence::new(&full, dwell).unwrap().count(),
            MONITOR_CHANNEL_SEQUENCE_CAPACITY
        );
        let mut over = [channel(1); MONITOR_CHANNEL_SEQUENCE_CAPACITY + 1];
        over[..MONITOR_CHANNEL_SEQUENCE_CAPACITY].copy_from_slice(&full);
        assert_eq!(
            MonitorChannelSequence::new(&over, dwell),
            Err(MonitorChannelSequenceError::TooMany {
                requested: MONITOR_CHANNEL_SEQUENCE_CAPACITY + 1,
                capacity: MONITOR_CHANNEL_SEQUENCE_CAPACITY,
            })
        );
    }
}
