//! Channel switch announcements (IEEE Std 802.11-2020 9.4.2.18, 9.4.2.52,
//! 9.4.2.19, 9.6.2.6 and 9.6.7.7).
//!
//! An access point that moves its BSS announces the move in its beacons and
//! in an action frame: the Channel Switch Announcement element (37), or the
//! Extended Channel Switch Announcement (60) that also names the new
//! operating class, with a Secondary Channel Offset element (62) for a
//! 40 MHz channel. [`ChannelSwitch`] is one such announcement and its
//! target channel; this module parses and encodes them and owns no timing:
//! the switch happens `count` target beacon transmission times later, which
//! the caller schedules.
//!
//! Received bytes reach this module; nothing here panics on them.

use crate::action::{ActionCategory, PublicAction, SpectrumManagementAction};
use crate::channel::{Band, Channel, ChannelError, ChannelWidth};
use crate::element::ElementId;

crate::closed_code::closed_code! {
    /// Whether the BSS's stations may transmit until the switch: the
    /// Channel Switch Mode field (IEEE Std 802.11-2020, 9.4.2.19).
    #[derive(Hash)]
    pub enum ChannelSwitchMode: 8, ChannelSwitchError::Malformed => ChannelSwitchError {
        /// Stations go on transmitting until the switch.
        Continue = 0,
        /// Stations transmit nothing more on the current channel.
        StopTransmitting = 1,
    }
}

crate::closed_code::closed_code! {
    /// Where a 40 MHz channel's secondary 20 MHz lies: the Secondary
    /// Channel Offset field (IEEE Std 802.11-2020, 9.4.2.20).
    #[derive(Hash)]
    pub enum SecondaryChannelOffset: 8, ChannelSwitchError::Malformed => ChannelSwitchError {
        /// No secondary channel: 20 MHz.
        None = 0,
        Above = 1,
        Below = 3,
    }
}

oer_portable_wire::octet_code! { "oer_protocols_ieee80211_wire::channel_switch";
    /// A global operating class: any octet, the 20 and 40 MHz classes of
    /// 2.4 and 5 GHz named.
    ///
    /// SOURCE: IEEE Std 802.11-2020, Annex E, Table E-4 (Global operating
    /// classes).
    OperatingClass {
        /// 2.4 GHz, 20 MHz, channels 1 to 13.
        GHZ2_4_MHZ20 = 81,
        /// 2.4 GHz, 20 MHz, channel 14.
        GHZ2_4_MHZ20_CHANNEL_14 = 82,
        /// 2.4 GHz, 40 MHz, the secondary above.
        GHZ2_4_MHZ40_ABOVE = 83,
        /// 2.4 GHz, 40 MHz, the secondary below.
        GHZ2_4_MHZ40_BELOW = 84,
        /// 5 GHz, 20 MHz, channels 36 to 48.
        GHZ5_36_MHZ20 = 115,
        /// 5 GHz, 40 MHz above, channels 36 and 44.
        GHZ5_36_MHZ40_ABOVE = 116,
        /// 5 GHz, 40 MHz below, channels 40 and 48.
        GHZ5_36_MHZ40_BELOW = 117,
        /// 5 GHz, 20 MHz, channels 52 to 64.
        GHZ5_52_MHZ20 = 118,
        /// 5 GHz, 40 MHz above, channels 52 and 60.
        GHZ5_52_MHZ40_ABOVE = 119,
        /// 5 GHz, 40 MHz below, channels 56 and 64.
        GHZ5_52_MHZ40_BELOW = 120,
        /// 5 GHz, 20 MHz, channels 100 to 144.
        GHZ5_100_MHZ20 = 121,
        /// 5 GHz, 40 MHz above, channels 100 to 140.
        GHZ5_100_MHZ40_ABOVE = 122,
        /// 5 GHz, 40 MHz below, channels 104 to 144.
        GHZ5_100_MHZ40_BELOW = 123,
        /// 5 GHz, 20 MHz, channels 149 to 161.
        GHZ5_149_MHZ20 = 124,
        /// 5 GHz, 20 MHz, channels 149 to 177.
        GHZ5_149_177_MHZ20 = 125,
        /// 5 GHz, 40 MHz above, channels 149 to 173.
        GHZ5_149_MHZ40_ABOVE = 126,
        /// 5 GHz, 40 MHz below, channels 153 to 177.
        GHZ5_149_MHZ40_BELOW = 127,
    }
}

/// One channel switch announcement.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChannelSwitch {
    pub mode: ChannelSwitchMode,
    /// The new primary channel's number.
    pub channel_number: u8,
    /// Target beacon transmission times until the switch; 0 is any time
    /// from now.
    pub count: u8,
    /// The new operating class, which an extended announcement names.
    pub operating_class: Option<OperatingClass>,
    /// The new channel's secondary offset, which a Secondary Channel Offset
    /// element beside a Channel Switch Announcement names.
    pub secondary: SecondaryChannelOffset,
}

/// Why an announcement names no channel this crate represents.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ChannelSwitchError {
    /// An element's length or a reserved value is wrong.
    Malformed,
    /// An operating class of a width other than 20 or 40 MHz, or unknown.
    UnsupportedOperatingClass(OperatingClass),
    /// The channel number and width are no valid channel.
    Channel(ChannelError),
}

impl ChannelSwitch {
    /// The channel the BSS moves to; `current` is the band an announcement
    /// without an operating class stays in. Without a secondary offset the
    /// new channel is 20 MHz wide (9.4.2.19).
    pub fn target(self, current: Band) -> Result<Channel, ChannelSwitchError> {
        let (band, width) = match self.operating_class {
            Some(class) => operating_class(class)?,
            None => (
                current,
                match self.secondary {
                    SecondaryChannelOffset::None => ChannelWidth::Mhz20,
                    SecondaryChannelOffset::Above => ChannelWidth::Mhz40Above,
                    SecondaryChannelOffset::Below => ChannelWidth::Mhz40Below,
                },
            ),
        };
        Channel::new(band, self.channel_number, width).map_err(ChannelSwitchError::Channel)
    }

    /// The announcement of a move to `channel` in `count` target beacon
    /// transmission times.
    pub fn to(channel: Channel, mode: ChannelSwitchMode, count: u8) -> Self {
        Self {
            mode,
            channel_number: channel.number(),
            count,
            operating_class: None,
            secondary: match channel.width() {
                ChannelWidth::Mhz20 => SecondaryChannelOffset::None,
                ChannelWidth::Mhz40Above => SecondaryChannelOffset::Above,
                ChannelWidth::Mhz40Below => SecondaryChannelOffset::Below,
            },
        }
    }

    /// The extended announcement of a move to `channel`, which names its
    /// operating class: the form a move to another band needs, as a plain
    /// announcement names a channel of the current band.
    pub fn extended_to(channel: Channel, mode: ChannelSwitchMode, count: u8) -> Self {
        Self {
            operating_class: Some(operating_class_of(channel)),
            ..Self::to(channel, mode, count)
        }
    }

    /// The announcement's elements, written into `out`: their length, or
    /// `None` when `out` is too short. An extended announcement is the
    /// Extended Channel Switch Announcement, whose operating class names
    /// the width; a plain one is the Channel Switch Announcement and, for a
    /// 40 MHz channel, the Secondary Channel Offset after it.
    pub fn encode_elements(self, out: &mut [u8]) -> Option<usize> {
        if let Some(class) = self.operating_class {
            let element = [
                ElementId::EXTENDED_CHANNEL_SWITCH_ANNOUNCEMENT.value(),
                4,
                self.mode.wire().get(),
                class.value(),
                self.channel_number,
                self.count,
            ];
            oer_portable_wire::Writer::new(out).write(&element).ok()?;
            return Some(element.len());
        }
        let announcement = [
            ElementId::CHANNEL_SWITCH_ANNOUNCEMENT.value(),
            3,
            self.mode.wire().get(),
            self.channel_number,
            self.count,
        ];
        let [a0, a1, a2, a3, a4] = announcement;
        let with_offset;
        let elements: &[u8] = match self.secondary {
            SecondaryChannelOffset::None => &announcement,
            offset => {
                with_offset = [
                    a0,
                    a1,
                    a2,
                    a3,
                    a4,
                    ElementId::SECONDARY_CHANNEL_OFFSET.value(),
                    1,
                    offset.wire().get(),
                ];
                &with_offset
            }
        };
        let (written, _) = out.split_at_mut_checked(elements.len())?;
        oer_portable_wire::Writer::new(written)
            .write(elements)
            .ok()?;
        Some(elements.len())
    }

    /// The Channel Switch Announcement action frame body: category, action
    /// and the elements; its length, or `None` when `out` is too short.
    pub fn encode_action(self, out: &mut [u8]) -> Option<usize> {
        let (head, rest) = out.split_first_chunk_mut::<2>()?;
        *head = [
            ActionCategory::SPECTRUM_MANAGEMENT.value(),
            SpectrumManagementAction::CHANNEL_SWITCH_ANNOUNCEMENT.value(),
        ];
        head.len().checked_add(self.encode_elements(rest)?)
    }
}

/// The announcement in the information elements `elements` of a beacon,
/// Probe Response or Channel Switch Announcement action: `None` without one.
/// An extended announcement takes precedence over a plain one.
pub fn parse_channel_switch(elements: &[u8]) -> Result<Option<ChannelSwitch>, ChannelSwitchError> {
    let mut plain = None;
    let mut extended = None;
    let mut secondary = SecondaryChannelOffset::None;
    let mut remaining = elements;
    while !remaining.is_empty() {
        let mut reader = oer_portable_wire::Reader::new(remaining);
        let &[id, length] = reader
            .array::<2>()
            .map_err(|_| ChannelSwitchError::Malformed)?;
        let body = reader
            .take(usize::from(length))
            .map_err(|_| ChannelSwitchError::Malformed)?;
        remaining = reader.remaining();
        match (ElementId::new(id), body) {
            (ElementId::CHANNEL_SWITCH_ANNOUNCEMENT, &[mode, channel, count]) => {
                plain = Some((ChannelSwitchMode::from_wire(mode)?, channel, count));
            }
            (ElementId::EXTENDED_CHANNEL_SWITCH_ANNOUNCEMENT, &[mode, class, channel, count]) => {
                extended = Some((
                    ChannelSwitchMode::from_wire(mode)?,
                    OperatingClass::new(class),
                    channel,
                    count,
                ));
            }
            (ElementId::SECONDARY_CHANNEL_OFFSET, &[value]) => {
                secondary = SecondaryChannelOffset::from_wire(value)?
            }
            (
                ElementId::CHANNEL_SWITCH_ANNOUNCEMENT
                | ElementId::EXTENDED_CHANNEL_SWITCH_ANNOUNCEMENT,
                _,
            )
            | (ElementId::SECONDARY_CHANNEL_OFFSET, _) => {
                return Err(ChannelSwitchError::Malformed);
            }
            _ => {}
        }
    }
    Ok(match (extended, plain) {
        (Some((mode, class, channel_number, count)), _) => Some(ChannelSwitch {
            mode,
            channel_number,
            count,
            operating_class: Some(class),
            secondary,
        }),
        (None, Some((mode, channel_number, count))) => Some(ChannelSwitch {
            mode,
            channel_number,
            count,
            operating_class: None,
            secondary,
        }),
        (None, None) => None,
    })
}

/// The announcement an action frame body carries: a Spectrum Management
/// Channel Switch Announcement, or a Public Extended Channel Switch
/// Announcement; `None` for any other action.
pub fn parse_channel_switch_action(
    body: &[u8],
) -> Result<Option<ChannelSwitch>, ChannelSwitchError> {
    let [category, action, fields @ ..] = body else {
        return Ok(None);
    };
    match (ActionCategory::new(*category), fields) {
        (ActionCategory::SPECTRUM_MANAGEMENT, elements)
            if SpectrumManagementAction::new(*action)
                == SpectrumManagementAction::CHANNEL_SWITCH_ANNOUNCEMENT =>
        {
            parse_channel_switch(elements)
        }
        (ActionCategory::PUBLIC, [mode, class, channel, count, elements @ ..])
            if PublicAction::new(*action) == PublicAction::EXTENDED_CHANNEL_SWITCH_ANNOUNCEMENT =>
        {
            let secondary = parse_channel_switch(elements)?
                .map_or(SecondaryChannelOffset::None, |announcement| {
                    announcement.secondary
                });
            Ok(Some(ChannelSwitch {
                mode: ChannelSwitchMode::from_wire(*mode)?,
                channel_number: *channel,
                count: *count,
                operating_class: Some(OperatingClass::new(*class)),
                secondary,
            }))
        }
        _ => Ok(None),
    }
}

/// The global operating class (IEEE Std 802.11-2020 Table E-4) of a 20 or
/// 40 MHz `channel`.
fn operating_class_of(channel: Channel) -> OperatingClass {
    let number = channel.number();
    match (channel.band(), channel.width()) {
        (Band::Ghz2_4, ChannelWidth::Mhz20) => OperatingClass::GHZ2_4_MHZ20,
        (Band::Ghz2_4, ChannelWidth::Mhz40Above) => OperatingClass::GHZ2_4_MHZ40_ABOVE,
        (Band::Ghz2_4, ChannelWidth::Mhz40Below) => OperatingClass::GHZ2_4_MHZ40_BELOW,
        (Band::Ghz5, width) => {
            let [twenty, above, below] = match number {
                36..=48 => [
                    OperatingClass::GHZ5_36_MHZ20,
                    OperatingClass::GHZ5_36_MHZ40_ABOVE,
                    OperatingClass::GHZ5_36_MHZ40_BELOW,
                ],
                52..=64 => [
                    OperatingClass::GHZ5_52_MHZ20,
                    OperatingClass::GHZ5_52_MHZ40_ABOVE,
                    OperatingClass::GHZ5_52_MHZ40_BELOW,
                ],
                100..=144 => [
                    OperatingClass::GHZ5_100_MHZ20,
                    OperatingClass::GHZ5_100_MHZ40_ABOVE,
                    OperatingClass::GHZ5_100_MHZ40_BELOW,
                ],
                _ => [
                    OperatingClass::GHZ5_149_177_MHZ20,
                    OperatingClass::GHZ5_149_MHZ40_ABOVE,
                    OperatingClass::GHZ5_149_MHZ40_BELOW,
                ],
            };
            match width {
                ChannelWidth::Mhz20 => twenty,
                ChannelWidth::Mhz40Above => above,
                ChannelWidth::Mhz40Below => below,
            }
        }
    }
}

/// The band and width of a global operating class (IEEE Std 802.11-2020
/// Table E-4) of 20 or 40 MHz.
fn operating_class(class: OperatingClass) -> Result<(Band, ChannelWidth), ChannelSwitchError> {
    Ok(match class {
        OperatingClass::GHZ2_4_MHZ20 | OperatingClass::GHZ2_4_MHZ20_CHANNEL_14 => {
            (Band::Ghz2_4, ChannelWidth::Mhz20)
        }
        OperatingClass::GHZ2_4_MHZ40_ABOVE => (Band::Ghz2_4, ChannelWidth::Mhz40Above),
        OperatingClass::GHZ2_4_MHZ40_BELOW => (Band::Ghz2_4, ChannelWidth::Mhz40Below),
        OperatingClass::GHZ5_36_MHZ20
        | OperatingClass::GHZ5_52_MHZ20
        | OperatingClass::GHZ5_100_MHZ20
        | OperatingClass::GHZ5_149_MHZ20
        | OperatingClass::GHZ5_149_177_MHZ20 => (Band::Ghz5, ChannelWidth::Mhz20),
        OperatingClass::GHZ5_36_MHZ40_ABOVE
        | OperatingClass::GHZ5_52_MHZ40_ABOVE
        | OperatingClass::GHZ5_100_MHZ40_ABOVE
        | OperatingClass::GHZ5_149_MHZ40_ABOVE => (Band::Ghz5, ChannelWidth::Mhz40Above),
        OperatingClass::GHZ5_36_MHZ40_BELOW
        | OperatingClass::GHZ5_52_MHZ40_BELOW
        | OperatingClass::GHZ5_100_MHZ40_BELOW
        | OperatingClass::GHZ5_149_MHZ40_BELOW => (Band::Ghz5, ChannelWidth::Mhz40Below),
        class => return Err(ChannelSwitchError::UnsupportedOperatingClass(class)),
    })
}

#[cfg(test)]
mod tests;
