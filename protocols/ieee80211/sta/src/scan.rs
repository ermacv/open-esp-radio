//! Station-owned BSS observations and candidate selection.

use oer_ieee80211_wire::scan::{ScanRecord, parse_management};
use oer_time::Instant;

pub const SCAN_RECORD_CAPACITY: usize = 32;

/// The scan result: its records and whether it holds every BSS heard.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanSummary {
    pub records: usize,
    pub completeness: ScanCompleteness,
}

/// Whether a scan table holds every BSS it heard since it was cleared.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanCompleteness {
    /// Every BSS heard has its record.
    Complete,
    /// The table was full when a new BSS was heard: the result misses at
    /// least that one.
    Truncated,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanObservation {
    Ignored,
    Inserted { index: usize },
    Updated { index: usize },
    Duplicate { index: usize },
    Dropped,
}

/// One BSS observation: the decoded beacon or probe response and the time
/// the receiver took it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BssObservation {
    pub record: ScanRecord,
    /// The receiver's monotonic time of this frame's reception, from its
    /// receive timestamp; a station advances the record's `timestamp` by the
    /// time since to set its own TSF. `None` when the receiver had no
    /// reception time for the frame.
    pub received_at: Option<Instant>,
}

/// Fixed-capacity BSS table owned by one radio task.
pub struct ScanTable<const N: usize = SCAN_RECORD_CAPACITY> {
    records: [Option<BssObservation>; N],
    length: usize,
    completeness: ScanCompleteness,
}

impl<const N: usize> ScanTable<N> {
    pub const fn new() -> Self {
        Self {
            records: [None; N],
            length: 0,
            completeness: ScanCompleteness::Complete,
        }
    }

    pub fn clear(&mut self) {
        self.records.fill(None);
        self.length = 0;
        self.completeness = ScanCompleteness::Complete;
    }

    /// Present observations in insertion order, excluding vacant cells.
    pub fn records(&self) -> impl Iterator<Item = &ScanRecord> + Clone {
        self.observations().map(|observation| &observation.record)
    }

    /// Present observations with their reception times, in insertion order.
    pub fn observations(&self) -> impl Iterator<Item = &BssObservation> + Clone {
        self.records.iter().flatten()
    }

    /// The observation at `index`, as a [`ScanObservation`] names it;
    /// `None` for a vacant cell or an index past the table.
    pub fn get(&self, index: usize) -> Option<&ScanRecord> {
        self.records
            .get(index)?
            .as_ref()
            .map(|observation| &observation.record)
    }

    pub fn summary(&self) -> ScanSummary {
        ScanSummary {
            records: self.length,
            completeness: self.completeness,
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

        for (index, existing) in self.records.iter_mut().take(self.length).enumerate() {
            let Some(BssObservation {
                record: existing,
                received_at: existing_received_at,
            }) = existing
            else {
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
                if *existing == record && *existing_received_at == received_at {
                    return ScanObservation::Duplicate { index };
                }
                *existing = record;
                *existing_received_at = received_at;
                return ScanObservation::Updated { index };
            }
        }

        let index = self.length;
        // Every cell below `length` is occupied, so the first vacant one is
        // `length`; there is none when the table holds `N` records.
        let Some(vacant) = self.records.get_mut(index) else {
            self.completeness = ScanCompleteness::Truncated;
            return ScanObservation::Dropped;
        };
        *vacant = Some(BssObservation {
            record,
            received_at,
        });
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
    policy: crate::security::StaSecurityPolicy,
) -> Option<&'a ScanRecord> {
    records
        .into_iter()
        .filter(|record| {
            record.ssid_bytes() == ssid
                && (1..=13).contains(&record.channel)
                && crate::security::select_association_rsn(record, policy).is_ok()
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
            completeness: ScanCompleteness::Complete,
        })
    }
}
