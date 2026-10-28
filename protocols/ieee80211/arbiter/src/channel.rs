//! The channels an access point may operate on.
//!
//! An access point on a 5 GHz channel shared with radar (channels 52
//! through 144) must detect radar before and while it serves a BSS there
//! (dynamic frequency selection, ETSI EN 301 893 and FCC Part 15.407). This
//! stack has no radar detection, so an access point serves no such
//! channel.

use oer_ieee80211_wire::channel::Channel;
use oer_radio_regulatory::{FrequencySpan, radar_detection_required};

/// Half the width of a 20 MHz primary channel, in kHz.
const PRIMARY_HALF_WIDTH_KHZ: u32 = 10_000;

/// Whether an access point on `channel` must detect radar: its 20 MHz
/// primary channel overlaps a span radar shares (the 5 GHz channels 52
/// through 144).
pub const fn requires_radar_detection(channel: Channel) -> bool {
    let primary_khz = (channel.primary_frequency_mhz() as u32).saturating_mul(1_000);
    match FrequencySpan::new(
        primary_khz.saturating_sub(PRIMARY_HALF_WIDTH_KHZ),
        primary_khz.saturating_add(PRIMARY_HALF_WIDTH_KHZ),
    ) {
        Some(primary) => radar_detection_required(primary),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use oer_ieee80211_wire::channel::{Band, ChannelWidth};

    use super::*;

    #[test]
    fn the_5_ghz_channels_shared_with_radar_need_its_detection() {
        let channel = |band, number| Channel::new(band, number, ChannelWidth::Mhz20).unwrap();
        for (band, number, radar) in [
            (Band::Ghz2_4, 6, false),
            (Band::Ghz5, 36, false),
            (Band::Ghz5, 48, false),
            (Band::Ghz5, 52, true),
            (Band::Ghz5, 100, true),
            (Band::Ghz5, 144, true),
            (Band::Ghz5, 149, false),
            (Band::Ghz5, 165, false),
        ] {
            assert_eq!(requires_radar_detection(channel(band, number)), radar);
        }
        // The primary channel decides, whatever the width: a 40 MHz pair
        // whose primary is channel 48 stays outside, one whose primary is
        // channel 52 inside.
        let wide = |number, width| Channel::new(Band::Ghz5, number, width).unwrap();
        assert!(!requires_radar_detection(wide(
            48,
            ChannelWidth::Mhz40Below
        )));
        assert!(requires_radar_detection(wide(52, ChannelWidth::Mhz40Above)));
    }
}
