//! Wi-Fi's publication to the coexistence time-slice schedule.
//!
//! The vendor Wi-Fi library tells the schedule what Wi-Fi is doing through
//! one status word and a schedule interval, and restarts the phases after a
//! change while another radio shares the schedule. This module holds that
//! policy as values; the runtime applies them under the radio arbiter lease.
//!
//! SOURCE: complete pinned `libpp.a[pm.o]::pm_on_coex_schm_status_config`,
//! `pm_start` and `pm_on_coex_start`, and `libnet80211.a[ieee80211_scan.o]::
//! scan_next_channel` and `scan_op_end`.

use oer_esp32s31_coex::wifi_status;

/// Schedule interval, in the schedule's 100 µs unit, while Wi-Fi is idle.
const IDLE_INTERVAL: u32 = 1_000;
/// Schedule interval while scanning, or connecting under the normal
/// reconnect policy.
const ACTIVE_INTERVAL: u32 = 100;
/// Schedule interval while reconnecting under coexistence.
const RECONNECT_INTERVAL: u32 = 240;
/// Schedule interval unit, in microseconds.
const INTERVAL_UNIT_MICROS: u32 = 100;
/// Beacon interval the vendor assumes when an access point advertises none.
const DEFAULT_BEACON_INTERVAL_TU: u32 = 100;
/// The vendor's time unit, in microseconds.
const TU_MICROS: u32 = 1_024;

/// What Wi-Fi is doing, as far as the coexistence schedule is concerned.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiCoexActivity {
    /// No scan, connection or association: an idle station, a started
    /// access point or a stopped radio.
    Idle,
    /// A station scan dwells on its channels.
    Scanning,
    /// A station authenticates, associates or completes its key handshake.
    Connecting {
        /// The vendor reconnect policy under coexistence is active: the
        /// station lost its association while another radio was active.
        reconnecting: bool,
    },
    /// A station is associated with its access point.
    Connected {
        /// The access point's advertised beacon interval, in time units.
        beacon_interval_tu: u16,
    },
}

/// One publication of Wi-Fi's activity to the schedule.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiCoexStatusUpdate {
    /// The Wi-Fi status word that replaces every previous Wi-Fi bit.
    pub status: u16,
    /// The schedule interval to set, in the schedule's 100 µs unit.
    pub interval: u32,
    /// Restart the phases when another radio is active under coexistence.
    /// A connected station instead restarts them at its next beacon.
    pub restart_when_shared: bool,
}

impl WifiCoexActivity {
    /// The status word, interval and restart rule of this activity.
    ///
    /// The vendor clears every Wi-Fi status bit, sets the one bit of the new
    /// state and derives the interval from that bit alone. A connected
    /// station's interval is its beacon interval in 100 µs units, so one
    /// period of a scheme spans that many beacon intervals.
    pub const fn status_update(self) -> WifiCoexStatusUpdate {
        match self {
            Self::Idle => WifiCoexStatusUpdate {
                status: 0,
                interval: IDLE_INTERVAL,
                restart_when_shared: true,
            },
            Self::Scanning => WifiCoexStatusUpdate {
                status: wifi_status::SCAN,
                interval: ACTIVE_INTERVAL,
                restart_when_shared: true,
            },
            Self::Connecting { reconnecting } => WifiCoexStatusUpdate {
                status: wifi_status::CONNECTING,
                interval: if reconnecting {
                    RECONNECT_INTERVAL
                } else {
                    ACTIVE_INTERVAL
                },
                restart_when_shared: true,
            },
            Self::Connected { beacon_interval_tu } => WifiCoexStatusUpdate {
                status: wifi_status::CONNECTED,
                interval: connected_beacon_interval_micros(beacon_interval_tu)
                    / INTERVAL_UNIT_MICROS,
                restart_when_shared: false,
            },
        }
    }
}

/// The beacon interval power management runs on, in microseconds: the
/// advertised interval, or 100 TU when the access point advertises none.
///
/// SOURCE: complete pinned `libpp.a[pm.o]::pm_start` (`0x17c..0x186`).
pub const fn connected_beacon_interval_micros(beacon_interval_tu: u16) -> u32 {
    let tu = if beacon_interval_tu == 0 {
        DEFAULT_BEACON_INTERVAL_TU
    } else {
        beacon_interval_tu as u32
    };
    tu * TU_MICROS
}

/// Dwell of one scan channel while another radio is active under
/// coexistence, in milliseconds.
///
/// The vendor replaces the requested dwell with ten milliseconds per period
/// of the current scheme, scaled by the requested dwell over 120 ms when the
/// request is longer than that.
///
/// SOURCE: complete pinned `libnet80211.a[ieee80211_scan.o]::
/// scan_next_channel` (`0x21c..0x23e`).
pub const fn shared_scan_dwell_millis(requested_millis: u32, current_period: u8) -> u32 {
    const REFERENCE_DWELL_MILLIS: u32 = 120;
    let per_period = current_period as u32 * 10;
    if requested_millis > REFERENCE_DWELL_MILLIS {
        requested_millis * per_period / REFERENCE_DWELL_MILLIS
    } else {
        per_period
    }
}

/// The Wi-Fi channel as the vendor records it for coexistence: the primary
/// channel number and its secondary channel offset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiCoexChannel {
    pub primary: u8,
    /// `0` without a secondary channel, `1` above and `2` below the primary.
    pub secondary: u8,
}

impl WifiCoexChannel {
    /// The coexistence channel of one PHY channel request: a channel number
    /// with `cbw` 0, or an HT40 centre frequency with `cbw` 2 (secondary
    /// above) or 3 (secondary below).
    ///
    /// SOURCE: complete pinned `libnet80211.a[ieee80211_ht.o]::
    /// ieee80211_update_channel` passes the primary channel and its
    /// `wifi_second_chan_t` offset to `coex_wifi_channel_set`; the Bluetooth
    /// consumer `libbredr_app.a[bt_coex_schm_adapter.c.o]::
    /// r_bt_coex_schm_calculate_afh_by_wifi_channel` reads offset 1 as the
    /// channel four above the primary and 2 as four below.
    pub const fn from_phy_request(channel_or_frequency: u16, cbw: u8) -> Option<Self> {
        const SECONDARY_ABOVE_CENTRE_OFFSET_MHZ: u16 = 10;
        let (primary_frequency, secondary) = match cbw {
            0 => {
                return match channel_or_frequency {
                    1..=14 => Some(Self {
                        primary: channel_or_frequency as u8,
                        secondary: 0,
                    }),
                    _ => None,
                };
            }
            2 => (
                channel_or_frequency.wrapping_sub(SECONDARY_ABOVE_CENTRE_OFFSET_MHZ),
                1,
            ),
            3 => (
                channel_or_frequency.wrapping_add(SECONDARY_ABOVE_CENTRE_OFFSET_MHZ),
                2,
            ),
            _ => return None,
        };
        match primary_frequency {
            2_412..=2_472 if (primary_frequency - 2_407) % 5 == 0 => Some(Self {
                primary: ((primary_frequency - 2_407) / 5) as u8,
                secondary,
            }),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests;
