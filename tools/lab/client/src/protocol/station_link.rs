//! The router's answer to `RouterOperation::StationLink`: the AP's view of
//! one station's link, as its Linux wireless stack reports it: the station's counters and the rates of its last frames each way
//! (`iw dev <ap> station get`), the AP's channel and width (`iw dev <ap>
//! info`), the survey of the channel in use (`iw dev <ap> survey dump`) and
//! the station's transmit queues per TID (mac80211's per-station `aqm`
//! debugfs file) and the radio's own queue (the phy's `aqm` file), both
//! verbatim and typed ([`queue`]), and the radio's own counters
//! (mac80211's `statistics` debugfs files and mt76's BlockAck misses, when
//! the router has them). Directions are the AP's: `tx` is what the AP sent
//! the station. Each read carries the router's boot clock just before it.
//!
//! The router (`oer-tools-lab-router`, `station::link`) runs the commands
//! and parses their text into this record; clients read it here.
pub mod queue;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use self::queue::{RadioQueues, StationQueues};

/// What the AP reports of one station's link at one moment. Counters are
/// the station's since its association (the survey's since the radio
/// started); a counter or a value the driver does not report is absent.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StationLink {
    /// The router's boot clock just before `iw station get`, in
    /// milliseconds (the first field of its `/proc/uptime`).
    pub observed_at_ms: u64,
    /// When the station associated, on the same clock (`associated at
    /// [boottime]`): one association's identity.
    pub associated_at_boottime_ms: Option<u64>,
    /// The station's frames and bytes the AP received and sent.
    pub rx_bytes: u64,
    pub rx_packets: u64,
    pub tx_bytes: u64,
    pub tx_packets: u64,
    /// The AP's retransmissions to the station and the frames it gave up.
    pub tx_retries: u64,
    pub tx_failed: u64,
    /// The station's frames the AP dropped after receiving them.
    pub rx_drop_misc: Option<u64>,
    /// Air time of the station's frames each way, in microseconds.
    pub rx_duration_us: Option<u64>,
    pub tx_duration_us: Option<u64>,
    /// The averaged signal of the station's frames, in dBm.
    pub signal_avg_dbm: Option<i32>,
    /// The rate of the last frame from the station and of the last one to it.
    pub rx_bitrate: Option<Bitrate>,
    pub tx_bitrate: Option<Bitrate>,
    /// The AP's channel, its frequency and its width.
    pub channel: Option<u8>,
    pub frequency_mhz: Option<u32>,
    pub width_mhz: Option<u32>,
    /// The channel in use's survey.
    pub survey: Option<Survey>,
    /// The packets CoDel dropped from the station's TID 0 queue: the
    /// `drops` of `station_queues`' TID 0; absent without it.
    pub tid0_aqm_drops: Option<u64>,
    /// The station's per-TID queues; absent without a readable file.
    pub station_queues: Option<StationQueues>,
    /// The radio's queue, every station's; absent without a readable file.
    pub radio_queues: Option<RadioQueues>,
    /// The radio's counters by the name its driver gives them, not the
    /// station's: FCS errors, A-MPDUs and the like. Empty on a router
    /// without them.
    pub radio: BTreeMap<String, u64>,
}

/// The survey of the channel in use: the radio's time on it, busy, receiving
/// (and receiving its own BSS) and transmitting, in milliseconds, and its
/// noise floor.
#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Survey {
    pub frequency_mhz: u32,
    pub noise_dbm: Option<i32>,
    pub active_ms: Option<u64>,
    pub busy_ms: Option<u64>,
    pub receive_ms: Option<u64>,
    pub bss_receive_ms: Option<u64>,
    pub transmit_ms: Option<u64>,
}

/// The PHY format of a reported rate.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RateFormat {
    /// DSSS, CCK or OFDM: no MCS.
    Legacy,
    Ht,
    Vht,
    He,
    Eht,
}

/// One rate as `iw` prints it, and what it names.
///
/// SOURCE: iw 6.9 `station.c::parse_bitrate`: the rate in units of
/// 100 kbit/s as `"%d.%d MBit/s"` (`"(unknown)"` without one), then
/// ` MCS %d` (HT), ` VHT-MCS %d`, ` HE-MCS %d` or ` EHT-MCS %d`, a width
/// token other than 20 MHz (` 40MHz`, ` 80MHz`, ` 80P80MHz`, ` 160MHz`,
/// ` 320MHz`), ` short GI`, and the stream count (` VHT-NSS %d`,
/// ` HE-NSS %d`, ` EHT-NSS %d`).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Bitrate {
    /// The line as `iw` printed it.
    pub text: String,
    pub kbit_per_s: Option<u64>,
    pub format: RateFormat,
    pub mcs: Option<u8>,
    /// The spatial streams a VHT, HE or EHT rate names; an HT MCS implies
    /// its own.
    pub streams: Option<u8>,
    pub width_mhz: u32,
    pub short_gi: bool,
}
