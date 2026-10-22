//! Station-owned BSS observations and candidate selection.

use oer_ieee80211_wire::scan::{ScanRecord, parse_management};
use oer_time::Instant;

pub const SCAN_RECORD_CAPACITY: usize = 32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanSummary {
    pub records: usize,
    pub observed_frames: u32,
    pub dropped_unique_bss: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanObservation {
    Ignored,
    Inserted { index: usize },
    Updated { index: usize },
    Duplicate { index: usize },
    Dropped,
}

/// Fixed-capacity BSS table owned by one radio task.
pub struct ScanTable<const N: usize = SCAN_RECORD_CAPACITY> {
    records: [Option<ScanRecord>; N],
    length: usize,
    observed_frames: u32,
    dropped_unique_bss: u32,
}

impl<const N: usize> ScanTable<N> {
    pub const fn new() -> Self {
        Self {
            records: [None; N],
            length: 0,
            observed_frames: 0,
            dropped_unique_bss: 0,
        }
    }

    pub fn clear(&mut self) {
        self.records.fill(None);
        self.length = 0;
        self.observed_frames = 0;
        self.dropped_unique_bss = 0;
    }

    /// Present observations in insertion order, excluding vacant cells.
    pub fn records(&self) -> impl Iterator<Item = &ScanRecord> + Clone {
        self.records.iter().flatten()
    }

    /// The observation at `index`, as a [`ScanObservation`] names it;
    /// `None` for a vacant cell or an index past the table.
    pub fn get(&self, index: usize) -> Option<&ScanRecord> {
        self.records.get(index)?.as_ref()
    }

    pub fn summary(&self) -> ScanSummary {
        ScanSummary {
            records: self.length,
            observed_frames: self.observed_frames,
            dropped_unique_bss: self.dropped_unique_bss,
        }
    }

    /// Parse and merge one beacon/probe response by BSSID, received at
    /// `received_at` on the receiver's monotonic clock, when known.
    pub fn observe_management(
        &mut self,
        frame: &[u8],
        fallback_channel: u8,
        rssi: i8,
        received_at: Option<Instant>,
    ) -> ScanObservation {
        let Some(mut record) = parse_management(frame, fallback_channel, rssi) else {
            return ScanObservation::Ignored;
        };
        record.received_at = received_at;
        self.observed_frames = self.observed_frames.saturating_add(1);

        for (index, existing) in self.records.iter_mut().take(self.length).enumerate() {
            let Some(existing) = existing else {
                continue;
            };
            if existing.bssid == record.bssid {
                // A BSS record is current control-plane state, not a sample
                // selected by peak RSSI. HT/ERP protection, WMM and Operation
                // elements can change while a BSSID remains the same. Keeping
                // the strongest historical frame would retain stale policy
                // indefinitely whenever a later beacon happened to be even
                // one dB weaker.
                //
                // A hidden-SSID beacon is the one exception to wholesale
                // replacement: preserve an SSID learned from a probe response
                // while still taking every current advertised parameter from
                // the latest complete frame.
                if record.ssid_bytes().is_empty() && !existing.ssid_bytes().is_empty() {
                    record.copy_ssid_from(existing);
                }
                if record.information_elements_truncated && !existing.information_elements_truncated
                {
                    // A malformed suffix cannot prove that a previously
                    // observed security/protection element disappeared. Keep
                    // the complete policy, but retain the current signal
                    // sample used to select among BSSIDs.
                    if existing.rssi == record.rssi {
                        return ScanObservation::Duplicate { index };
                    }
                    existing.rssi = record.rssi;
                    return ScanObservation::Updated { index };
                }
                if *existing == record {
                    return ScanObservation::Duplicate { index };
                }
                *existing = record;
                return ScanObservation::Updated { index };
            }
        }

        let index = self.length;
        // Every cell below `length` is occupied, so the first vacant one is
        // `length`; there is none when the table holds `N` records.
        let Some(vacant) = self.records.get_mut(index) else {
            self.dropped_unique_bss = self.dropped_unique_bss.saturating_add(1);
            return ScanObservation::Dropped;
        };
        *vacant = Some(record);
        // `index` names a cell of an array of `N` cells, so `index + 1 <= N`.
        self.length = index.wrapping_add(1);
        ScanObservation::Inserted { index }
    }
}

impl<const N: usize> Default for ScanTable<N> {
    fn default() -> Self {
        Self::new()
    }
}

/// Select the strongest complete observation with an exact SSID match.
pub fn best_matching_ssid<'a>(
    records: impl IntoIterator<Item = &'a ScanRecord>,
    ssid: &[u8],
) -> Option<&'a ScanRecord> {
    records
        .into_iter()
        .filter(|record| record.ssid_bytes() == ssid && (1..=13).contains(&record.channel))
        .max_by_key(|record| record.rssi)
}

/// Select the strongest exact SSID match that admits the station's security
/// policy.
pub fn best_matching_ssid_and_security<'a>(
    records: impl IntoIterator<Item = &'a ScanRecord>,
    ssid: &[u8],
    policy: oer_ieee80211_wire::security::StaSecurityPolicy,
) -> Option<&'a ScanRecord> {
    records
        .into_iter()
        .filter(|record| {
            record.ssid_bytes() == ssid
                && (1..=13).contains(&record.channel)
                && oer_ieee80211_wire::station::select_association_rsn(record, policy).is_ok()
        })
        .max_by_key(|record| record.rssi)
}

#[cfg(test)]
mod tests;

#[cfg(feature = "placement")]
impl<const N: usize> ScanTable<N> {
    /// Construct the BSS table directly in the caller's permanent storage.
    pub fn init_in_place(storage: &mut core::mem::MaybeUninit<Self>) -> &mut Self {
        oer_memory::init_in_place!(storage, Self {
            records => |place| oer_memory::init_array(place, |_| None),
            length: 0,
            observed_frames: 0,
            dropped_unique_bss: 0,
        })
    }
}
