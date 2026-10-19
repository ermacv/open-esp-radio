//! The channels an access point may operate on.
//!
//! An access point on a 5 GHz channel shared with radar (channels 52
//! through 144) must detect radar before and while it serves a BSS there
//! (dynamic frequency selection, ETSI EN 301 893 and FCC Part 15.407). This
//! stack has no radar detection, so an access point serves no such
//! channel.

use oer_ieee80211_mac::channel::{Band, Channel};

/// Whether an access point on `channel` must detect radar: the 5 GHz
/// channels 52 through 144.
pub const fn requires_radar_detection(channel: Channel) -> bool {
    matches!(channel.band(), Band::Ghz5) && matches!(channel.number(), 52..=144)
}

#[cfg(test)]
mod tests {
    use oer_ieee80211_mac::channel::ChannelWidth;

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
        ] {
            assert_eq!(requires_radar_detection(channel(band, number)), radar);
        }
    }
}
