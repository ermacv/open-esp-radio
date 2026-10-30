//! The regulatory view of an IEEE 802.11 channel: the frequencies it
//! occupies.
//!
//! The codec names a channel by its band, number and width; which
//! frequencies that occupies, and what a regulatory domain allows there, is
//! the port's operational description.

use oer_ieee80211_wire::channel::{Channel, WifiChannel};
use oer_radio_regulatory::{FrequencySpan, RadioChannel};

/// A channel, [`Channel`] or [`WifiChannel`], as a regulatory domain sees it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct RegulatedChannel<C = Channel>(C);

impl<C> RegulatedChannel<C> {
    /// The regulatory view of `channel`.
    pub const fn new(channel: C) -> Self {
        Self(channel)
    }
}

impl<C: Copy> RegulatedChannel<C> {
    /// The channel this regulatory view describes.
    pub const fn channel(self) -> C {
        self.0
    }
}

impl<C> From<C> for RegulatedChannel<C> {
    fn from(channel: C) -> Self {
        Self(channel)
    }
}

impl RadioChannel for RegulatedChannel<Channel> {
    fn span(&self) -> FrequencySpan {
        *self.0.select(&SPANS)
    }
}

impl RadioChannel for RegulatedChannel<WifiChannel> {
    fn span(&self) -> FrequencySpan {
        *Channel::from_wifi_channel(self.0).select(&SPANS)
    }
}

/// The span of every channel: its bandwidth around its center frequency.
///
/// SOURCE: IEEE Std 802.11-2020, Annex E (channel starting frequencies and
/// center frequency indexes). Built and checked at compilation.
static SPANS: [FrequencySpan; Channel::COUNT] = const {
    let mut spans = [match FrequencySpan::new(0, 1) {
        Some(span) => span,
        None => panic!("a placeholder span is not empty"),
    }; Channel::COUNT];
    let all = Channel::ALL;
    let mut channels = all.as_slice();
    let mut cells = spans.as_mut_slice();
    while let (Some((channel, rest)), Some((cell, cells_rest))) =
        (channels.split_first(), cells.split_first_mut())
    {
        let half_khz = channel.bandwidth_mhz() as u32 * 500;
        let center_khz = channel.center_frequency_mhz() as u32 * 1_000;
        *cell = match FrequencySpan::new(center_khz - half_khz, center_khz + half_khz) {
            Some(span) => span,
            None => panic!("a channel occupies no frequencies"),
        };
        channels = rest;
        cells = cells_rest;
    }
    spans
};

#[cfg(test)]
mod tests {
    use super::*;
    use oer_ieee80211_wire::channel::ChannelWidth;

    #[test]
    fn channels_span_their_width_around_the_center() {
        let span = |channel| RegulatedChannel::new(channel).span();
        let six = span(Channel::ghz2_4(6, ChannelWidth::Mhz20).unwrap());
        assert_eq!((six.low_khz(), six.high_khz()), (2_427_000, 2_447_000));
        let ht40 = span(Channel::ghz2_4(1, ChannelWidth::Mhz40Above).unwrap());
        assert_eq!((ht40.low_khz(), ht40.high_khz()), (2_402_000, 2_442_000));
        let fourteen = span(Channel::ghz2_4(14, ChannelWidth::Mhz20).unwrap());
        assert_eq!(fourteen.high_khz(), 2_494_000);
        let forty = span(Channel::ghz5(36, ChannelWidth::Mhz40Above).unwrap());
        assert_eq!((forty.low_khz(), forty.high_khz()), (5_170_000, 5_210_000));
    }

    #[test]
    fn every_channel_spans_its_width_and_a_wifi_channel_the_same() {
        for channel in Channel::ALL {
            let span = RegulatedChannel::new(channel).span();
            let center = u32::from(channel.center_frequency_mhz()) * 1000;
            let half = u32::from(channel.bandwidth_mhz()) * 500;
            assert_eq!(
                (span.low_khz(), span.high_khz()),
                (center - half, center + half)
            );
            if let Some(wifi) = channel.wifi_channel() {
                assert_eq!(RegulatedChannel::from(wifi).span(), span);
                assert_eq!(RegulatedChannel::from(wifi).channel(), wifi);
            }
        }
    }
}
