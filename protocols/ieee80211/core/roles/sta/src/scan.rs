//! Station-owned BSS observations and candidate selection.

use oer_ieee80211_wire::scan::{ScanFrame, ScanRecord};
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

/// One BSS observation: the decoded beacon or probe response, the signal
/// level and the time the receiver took it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BssObservation {
    pub record: ScanRecord,
    /// The channel the station takes the BSS to be on: the DS Parameter
    /// Set's channel, or the receiver's when the frame carries none.
    pub channel: u8,
    /// The ERP protection the frame's ERP elements ask for
    /// ([`crate::association::erp_protection`]).
    pub erp: oer_ieee80211_wire::protection::ErpProtection,
    /// The received signal level in dBm, `None` when the receiver reported
    /// none; an unknown level ranks below every known one.
    pub rssi: Option<i8>,
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

    /// Parse and merge one beacon/probe response by BSSID, received on
    /// `receive_channel` at `received_at` on the receiver's monotonic clock,
    /// when known.
    pub fn observe_management(
        &mut self,
        frame: &[u8],
        receive_channel: u8,
        rssi: Option<i8>,
        received_at: Option<Instant>,
    ) -> ScanObservation {
        self.observe(frame, receive_channel, rssi, received_at, |_| true)
    }

    /// As [`Self::observe_management`], but a BSS the table does not hold
    /// yet enters it only when it names `ssid`: a connect's scan keeps its
    /// capacity for its own network wherever other networks crowd the air.
    /// A held BSS is still updated by any of its frames, so a hidden
    /// network's empty-SSID beacon refreshes the record its probe response
    /// made.
    pub fn observe_network(
        &mut self,
        frame: &[u8],
        receive_channel: u8,
        rssi: Option<i8>,
        received_at: Option<Instant>,
        ssid: &[u8],
    ) -> ScanObservation {
        self.observe(frame, receive_channel, rssi, received_at, |record| {
            record.ssid_bytes() == ssid
        })
    }

    fn observe(
        &mut self,
        frame: &[u8],
        receive_channel: u8,
        rssi: Option<i8>,
        received_at: Option<Instant>,
        admit: impl FnOnce(&ScanRecord) -> bool,
    ) -> ScanObservation {
        let Some(frame) = ScanFrame::parse(frame) else {
            return ScanObservation::Ignored;
        };
        let erp = crate::association::erp_protection(frame.erp_elements());
        let mut record = frame.into_record();
        // A frame without a DS Parameter Set names no channel: the BSS is
        // taken to be where the frame was heard.
        let channel = record.ds_channel.unwrap_or(receive_channel);

        for (index, existing) in self.records.iter_mut().take(self.length).enumerate() {
            let Some(BssObservation {
                record: existing,
                channel: existing_channel,
                erp: existing_erp,
                rssi: existing_rssi,
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
                    // the complete record, but retain the current signal
                    // sample used to select among BSSIDs and this frame's
                    // ERP decision: either its ERP elements were checked
                    // whole, or their damage asks for protection. The next
                    // complete frame sets it anew.
                    if *existing_rssi == rssi && *existing_erp == erp {
                        return ScanObservation::Duplicate { index };
                    }
                    *existing_rssi = rssi;
                    *existing_erp = erp;
                    return ScanObservation::Updated { index };
                }
                if *existing == record
                    && *existing_channel == channel
                    && *existing_erp == erp
                    && *existing_rssi == rssi
                    && *existing_received_at == received_at
                {
                    return ScanObservation::Duplicate { index };
                }
                *existing = record;
                *existing_channel = channel;
                *existing_erp = erp;
                *existing_rssi = rssi;
                *existing_received_at = received_at;
                return ScanObservation::Updated { index };
            }
        }

        if !admit(&record) {
            return ScanObservation::Ignored;
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
            channel,
            erp,
            rssi,
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
    observations: impl IntoIterator<Item = &'a BssObservation>,
    ssid: &[u8],
) -> Option<&'a BssObservation> {
    observations
        .into_iter()
        .filter(|observation| {
            observation.record.ssid_bytes() == ssid && (1..=13).contains(&observation.channel)
        })
        .max_by_key(|observation| observation.rssi)
}

/// Select the strongest exact SSID match that admits the station's security
/// policy.
pub fn best_matching_ssid_and_security<'a>(
    observations: impl IntoIterator<Item = &'a BssObservation>,
    ssid: &[u8],
    policy: crate::security::StaSecurityPolicy,
) -> Option<&'a BssObservation> {
    observations
        .into_iter()
        .filter(|observation| {
            let record = &observation.record;
            record.ssid_bytes() == ssid
                && (1..=13).contains(&observation.channel)
                && crate::security::select_association_rsn(record, policy).is_ok()
        })
        .max_by_key(|observation| observation.rssi)
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
