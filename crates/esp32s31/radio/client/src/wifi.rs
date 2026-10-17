//! Wi-Fi as a client of the radio system.
//!
//! The driver lends its cold partition to `join` and gets it back clocked:
//! the actor registers the PHY domain or wakes closed RF, enables
//! coexistence, enters common power, enables the Wi-Fi module clocks and
//! enters Wi-Fi into the PHY client set. It lends the clocked client to
//! `track` and gets it back with the MAC up: the actor runs the initial
//! tracking, enables Wi-Fi RX, selects the initial channel and runs the MAC
//! cold initialization. It lends the clocked client, with the MAC stopped, to
//! `leave` and gets it back cold.

use oer_esp32s31_hal::ieee80211::client::{WifiClocked, WifiCold, WifiPowered};
use oer_esp32s31_ieee80211_mac::init::{
    MacColdStartConfig, MacColdStartOutcome, MacSlowClockCalibration,
};
use oer_esp32s31_phy::{PhyTemperatureOutcome, PhyTxTargetPowerProfile};
use oer_ieee80211_mac::channel::WifiChannel;

use crate::{Client, PhyPreparation, sealed};

/// The doorbell bit of the Wi-Fi cell.
pub const BIT: u32 = 1 << 0;

/// The Wi-Fi client.
#[derive(Debug)]
pub enum Wifi {}

impl sealed::Sealed for Wifi {}

impl Client for Wifi {
    type Join = WifiJoin;
    type Joined = WifiJoined;
    type JoinReturned = WifiReturned;
    type Track = WifiTrack;
    type Tracked = WifiTracked;
    type TrackReturned = WifiClocked;
    type Leave = WifiLeave;
    type Left = WifiLeft;

    fn join_returned(join: WifiJoin) -> WifiReturned {
        WifiReturned::Cold(join.wifi)
    }

    fn track_returned(track: WifiTrack) -> WifiClocked {
        track.wifi
    }
}

/// `join`: the cold Wi-Fi partition.
pub struct WifiJoin {
    pub wifi: WifiCold,
}

/// The Wi-Fi partition at the stage a failed request left it.
pub enum WifiReturned {
    Cold(WifiCold),
    Powered(WifiPowered),
    Clocked(WifiClocked),
}

/// A successful `join`: the clocked Wi-Fi client.
pub struct WifiJoined {
    pub wifi: WifiClocked,
    pub phy: PhyPreparation,
}

/// `track`: the clocked client and the MAC bring-up inputs.
pub struct WifiTrack {
    pub wifi: WifiClocked,
    pub channel: WifiChannel,
    pub maximum_tx_power_quarter_dbm: i8,
    pub mac: MacColdStartConfig,
    pub slow_clock: MacSlowClockCalibration,
    pub delay_random: u32,
}

/// What the initial tracking and the MAC bring-up observed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackReport {
    /// The initial tracking's duration; `None` when tracking was not due.
    pub tracking_micros: Option<u64>,
    /// The temperature the tracking read, if it read one.
    pub temperature: Option<PhyTemperatureOutcome>,
    pub tx_power: PhyTxTargetPowerProfile,
    pub mac: MacColdStartOutcome,
}

/// A successful `track`: the clocked client with the MAC up.
pub struct WifiTracked {
    pub wifi: WifiClocked,
    pub report: TrackReport,
}

/// `leave`: the clocked client, with the MAC stopped.
pub struct WifiLeave {
    pub wifi: WifiClocked,
}

/// What `leave` hands back: the cold partition, or the stage a failed
/// release kept.
pub struct WifiLeft {
    pub wifi: WifiReturned,
}
