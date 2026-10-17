//! Allocation-free scan records and management-frame parsing.
//!
//! This is the maintained source-owned passive-scan frontier. Channel changes,
//! dwell timers, and RX ownership remain explicit responsibilities of the
//! radio owner; this module owns only bounded observations and selection. The
//! implementation is chip-independent and forms the maintained upper-stack
//! extraction.

use crate::address::{Bssid, IndividualAddress};
use crate::ht::{HtPeerCapabilities, ht_peer_capabilities};
use crate::security::LinkProtection;
use oer_time::Instant;

pub const SCAN_RECORD_CAPACITY: usize = 32;
pub const RSN_IE_CAPACITY: usize = 64;
pub const RSNXE_CAPACITY: usize = 16;
pub const EXTENDED_RATES_CAPACITY: usize = 16;
pub const HT_CAPABILITY_IE_LEN: usize = 28;
pub const HT_OPERATION_IE_LEN: usize = 24;
pub const HE_CAPABILITY_IE_CAPACITY: usize = 64;
pub const HE_OPERATION_IE_CAPACITY: usize = 32;
pub const WMM_IE_CAPACITY: usize = 26;

const HE_CAPABILITIES_EXTENSION_ID: u8 = 35;
const HE_OPERATION_EXTENSION_ID: u8 = 36;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HtSecondaryChannel {
    Above,
    Below,
}

/// Recover the Default PE Duration advertised in an HE Operation IE.
///
/// A stored extension IE includes `[element_id, length, extension_id]`
/// before its payload, so HE Operation Parameters byte zero is byte three.
/// The field is its low three bits. This layout and its direct use by the
/// ESP32-S31 MAC are independently confirmed by complete
/// `libnet80211.a[ieee80211_he.o]::ieee80211_parse_heopr` and
/// `libpp.a[hal_mac_ctl.o]::hal_he_set_default_pe`.
pub fn he_default_packet_extension_duration(he_operation_ie: &[u8]) -> Option<u8> {
    if he_operation_ie.len() < 4
        || he_operation_ie[0] != 255
        || he_operation_ie[2] != HE_OPERATION_EXTENSION_ID
    {
        return None;
    }
    Some(he_operation_ie[3] & 0x07)
}

/// Recover the peer's HE Operation `ER-SU-Disable` bit.
///
/// Complete
/// `libnet80211.a[ieee80211_he.o]::ieee80211_parse_heopr`
/// logs complete-IE byte five bit zero as `ER-SU-Disable`, stores it at
/// peer-state bit 10 and passes it unchanged to `hal_he_set_ersu`.
pub fn he_extended_range_single_user_disabled(he_operation_ie: &[u8]) -> Option<bool> {
    if he_operation_ie.len() < 6
        || he_operation_ie[0] != 255
        || he_operation_ie[2] != HE_OPERATION_EXTENSION_ID
    {
        return None;
    }
    Some(he_operation_ie[5] & 0x01 != 0)
}

/// One bounded, owned observation from a beacon or probe response.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanRecord {
    pub ssid: [u8; 32],
    pub ssid_len: u8,
    /// Individual infrastructure BSS address, validated while parsing.
    pub bssid: Bssid,
    pub channel: u8,
    pub rssi: i8,
    pub privacy: bool,
    pub rsn: bool,
    /// Number of complete RSN elements observed in this bounded record.
    pub rsn_ie_count: u8,
    pub legacy_wpa: bool,
    pub information_elements_truncated: bool,
    pub capability_info: u16,
    /// The access point's TSF when it sent this beacon or probe response.
    pub timestamp: crate::tsf::TsfInstant,
    /// The receiver's monotonic time of this frame's reception, from its
    /// receive timestamp; a station advances `timestamp` by the time since
    /// to set its own TSF. `None` when the receiver had no reception time
    /// for the frame.
    pub received_at: Option<Instant>,
    pub beacon_interval_tu: u16,
    pub supported_rates: [u8; 8],
    pub supported_rates_len: u8,
    pub extended_supported_rates: [u8; EXTENDED_RATES_CAPACITY],
    pub extended_supported_rates_len: u8,
    /// One-byte ERP Information payload.  In particular, bit one is the
    /// infrastructure BSS Use Protection policy.
    pub erp_information: Option<u8>,
    pub ht_capability_ie: [u8; HT_CAPABILITY_IE_LEN],
    pub ht_capability_ie_present: bool,
    pub ht_operation_ie: [u8; HT_OPERATION_IE_LEN],
    pub ht_operation_ie_present: bool,
    pub he_capability_ie: [u8; HE_CAPABILITY_IE_CAPACITY],
    pub he_capability_ie_len: u8,
    pub he_operation_ie: [u8; HE_OPERATION_IE_CAPACITY],
    pub he_operation_ie_len: u8,
    pub wmm_ie: [u8; WMM_IE_CAPACITY],
    pub wmm_ie_len: u8,
    pub rsn_ie: [u8; RSN_IE_CAPACITY],
    pub rsn_ie_len: u8,
    pub rsnxe: [u8; RSNXE_CAPACITY],
    pub rsnxe_len: u8,
}

impl ScanRecord {
    /// An observation's identity and signal sample, before its information
    /// elements are decoded. Empty table slots use `None`, never this record.
    pub const fn new(bssid: Bssid, channel: u8, rssi: i8) -> Self {
        Self {
            ssid: [0; 32],
            ssid_len: 0,
            bssid,
            channel,
            rssi,
            privacy: false,
            rsn: false,
            rsn_ie_count: 0,
            legacy_wpa: false,
            information_elements_truncated: false,
            capability_info: 0,
            timestamp: crate::tsf::TsfInstant::from_micros(0),
            received_at: None,
            beacon_interval_tu: 0,
            supported_rates: [0; 8],
            supported_rates_len: 0,
            extended_supported_rates: [0; EXTENDED_RATES_CAPACITY],
            extended_supported_rates_len: 0,
            erp_information: None,
            ht_capability_ie: [0; HT_CAPABILITY_IE_LEN],
            ht_capability_ie_present: false,
            ht_operation_ie: [0; HT_OPERATION_IE_LEN],
            ht_operation_ie_present: false,
            he_capability_ie: [0; HE_CAPABILITY_IE_CAPACITY],
            he_capability_ie_len: 0,
            he_operation_ie: [0; HE_OPERATION_IE_CAPACITY],
            he_operation_ie_len: 0,
            wmm_ie: [0; WMM_IE_CAPACITY],
            wmm_ie_len: 0,
            rsn_ie: [0; RSN_IE_CAPACITY],
            rsn_ie_len: 0,
            rsnxe: [0; RSNXE_CAPACITY],
            rsnxe_len: 0,
        }
    }

    pub fn ssid_bytes(&self) -> &[u8] {
        &self.ssid[..usize::from(self.ssid_len)]
    }

    /// Whether this observation matches one exact requested security mode.
    ///
    /// Mixed WPA/WPA2 advertisements are intentionally rejected. Open means
    /// that the Privacy capability is clear and neither RSN nor legacy WPA
    /// was observed; WPA2 means a Privacy-marked, RSN-only BSS. Full WPA2
    /// suite validation remains at association encoding.
    pub const fn matches_security(&self, mode: LinkProtection) -> bool {
        match mode {
            LinkProtection::Open => {
                !self.information_elements_truncated
                    && !self.privacy
                    && !self.rsn
                    && self.rsn_ie_count == 0
                    && !self.legacy_wpa
                    && self.rsn_ie_len == 0
            }
            LinkProtection::Ccmp => {
                !self.information_elements_truncated
                    && self.privacy
                    && self.rsn
                    && self.rsn_ie_count == 1
                    && !self.legacy_wpa
                    && self.rsn_ie_len != 0
            }
        }
    }

    pub fn supported_rates_bytes(&self) -> &[u8] {
        &self.supported_rates[..usize::from(self.supported_rates_len)]
    }

    pub fn extended_supported_rates_bytes(&self) -> &[u8] {
        &self.extended_supported_rates[..usize::from(self.extended_supported_rates_len)]
    }

    pub const fn erp_information(&self) -> Option<u8> {
        self.erp_information
    }

    pub fn ht_capability_ie_bytes(&self) -> Option<&[u8; HT_CAPABILITY_IE_LEN]> {
        self.ht_capability_ie_present
            .then_some(&self.ht_capability_ie)
    }

    /// Parse the retained HT IE into peer receive capabilities.
    pub fn ht_peer_capabilities(&self) -> Option<HtPeerCapabilities> {
        ht_peer_capabilities(self.ht_capability_ie_bytes()?)
    }

    /// Whether this peer advertises the special 40-MHz HT Duplicate MCS32.
    pub fn supports_ht_duplicate_mcs32(&self) -> bool {
        self.ht_peer_capabilities()
            .is_some_and(HtPeerCapabilities::supports_ht_duplicate_mcs32)
    }

    pub fn ht_operation_ie_bytes(&self) -> Option<&[u8; HT_OPERATION_IE_LEN]> {
        self.ht_operation_ie_present
            .then_some(&self.ht_operation_ie)
    }

    /// Whether the AP advertises reception with a 400 ns guard interval on a
    /// 20 MHz HT channel.
    pub fn supports_ht_short_guard_interval_20mhz(&self) -> bool {
        self.ht_capability_ie_bytes().is_some_and(|capability| {
            u16::from_le_bytes([capability[2], capability[3]]) & (1 << 5) != 0
        })
    }

    /// Whether the AP advertises reception with a 400 ns guard interval on a
    /// 40 MHz HT channel.
    pub fn supports_ht_short_guard_interval_40mhz(&self) -> bool {
        self.ht_capability_ie_bytes().is_some_and(|capability| {
            u16::from_le_bytes([capability[2], capability[3]]) & (1 << 6) != 0
        })
    }

    /// Return the AP's usable 40-MHz secondary-channel geometry.
    ///
    /// The HT Capabilities Supported Channel Width bit must be present, the
    /// HT Operation primary channel must match this record, and the operation
    /// must permit any channel width. The secondary offset uses IEEE 802.11
    /// values one (above) and three (below).
    ///
    /// SOURCE(esp32s31): the complete IEs copied by this module from Beacon/Probe
    /// Response frames; the ESP32-S31 mapping from above/below to CBW 2/3 is
    /// independently recovered in
    /// `oer-esp32s31-pac/src/phy/frequency.rs::bss_tx_offset` from the
    /// complete rev0 ROM `phy_bb_bss_cbw40`.
    pub fn ht40_secondary_channel(&self) -> Option<HtSecondaryChannel> {
        let capability = self.ht_capability_ie_bytes()?;
        let operation = self.ht_operation_ie_bytes()?;
        let capability_info = u16::from_le_bytes([capability[2], capability[3]]);
        if capability_info & (1 << 1) == 0
            || operation[2] != self.channel
            || operation[3] & (1 << 2) == 0
        {
            return None;
        }
        match operation[3] & 0x03 {
            1 if self.channel <= 9 => Some(HtSecondaryChannel::Above),
            3 if self.channel >= 5 => Some(HtSecondaryChannel::Below),
            _ => None,
        }
    }

    pub fn he_capability_ie_bytes(&self) -> &[u8] {
        &self.he_capability_ie[..usize::from(self.he_capability_ie_len)]
    }

    pub fn he_operation_ie_bytes(&self) -> &[u8] {
        &self.he_operation_ie[..usize::from(self.he_operation_ie_len)]
    }

    pub fn wmm_ie_bytes(&self) -> &[u8] {
        &self.wmm_ie[..usize::from(self.wmm_ie_len)]
    }

    pub fn wmm_parameters(&self) -> Option<crate::extensions::wmm::WmmParameterSet> {
        crate::extensions::wmm::parse_wmm_parameter_element(self.wmm_ie_bytes())
    }

    pub fn rsn_ie_bytes(&self) -> &[u8] {
        &self.rsn_ie[..usize::from(self.rsn_ie_len)]
    }

    pub fn rsnxe_bytes(&self) -> &[u8] {
        &self.rsnxe[..usize::from(self.rsnxe_len)]
    }
}

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

        for (index, existing) in self.records[..self.length].iter_mut().enumerate() {
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
                if record.ssid_len == 0 && existing.ssid_len != 0 {
                    record.ssid = existing.ssid;
                    record.ssid_len = existing.ssid_len;
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

        if self.length == N {
            self.dropped_unique_bss = self.dropped_unique_bss.saturating_add(1);
            return ScanObservation::Dropped;
        }
        let index = self.length;
        self.records[index] = Some(record);
        self.length += 1;
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
                && crate::station::select_association_rsn(record, policy).is_ok()
        })
        .max_by_key(|record| record.rssi)
}

/// Decode one beacon or probe response into an owned bounded record.
///
/// The fixed management and beacon fields are checked as one array before
/// interpreting the variable information elements.
pub fn parse_management(frame: &[u8], fallback_channel: u8, rssi: i8) -> Option<ScanRecord> {
    let (header, elements) = frame.split_first_chunk::<36>()?;
    let frame_control = u16::from_le_bytes([header[0], header[1]]);
    let subtype = (frame_control >> 4) & 0x0f;
    if frame_control & 0x000c != 0 || !matches!(subtype, 5 | 8) {
        return None;
    }

    let bssid = Bssid::new(IndividualAddress::new(header[16..22].try_into().ok()?)?)?;
    let mut record = ScanRecord::new(bssid, fallback_channel, rssi);
    let [.., t0, t1, t2, t3, t4, t5, t6, t7, b0, b1, c0, c1] = *header;
    record.timestamp =
        crate::tsf::TsfInstant::from_micros(u64::from_le_bytes([t0, t1, t2, t3, t4, t5, t6, t7]));
    record.beacon_interval_tu = u16::from_le_bytes([b0, b1]);
    record.capability_info = u16::from_le_bytes([c0, c1]);
    record.privacy = record.capability_info & 0x0010 != 0;

    let mut offset = 0;
    while offset + 2 <= elements.len() {
        let id = elements[offset];
        let length = usize::from(elements[offset + 1]);
        offset += 2;
        let Some(end) = offset.checked_add(length) else {
            record.information_elements_truncated = true;
            break;
        };
        if end > elements.len() {
            record.information_elements_truncated = true;
            break;
        }
        let value = &elements[offset..end];
        match id {
            0 if length <= record.ssid.len() => {
                record.ssid[..length].copy_from_slice(value);
                record.ssid_len = length as u8;
            }
            1 if length <= record.supported_rates.len() => {
                record.supported_rates[..length].copy_from_slice(value);
                record.supported_rates_len = length as u8;
            }
            3 if length == 1 => record.channel = value[0],
            42 if length == 1 => {
                // Multiple ERP elements are malformed, but protection must
                // fail closed while the broader scan record remains usable:
                // retain every observed Use Protection assertion.
                record.erp_information = Some(record.erp_information.unwrap_or(0) | value[0]);
            }
            45 if length + 2 == HT_CAPABILITY_IE_LEN => {
                record
                    .ht_capability_ie
                    .copy_from_slice(&elements[offset - 2..end]);
                record.ht_capability_ie_present = true;
            }
            48 => {
                record.rsn = true;
                record.rsn_ie_count = record.rsn_ie_count.saturating_add(1);
                let total = length + 2;
                if total <= record.rsn_ie.len() {
                    record.rsn_ie[..total].copy_from_slice(&elements[offset - 2..end]);
                    record.rsn_ie_len = total as u8;
                } else {
                    record.information_elements_truncated = true;
                }
            }
            50 => {
                let copied = length.min(record.extended_supported_rates.len());
                record.extended_supported_rates[..copied].copy_from_slice(&value[..copied]);
                record.extended_supported_rates_len = copied as u8;
                record.information_elements_truncated |= copied != length;
            }
            61 if length + 2 == HT_OPERATION_IE_LEN => {
                record
                    .ht_operation_ie
                    .copy_from_slice(&elements[offset - 2..end]);
                record.ht_operation_ie_present = true;
            }
            244 => {
                let total = length + 2;
                if total <= record.rsnxe.len() {
                    record.rsnxe[..total].copy_from_slice(&elements[offset - 2..end]);
                    record.rsnxe_len = total as u8;
                } else {
                    record.information_elements_truncated = true;
                }
            }
            255 if value.first().copied() == Some(HE_CAPABILITIES_EXTENSION_ID) => {
                let total = length + 2;
                if total <= record.he_capability_ie.len() {
                    record.he_capability_ie[..total].copy_from_slice(&elements[offset - 2..end]);
                    record.he_capability_ie_len = total as u8;
                } else {
                    record.information_elements_truncated = true;
                }
            }
            255 if value.first().copied() == Some(HE_OPERATION_EXTENSION_ID) => {
                let total = length + 2;
                if total <= record.he_operation_ie.len() {
                    record.he_operation_ie[..total].copy_from_slice(&elements[offset - 2..end]);
                    record.he_operation_ie_len = total as u8;
                } else {
                    record.information_elements_truncated = true;
                }
            }
            221 if length >= 4 && value[..4] == [0x00, 0x50, 0xf2, 0x01] => {
                record.legacy_wpa = true;
            }
            221 if length >= 6 && value[..4] == [0x00, 0x50, 0xf2, 0x02] => {
                let total = length + 2;
                if total <= record.wmm_ie.len() {
                    record.wmm_ie[..total].copy_from_slice(&elements[offset - 2..end]);
                    record.wmm_ie_len = total as u8;
                } else {
                    record.information_elements_truncated = true;
                }
            }
            _ => {}
        }
        offset = end;
    }
    // One trailing byte cannot form an IE header. Treat it as malformed,
    // rather than silently accepting it as absence of a later RSN/WPA IE.
    record.information_elements_truncated |= offset != elements.len();
    Some(record)
}

#[cfg(test)]
mod tests;
