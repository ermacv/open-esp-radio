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

use crate::channel::{Band, Channel, ChannelError, ChannelWidth};

/// The Channel Switch Announcement element.
pub const CHANNEL_SWITCH_ELEMENT_ID: u8 = 37;
/// The Secondary Channel Offset element.
pub const SECONDARY_CHANNEL_OFFSET_ELEMENT_ID: u8 = 62;
/// The Extended Channel Switch Announcement element.
pub const EXTENDED_CHANNEL_SWITCH_ELEMENT_ID: u8 = 60;
/// The Spectrum Management action category.
const SPECTRUM_MANAGEMENT_CATEGORY: u8 = 0;
/// Its Channel Switch Announcement action.
const CHANNEL_SWITCH_ACTION: u8 = 4;
/// The Public action category.
const PUBLIC_CATEGORY: u8 = 4;
/// Its Extended Channel Switch Announcement action.
const EXTENDED_CHANNEL_SWITCH_ACTION: u8 = 4;

/// Whether the BSS's stations may transmit until the switch.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum ChannelSwitchMode {
    /// Stations go on transmitting until the switch.
    Continue,
    /// Stations transmit nothing more on the current channel.
    StopTransmitting,
}

/// Where a 40 MHz channel's secondary 20 MHz lies.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SecondaryChannelOffset {
    /// No secondary channel: 20 MHz.
    None,
    Above,
    Below,
}

/// One channel switch announcement.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ChannelSwitch {
    pub mode: ChannelSwitchMode,
    /// The new primary channel's number.
    pub channel_number: u8,
    /// Target beacon transmission times until the switch; 0 is any time
    /// from now.
    pub count: u8,
    /// The new operating class, which an extended announcement names.
    pub operating_class: Option<u8>,
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
    UnsupportedOperatingClass(u8),
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
                EXTENDED_CHANNEL_SWITCH_ELEMENT_ID,
                4,
                mode_value(self.mode),
                class,
                self.channel_number,
                self.count,
            ];
            out.get_mut(..element.len())?.copy_from_slice(&element);
            return Some(element.len());
        }
        let mut elements = [0_u8; 8];
        elements[..5].copy_from_slice(&[
            CHANNEL_SWITCH_ELEMENT_ID,
            3,
            mode_value(self.mode),
            self.channel_number,
            self.count,
        ]);
        let length = match self.secondary {
            SecondaryChannelOffset::None => 5,
            offset => {
                elements[5..8].copy_from_slice(&[
                    SECONDARY_CHANNEL_OFFSET_ELEMENT_ID,
                    1,
                    offset_value(offset),
                ]);
                8
            }
        };
        out.get_mut(..length)?.copy_from_slice(&elements[..length]);
        Some(length)
    }

    /// The Channel Switch Announcement action frame body: category, action
    /// and the elements; its length, or `None` when `out` is too short.
    pub fn encode_action(self, out: &mut [u8]) -> Option<usize> {
        let (head, rest) = out.split_at_mut_checked(2)?;
        head.copy_from_slice(&[SPECTRUM_MANAGEMENT_CATEGORY, CHANNEL_SWITCH_ACTION]);
        Some(2 + self.encode_elements(rest)?)
    }
}

/// The announcement in the information elements `elements` of a beacon,
/// Probe Response or Channel Switch Announcement action: `None` without one.
/// An extended announcement takes precedence over a plain one.
pub fn parse_channel_switch(elements: &[u8]) -> Result<Option<ChannelSwitch>, ChannelSwitchError> {
    let mut plain = None;
    let mut extended = None;
    let mut secondary = SecondaryChannelOffset::None;
    let mut offset = 0;
    while offset < elements.len() {
        let header = elements
            .get(offset..offset + 2)
            .ok_or(ChannelSwitchError::Malformed)?;
        let body = elements
            .get(offset + 2..offset + 2 + usize::from(header[1]))
            .ok_or(ChannelSwitchError::Malformed)?;
        match (header[0], body) {
            (CHANNEL_SWITCH_ELEMENT_ID, &[mode, channel, count]) => {
                plain = Some((mode_of(mode)?, channel, count));
            }
            (EXTENDED_CHANNEL_SWITCH_ELEMENT_ID, &[mode, class, channel, count]) => {
                extended = Some((mode_of(mode)?, class, channel, count));
            }
            (SECONDARY_CHANNEL_OFFSET_ELEMENT_ID, &[value]) => secondary = offset_of(value)?,
            (CHANNEL_SWITCH_ELEMENT_ID | EXTENDED_CHANNEL_SWITCH_ELEMENT_ID, _)
            | (SECONDARY_CHANNEL_OFFSET_ELEMENT_ID, _) => {
                return Err(ChannelSwitchError::Malformed);
            }
            _ => {}
        }
        offset += 2 + body.len();
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
    match body {
        [
            SPECTRUM_MANAGEMENT_CATEGORY,
            CHANNEL_SWITCH_ACTION,
            elements @ ..,
        ] => parse_channel_switch(elements),
        [
            PUBLIC_CATEGORY,
            EXTENDED_CHANNEL_SWITCH_ACTION,
            mode,
            class,
            channel,
            count,
            elements @ ..,
        ] => {
            let secondary = parse_channel_switch(elements)?
                .map_or(SecondaryChannelOffset::None, |announcement| {
                    announcement.secondary
                });
            Ok(Some(ChannelSwitch {
                mode: mode_of(*mode)?,
                channel_number: *channel,
                count: *count,
                operating_class: Some(*class),
                secondary,
            }))
        }
        _ => Ok(None),
    }
}

const fn mode_value(mode: ChannelSwitchMode) -> u8 {
    match mode {
        ChannelSwitchMode::Continue => 0,
        ChannelSwitchMode::StopTransmitting => 1,
    }
}

fn mode_of(value: u8) -> Result<ChannelSwitchMode, ChannelSwitchError> {
    match value {
        0 => Ok(ChannelSwitchMode::Continue),
        1 => Ok(ChannelSwitchMode::StopTransmitting),
        _ => Err(ChannelSwitchError::Malformed),
    }
}

const fn offset_value(offset: SecondaryChannelOffset) -> u8 {
    match offset {
        SecondaryChannelOffset::None => 0,
        SecondaryChannelOffset::Above => 1,
        SecondaryChannelOffset::Below => 3,
    }
}

fn offset_of(value: u8) -> Result<SecondaryChannelOffset, ChannelSwitchError> {
    match value {
        0 => Ok(SecondaryChannelOffset::None),
        1 => Ok(SecondaryChannelOffset::Above),
        3 => Ok(SecondaryChannelOffset::Below),
        _ => Err(ChannelSwitchError::Malformed),
    }
}

/// The global operating class (IEEE Std 802.11-2020 Table E-4) of a 20 or
/// 40 MHz `channel`.
fn operating_class_of(channel: Channel) -> u8 {
    let number = channel.number();
    match (channel.band(), channel.width()) {
        (Band::Ghz2_4, ChannelWidth::Mhz20) => 81,
        (Band::Ghz2_4, ChannelWidth::Mhz40Above) => 83,
        (Band::Ghz2_4, ChannelWidth::Mhz40Below) => 84,
        (Band::Ghz5, width) => {
            let [twenty, above, below] = match number {
                36..=48 => [115, 116, 117],
                52..=64 => [118, 119, 120],
                100..=144 => [121, 122, 123],
                _ => [125, 126, 127],
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
fn operating_class(class: u8) -> Result<(Band, ChannelWidth), ChannelSwitchError> {
    Ok(match class {
        81 | 82 => (Band::Ghz2_4, ChannelWidth::Mhz20),
        83 => (Band::Ghz2_4, ChannelWidth::Mhz40Above),
        84 => (Band::Ghz2_4, ChannelWidth::Mhz40Below),
        115 | 118 | 121 | 124 | 125 => (Band::Ghz5, ChannelWidth::Mhz20),
        116 | 119 | 122 | 126 => (Band::Ghz5, ChannelWidth::Mhz40Above),
        117 | 120 | 123 | 127 => (Band::Ghz5, ChannelWidth::Mhz40Below),
        class => return Err(ChannelSwitchError::UnsupportedOperatingClass(class)),
    })
}

#[cfg(test)]
mod tests;
