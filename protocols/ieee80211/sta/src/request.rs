//! Chip- and executor-independent station discovery request values.
//!
//! These types belong to the IEEE 802.11 station protocol boundary. They
//! contain no radio, DMA, interrupt, executor or security-key ownership, so a
//! chip adapter can consume the exact application policy without depending on
//! a top-level radio API.

use core::{fmt, num::NonZeroU16};

use oer_ieee80211_wire::channel::WifiChannel;
pub use oer_ieee80211_wire::ssid::{WifiSsid, WifiSsidError};
use oer_ieee80211_wire::station::association::Preference;
use oer_radio_regulatory::{Initiate, RegulatoryDomain};

const CHANNEL_ONE_BIT: u16 = 1;

/// Number of beacon intervals an infrastructure AP may buffer traffic for
/// this station after association.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(transparent)]
pub struct StationListenInterval(NonZeroU16);

impl StationListenInterval {
    /// Conservative legacy value used by the existing qualified STA path.
    pub const DEFAULT: Self = const { Self(NonZeroU16::new(3).unwrap()) };

    pub const fn new(beacon_intervals: u16) -> Option<Self> {
        match NonZeroU16::new(beacon_intervals) {
            Some(interval) => Some(Self(interval)),
            None => None,
        }
    }

    pub const fn get(self) -> u16 {
        self.0.get()
    }
}

impl Default for StationListenInterval {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Station power management for one complete service/reconnect epoch, as
/// the vendor Wi-Fi library offers it.
///
/// Every mode follows the coexistence schedule while another radio shares
/// the air: the station then advertises power save outside its Wi-Fi slices
/// even with [`Self::None`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum StationPowerMode {
    /// No power save of its own (`WIFI_PS_NONE`).
    #[default]
    None,
    /// Modem sleep waking for every DTIM (`WIFI_PS_MIN_MODEM`).
    MinModem,
    /// Modem sleep waking at the listen interval (`WIFI_PS_MAX_MODEM`).
    MaxModem(StationListenInterval),
}

impl StationPowerMode {
    /// The listen interval the station advertises in its association.
    pub const fn listen_interval(self) -> StationListenInterval {
        match self {
            Self::None | Self::MinModem => StationListenInterval::DEFAULT,
            Self::MaxModem(listen_interval) => listen_interval,
        }
    }
}

/// Allocation-free set of 2.4-GHz primary channels selected for scanning.
/// Only bits 0..=13 can be set, and only for channels the regulatory domain
/// permits: constructors validate channels 1..=14 before deriving an offset.
/// A channel the domain allows to listen on only is scanned passively: the
/// station sends no probe request there. Iterators retain the bound,
/// including after removing a selected preference, so their exact lengths
/// never exceed 14.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationScanChannels {
    /// Channels the station may send probe requests on.
    active: u16,
    /// Channels it listens on only.
    passive: u16,
}

impl StationScanChannels {
    /// The requested primary channels, when `domain` permits each.
    pub fn permitted(
        domain: &RegulatoryDomain,
        channels: &[u8],
    ) -> Result<Self, StationScanChannelsError> {
        let mut set = Self {
            active: 0,
            passive: 0,
        };
        for &channel in channels {
            if !(1..=14).contains(&channel) {
                return Err(StationScanChannelsError::InvalidPrimary(channel));
            }
            let permitted = WifiChannel::mhz20(channel)
                .ok()
                .and_then(|wifi| domain.permit(wifi).ok());
            let Some(permitted) = permitted else {
                return Err(StationScanChannelsError::NotPermitted(channel));
            };
            set.insert(channel, permitted.initiate());
        }
        set.non_empty()
    }

    /// Every 2.4-GHz channel `domain` permits.
    pub fn all_permitted(domain: &RegulatoryDomain) -> Result<Self, StationScanChannelsError> {
        let mut set = Self {
            active: 0,
            passive: 0,
        };
        for channel in 1..=14_u8 {
            if let Some(permitted) = WifiChannel::mhz20(channel)
                .ok()
                .and_then(|wifi| domain.permit(wifi).ok())
            {
                set.insert(channel, permitted.initiate());
            }
        }
        set.non_empty()
    }

    fn insert(&mut self, channel: u8, initiate: Initiate) {
        // The checked channel is 1..=14, so the exact shift is 0..=13.
        let bit = CHANNEL_ONE_BIT << channel.wrapping_sub(1);
        match initiate {
            Initiate::Allowed => self.active |= bit,
            Initiate::PassiveOnly => self.passive |= bit,
        }
    }

    fn non_empty(self) -> Result<Self, StationScanChannelsError> {
        if self.bits() == 0 {
            Err(StationScanChannelsError::Empty)
        } else {
            Ok(self)
        }
    }

    const fn bits(self) -> u16 {
        self.active | self.passive
    }

    pub const fn contains(self, primary: u8) -> bool {
        // Short-circuit range checks prove 1..=14 before subtraction/shift.
        primary != 0
            && primary <= 14
            && self.bits() & (CHANNEL_ONE_BIT << primary.wrapping_sub(1)) != 0
    }

    /// Whether the station may send a probe request on `primary`: it is
    /// selected and not passive-only.
    pub const fn may_probe(self, primary: u8) -> bool {
        primary != 0
            && primary <= 14
            && self.active & (CHANNEL_ONE_BIT << primary.wrapping_sub(1)) != 0
    }

    pub const fn count(self) -> u8 {
        self.bits().count_ones() as u8
    }

    pub const fn primary_channels(self) -> StationScanChannelIter {
        StationScanChannelIter {
            remaining: self.bits(),
        }
    }

    /// Iterate the selected set with one optional channel first.
    ///
    /// A backend may use this as a scan-latency hint without changing the
    /// application-selected channel set. An absent or unselected preference
    /// falls back to ascending channel order.
    pub const fn primary_channels_preferred(
        self,
        preferred: Option<u8>,
    ) -> StationScanChannelOrderIter {
        let preferred = match preferred {
            Some(channel) if self.contains(channel) => Some(channel),
            _ => None,
        };
        StationScanChannelOrderIter {
            remaining: StationScanChannelIter {
                remaining: match preferred {
                    // `contains` proved the selected preference is 1..=14.
                    Some(channel) => self.bits() & !(CHANNEL_ONE_BIT << channel.wrapping_sub(1)),
                    None => self.bits(),
                },
            },
            preferred,
        }
    }
}

/// Allocation-free iterator over one checked station scan channel set.
pub struct StationScanChannelIter {
    remaining: u16,
}

impl Iterator for StationScanChannelIter {
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        let offset = self.remaining.trailing_zeros() as u8;
        // A nonempty 14-bit set has a lowest offset 0..=13. Subtracting
        // one from its nonzero bitmap clears that bit with `bits & (bits-1)`;
        // neither this subtraction nor offset+1 can wrap.
        self.remaining &= self.remaining.wrapping_sub(1);
        Some(offset.wrapping_add(1))
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.remaining.count_ones() as usize;
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for StationScanChannelIter {}

/// Allocation-free ordered traversal of one checked channel set.
pub struct StationScanChannelOrderIter {
    remaining: StationScanChannelIter,
    preferred: Option<u8>,
}

impl Iterator for StationScanChannelOrderIter {
    type Item = u8;

    fn next(&mut self) -> Option<Self::Item> {
        self.preferred.take().or_else(|| self.remaining.next())
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        // The preference was removed from the remaining set. Their combined
        // count is exactly the original subset's length, at most 14.
        let remaining = self
            .remaining
            .len()
            .wrapping_add(usize::from(self.preferred.is_some()));
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for StationScanChannelOrderIter {}

/// Invalid finite station scan channel set.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StationScanChannelsError {
    Empty,
    InvalidPrimary(u8),
    /// The regulatory domain does not permit the channel.
    NotPermitted(u8),
}

impl fmt::Display for StationScanChannelsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("a station scan requires at least one channel"),
            Self::InvalidPrimary(channel) => {
                write!(formatter, "invalid 2.4-GHz primary channel {channel}")
            }
            Self::NotPermitted(channel) => {
                write!(formatter, "the regulatory domain forbids channel {channel}")
            }
        }
    }
}

impl core::error::Error for StationScanChannelsError {}

/// Candidate-discovery and association policy for one station service.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationScanPolicy {
    channels: StationScanChannels,
    dwell_millis: NonZeroU16,
    association: Preference,
}

impl StationScanPolicy {
    pub const fn new(
        channels: StationScanChannels,
        dwell_millis: NonZeroU16,
        association: Preference,
    ) -> Self {
        Self {
            channels,
            dwell_millis,
            association,
        }
    }

    pub const fn channels(self) -> StationScanChannels {
        self.channels
    }

    pub const fn dwell_millis(self) -> u16 {
        self.dwell_millis.get()
    }

    pub const fn dwell(self) -> NonZeroU16 {
        self.dwell_millis
    }

    pub const fn association_preference(self) -> Preference {
        self.association
    }
}

/// Complete chip- and executor-independent candidate-discovery request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StationDiscovery {
    ssid: WifiSsid,
    scan: StationScanPolicy,
}

impl StationDiscovery {
    pub const fn new(ssid: WifiSsid, scan: StationScanPolicy) -> Self {
        Self { ssid, scan }
    }

    pub const fn ssid(self) -> WifiSsid {
        self.ssid
    }

    pub const fn scan(self) -> StationScanPolicy {
        self.scan
    }
}

#[cfg(test)]
mod tests;
