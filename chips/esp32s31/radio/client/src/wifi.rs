//! Wi-Fi as a client of the radio system.
//!
//! The driver lends its cold partition to `join` and gets it back clocked:
//! the actor registers the PHY domain or wakes closed RF, enables
//! coexistence, enters common power, enables the Wi-Fi module clocks and
//! enters Wi-Fi into the PHY client set. It lends the clocked client to
//! `track` and gets it back tuned: the actor runs the initial tracking,
//! enables Wi-Fi RX and selects the initial channel. A tracked member lends
//! its clocks' proof to `reset_mac` and gets it back with the proof of its
//! MAC block's reset and the values its cold preparation reads; the
//! preparation is the member's own. It lends the clocked client, with the MAC
//! stopped, to `leave` and gets it back cold.

use oer_chips_esp32s31_hal::radio::ieee80211::client::{
    ClockPoisoned, StoppedInstallation, WifiClocked, WifiClocksOn, WifiCold, WifiPowered,
};
use oer_chips_esp32s31_hal::radio::ieee80211::init::{
    MacBlockReset, MacCoexEvent, MacCoexPti, MacCoexPtiSource, MacTxPowerSource,
};
use oer_chips_esp32s31_hal::types::MacTxPowerPair;
use oer_chips_esp32s31_radio_phy::{PhyTxTargetPowerProfile, tracking::TemperatureObservation};
use oer_protocols_ieee80211_wire::channel::Ghz2_4Channel;
use oer_protocols_radio_regulatory::{Permitted, TxCeiling};

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
    type Retune = WifiRetune;
    type Retuned = WifiRetuned;
    type RetuneReturned = WifiClocked;
    type Leave = WifiLeave;
    type Left = WifiLeft;
    type ResetMac = WifiResetMac;
    type MacReset = WifiMacReset;

    fn join_returned(join: WifiJoin) -> WifiReturned {
        WifiReturned::Cold(join.wifi)
    }

    fn retune_returned(retune: WifiRetune) -> WifiClocked {
        retune.wifi
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
    /// The module clocks did not release: the registers stay installed and
    /// the clock obligation unresolved until reset.
    ClockPoisoned {
        stopped: StoppedInstallation,
        obligation: ClockPoisoned,
    },
}

/// A successful `join`: the clocked Wi-Fi client.
pub struct WifiJoined {
    pub wifi: WifiClocked,
    pub phy: PhyPreparation,
}

/// `track`: the clocked client, the initial channel and its TX ceiling.
pub struct WifiTrack {
    pub wifi: WifiClocked,
    pub channel: Permitted<oer_protocols_ieee80211_contracts_port::RegulatedChannel<Ghz2_4Channel>>,
    pub tx_ceiling: TxCeiling,
}

/// What the initial tracking and the channel observed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrackReport {
    /// The initial tracking's duration; `None` when tracking was not due.
    pub tracking_micros: Option<u64>,
    /// The PHY's retained temperature after the initial tracking and
    /// channel.
    pub temperature: TemperatureObservation,
    pub tx_power: PhyTxTargetPowerProfile,
    pub probe_priority: ProbePriority,
}

/// A successful `track`: the clocked client, tuned.
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

/// A channel transaction with the Wi-Fi runtime completely stopped.
pub struct WifiRetune {
    pub wifi: WifiClocked,
    pub channel: Permitted<oer_protocols_ieee80211_contracts_port::RegulatedChannel<Ghz2_4Channel>>,
    pub tx_ceiling: TxCeiling,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RetuneReport {
    /// Full channel transaction on the radio system's timer.
    pub micros: u64,
    pub tx_power: PhyTxTargetPowerProfile,
    pub probe_priority: ProbePriority,
    pub temperature: TemperatureObservation,
}

pub struct WifiRetuned {
    pub wifi: WifiClocked,
    pub report: RetuneReport,
}

/// PTI-only service of a Probe Request, read by the coexistence owner from
/// the shared (client, operation) table and returned with channel preparation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProbePriority {
    pub packet: MacCoexPti,
    pub scheduler: MacCoexPti,
}

/// `reset_mac`: a tracked member's clocks' proof, and the channel it is tuned
/// to, whose PHY field inside the MAC's block the reset writes back.
pub struct WifiResetMac {
    pub clocks: WifiClocksOn,
    pub channel: Ghz2_4Channel,
}

/// The answer to `reset_mac`: the clocks' proof back, the reset's proof the
/// cold preparation takes, and the values the preparation reads.
pub struct WifiMacReset {
    pub clocks: WifiClocksOn,
    pub reset: MacBlockReset,
    pub sources: MacColdSources,
}

/// The values of the radio system's that the MAC's cold preparation reads,
/// copied at the reset: the coexistence priorities of the four events it
/// queries, in `MacColdCoexPti::query` order (events 1, 3, 10, 15), and the
/// calibrated TX power profile under the member's ceiling.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MacColdSources {
    pub pti: [MacCoexPti; 4],
    pub tx_power: PhyTxTargetPowerProfile,
}

/// The coexistence events the cold preparation queries, in its order.
pub const MAC_COEX_EVENTS: [MacCoexEvent; 4] = [
    MacCoexEvent::Event1,
    MacCoexEvent::Event3,
    MacCoexEvent::Event10,
    MacCoexEvent::Event15,
];

impl MacCoexPtiSource for MacColdSources {
    fn mac_coex_pti(&mut self, event: MacCoexEvent) -> MacCoexPti {
        let [one, three, ten, fifteen] = self.pti;
        match event {
            MacCoexEvent::Event1 => one,
            MacCoexEvent::Event3 => three,
            MacCoexEvent::Event10 => ten,
            MacCoexEvent::Event15 => fifteen,
        }
    }
}

impl MacTxPowerSource for MacColdSources {
    fn mac_tx_power_pair(&mut self, rate: u8) -> MacTxPowerPair {
        let pair = self.tx_power.pair(rate);
        MacTxPowerPair {
            primary: pair.primary,
            alternate: pair.alternate,
        }
    }
}
