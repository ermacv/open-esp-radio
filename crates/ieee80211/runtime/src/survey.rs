//! An acceptance-check instrument for the W1 passive scan: the Station and
//! product paths never call it; it only consumes
//! [`oer_ieee80211_sta::receive::classify`] and
//! [`oer_ieee80211_sta::receive::observe_scan`].
//!
//! [`Survey::take`] classifies every received MPDU, records beacons and
//! probe responses in its scan table and keeps one [`BssTally`] per
//! BSSID, under the table's index. It reads only a record's beacon
//! interval, channel and timestamp.
//!
//! Stamps are the receiver's raw 32-bit microsecond receive stamps; their
//! differences wrap, so a span or gap is exact below 2³² µs. The handoff
//! lag is the low 32 bits of the handoff time less the stamp, wrapping: an
//! exact latency when both read the same clock, otherwise that latency plus
//! a constant offset, whose spread (maximum less minimum) remains exact.
//!
//! Received bytes reach this module; nothing here panics on them.

use core::num::NonZeroU64;
use oer_base_time::Instant;
use oer_ieee80211_lower_mac::{RxEvidence, RxMetadata};
use oer_ieee80211_mac::address::Bssid;
use oer_ieee80211_mac::scan::{ScanObservation, ScanRecord, ScanTable};
use oer_ieee80211_sta::receive::{Received, classify, observe_scan};

/// The gaps the survey logs for its stand BSSID; later ones are counted in
/// [`Survey::gap_log_dropped`].
pub const GAP_LOG_CAPACITY: usize = 64;

/// The Frame Control subtype of a beacon.
const BEACON: u16 = 8;
/// The Frame Control subtype of a probe response.
const PROBE_RESPONSE: u16 = 5;
/// Microseconds per time unit.
const TIME_UNIT_MICROS: u64 = 1024;

/// The lowest and highest of a sampled value.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MinMax<T> {
    pub min: T,
    pub max: T,
}

impl<T: Copy + Ord> MinMax<T> {
    fn widen(range: &mut Option<Self>, value: T) {
        *range = Some(match *range {
            None => Self {
                min: value,
                max: value,
            },
            Some(Self { min, max }) => Self {
                min: min.min(value),
                max: max.max(value),
            },
        });
    }
}

/// What the survey saw of one BSSID.
///
/// Beacons alone feed the gap and missed-beacon statistics: a probe
/// response answers a probe and is no periodic sample. A beacon or probe
/// response whose TSF repeats the previous one from its BSSID is a
/// duplicate: it is counted and feeds nothing else. A beacon with truncated
/// elements after a complete one leaves the stored record, and so its
/// timestamp, unchanged, and is counted as a duplicate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BssTally {
    /// Beacons, duplicates included.
    pub beacons: u32,
    /// Probe responses, duplicates included.
    pub probe_responses: u32,
    /// Frames whose TSF repeated the previous frame's.
    pub duplicates: u32,
    /// The received signal in dBm of the frames that carried one.
    pub rssi_dbm: Option<MinMax<i8>>,
    /// Frames without a received signal level.
    pub without_rssi: u32,
    /// Frames without a receive stamp.
    pub without_stamp: u32,
    /// The receive stamp of the first stamped frame.
    pub first_stamp: Option<u32>,
    /// The receive stamp of the latest stamped frame.
    pub last_stamp: Option<u32>,
    /// The handoff time of the first stamped frame, in µs, low 32 bits: the
    /// clock the stamps are compared with.
    pub first_handoff: Option<u32>,
    /// The handoff time of the latest stamped frame, in µs, low 32 bits.
    pub last_handoff: Option<u32>,
    /// The longest time between two consecutive stamped beacons, in µs.
    pub max_beacon_gap_us: u32,
    /// Gaps between consecutive stamped beacons longer than 1.5 beacon
    /// intervals, the interval the later beacon advertises.
    pub late_gaps: u32,
    /// Beacons missing between consecutive beacons by their TSF:
    /// round(ΔTSF / interval) − 1 each.
    pub missed_beacons: u32,
    /// Beacons whose TSF was below the previous beacon's: the access point
    /// restarted its TSF, and no beacon count is taken across it.
    pub tsf_regressions: u32,
    /// Beacons advertising a zero beacon interval, which take no gap
    /// statistics.
    pub zero_interval: u32,
    /// The handoff time less the receive stamp of the stamped frames, in µs.
    pub handoff_lag_us: Option<MinMax<u32>>,
    last_tsf: Option<u64>,
    last_beacon: Option<Beacon>,
}

/// The previous beacon's place, for the next one's gaps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Beacon {
    tsf: u64,
    stamp: Option<u32>,
}

impl BssTally {
    const EMPTY: Self = Self {
        beacons: 0,
        probe_responses: 0,
        duplicates: 0,
        rssi_dbm: None,
        without_rssi: 0,
        without_stamp: 0,
        first_stamp: None,
        last_stamp: None,
        first_handoff: None,
        last_handoff: None,
        max_beacon_gap_us: 0,
        late_gaps: 0,
        missed_beacons: 0,
        tsf_regressions: 0,
        zero_interval: 0,
        handoff_lag_us: None,
        last_tsf: None,
        last_beacon: None,
    };

    /// The time from the first to the latest stamped frame, in µs.
    pub fn stamp_span_us(&self) -> Option<u32> {
        Some(self.last_stamp?.wrapping_sub(self.first_stamp?))
    }

    /// The time from the first to the latest stamped frame's handoff, in
    /// µs. Over the same frames as [`Self::stamp_span_us`], their ratio is
    /// the stamp clock's rate against the handoff clock.
    pub fn handoff_span_us(&self) -> Option<u32> {
        Some(self.last_handoff?.wrapping_sub(self.first_handoff?))
    }
}

/// One beacon gap of the stand BSSID longer than 1.5 beacon intervals.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Gap {
    /// The receive stamp of the beacon that ended the gap.
    pub stamp: u32,
    /// The time since the previous beacon, in µs.
    pub gap_us: u32,
    /// The beacons the TSF says are missing in the gap; `None` when the TSF
    /// ran backwards.
    pub missed_by_tsf: Option<u32>,
}

/// Every received frame the survey counted, by what became of it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SurveyCounters {
    /// Every frame taken.
    pub frames: u32,
    /// Frames whose MAC header [`classify`] refused.
    pub header_refusals: u32,
    /// Data frames.
    pub data: u32,
    /// Management frames that are no well-formed beacon or probe response.
    pub ignored_management: u32,
    /// Beacons and probe responses of a new BSSID the full table refused
    /// (the table's `dropped_unique_bss`).
    pub table_full: u32,
    /// Beacons and probe responses whose DS Parameter Set names another
    /// channel than the one the receiver was configured to.
    pub off_channel: u32,
}

/// The W1 passive-scan survey over a scan table of `N` BSSIDs.
pub struct Survey<const N: usize> {
    table: ScanTable<N>,
    tallies: [BssTally; N],
    counters: SurveyCounters,
    stand: Bssid,
    gaps: heapless::Vec<Gap, GAP_LOG_CAPACITY>,
    gaps_dropped: u32,
}

impl<const N: usize> Survey<N> {
    /// An empty survey that logs the beacon gaps of `stand`.
    pub const fn new(stand: Bssid) -> Self {
        Self {
            table: ScanTable::new(),
            tallies: [BssTally::EMPTY; N],
            counters: SurveyCounters {
                frames: 0,
                header_refusals: 0,
                data: 0,
                ignored_management: 0,
                table_full: 0,
                off_channel: 0,
            },
            stand,
            gaps: heapless::Vec::new(),
            gaps_dropped: 0,
        }
    }

    /// Take one received MPDU: received at `now`, with the receiver's
    /// `meta`, its raw receive `stamp` and the time the receiver handed it
    /// over.
    pub fn take(
        &mut self,
        now: Instant,
        mpdu: &[u8],
        meta: &impl RxMetadata,
        stamp: Option<u32>,
        handoff: Instant,
    ) {
        self.counters.frames = self.counters.frames.saturating_add(1);
        let management = match classify(mpdu) {
            Err(_) => {
                self.counters.header_refusals = self.counters.header_refusals.saturating_add(1);
                return;
            }
            Ok(Received::Data(_)) => {
                self.counters.data = self.counters.data.saturating_add(1);
                return;
            }
            Ok(Received::Management(management)) => management,
        };
        let subtype = management.subtype();
        let index = match observe_scan(&mut self.table, now, management, meta) {
            ScanObservation::Ignored => {
                self.counters.ignored_management =
                    self.counters.ignored_management.saturating_add(1);
                return;
            }
            // The table counts it in its summary.
            ScanObservation::Dropped => return,
            ScanObservation::Inserted { index }
            | ScanObservation::Updated { index }
            | ScanObservation::Duplicate { index } => index,
        };
        // An observation names an occupied cell of the table's `N`, and the
        // tallies have `N` cells.
        let (Some(record), Some(tally)) = (self.table.get(index), self.tallies.get_mut(index))
        else {
            return;
        };
        let common = meta.common();
        if record.channel != common.channel.number() {
            self.counters.off_channel = self.counters.off_channel.saturating_add(1);
        }
        let rssi = match common.rssi_dbm {
            RxEvidence::HardwareObserved(rssi) | RxEvidence::ProtocolValidated(rssi) => Some(rssi),
            RxEvidence::Unavailable => None,
        };
        let gap = tally.take(record, subtype, rssi, stamp, handoff);
        if let Some(gap) = gap
            && record.bssid == self.stand
            && self.gaps.push(gap).is_err()
        {
            self.gaps_dropped = self.gaps_dropped.saturating_add(1);
        }
    }

    /// The scan table the survey fills.
    pub const fn table(&self) -> &ScanTable<N> {
        &self.table
    }

    /// The record and tally at the table's `index`.
    pub fn bss(&self, index: usize) -> Option<(&ScanRecord, &BssTally)> {
        Some((self.table.get(index)?, self.tallies.get(index)?))
    }

    /// Every BSSID's record and tally, in the table's order.
    pub fn entries(&self) -> impl Iterator<Item = (&ScanRecord, &BssTally)> {
        self.table.records().zip(self.tallies.iter())
    }

    /// The frame counters.
    pub fn counters(&self) -> SurveyCounters {
        SurveyCounters {
            table_full: self.table.summary().dropped_unique_bss,
            ..self.counters
        }
    }

    /// The stand BSSID's late beacon gaps, oldest first.
    pub fn gap_log(&self) -> &[Gap] {
        &self.gaps
    }

    /// The stand BSSID's late beacon gaps the full log did not keep.
    pub const fn gap_log_dropped(&self) -> u32 {
        self.gaps_dropped
    }
}

impl BssTally {
    /// Count one frame of this BSSID, whose record is now `record`: a late
    /// beacon gap, when it ended one.
    fn take(
        &mut self,
        record: &ScanRecord,
        subtype: u16,
        rssi: Option<i8>,
        stamp: Option<u32>,
        handoff: Instant,
    ) -> Option<Gap> {
        let beacon = subtype == BEACON;
        if beacon {
            self.beacons = self.beacons.saturating_add(1);
        } else if subtype == PROBE_RESPONSE {
            self.probe_responses = self.probe_responses.saturating_add(1);
        }
        match rssi {
            Some(rssi) => MinMax::widen(&mut self.rssi_dbm, rssi),
            None => self.without_rssi = self.without_rssi.saturating_add(1),
        }
        match stamp {
            Some(stamp) => {
                // The low 32 bits of the handoff time, on the stamp's wrap.
                let handoff = handoff.as_micros() as u32;
                self.first_stamp.get_or_insert(stamp);
                self.last_stamp = Some(stamp);
                self.first_handoff.get_or_insert(handoff);
                self.last_handoff = Some(handoff);
                let lag = handoff.wrapping_sub(stamp);
                MinMax::widen(&mut self.handoff_lag_us, lag);
            }
            None => self.without_stamp = self.without_stamp.saturating_add(1),
        }
        let tsf = record.timestamp.as_micros();
        if self.last_tsf.replace(tsf) == Some(tsf) {
            self.duplicates = self.duplicates.saturating_add(1);
            return None;
        }
        if !beacon {
            return None;
        }
        let previous = self.last_beacon.replace(Beacon { tsf, stamp });
        let Some(interval) = u64::from(record.beacon_interval_tu)
            .checked_mul(TIME_UNIT_MICROS)
            .and_then(NonZeroU64::new)
        else {
            self.zero_interval = self.zero_interval.saturating_add(1);
            return None;
        };
        let previous = previous?;
        let missed = match tsf.checked_sub(previous.tsf) {
            Some(elapsed) => {
                let missed = missed_beacons(elapsed, interval);
                self.missed_beacons = self.missed_beacons.saturating_add(missed);
                Some(missed)
            }
            None => {
                self.tsf_regressions = self.tsf_regressions.saturating_add(1);
                None
            }
        };
        let gap_us = stamp?.wrapping_sub(previous.stamp?);
        self.max_beacon_gap_us = self.max_beacon_gap_us.max(gap_us);
        // 1.5 intervals, compared doubled to stay in integers.
        let late = u64::from(gap_us)
            .checked_mul(2)
            .zip(interval.get().checked_mul(3))
            .is_some_and(|(gap, limit)| gap > limit);
        if !late {
            return None;
        }
        self.late_gaps = self.late_gaps.saturating_add(1);
        Some(Gap {
            stamp: stamp?,
            gap_us,
            missed_by_tsf: missed,
        })
    }
}

/// The beacons missing in `elapsed` µs of TSF between two received beacons
/// `interval` µs apart: round(elapsed / interval) − 1, at least zero and
/// saturating at `u32::MAX`.
fn missed_beacons(elapsed: u64, interval: NonZeroU64) -> u32 {
    let half = interval.get().div_ceil(2);
    let periods = elapsed.saturating_add(half) / interval;
    u32::try_from(periods.saturating_sub(1)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
