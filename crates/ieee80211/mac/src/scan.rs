//! Allocation-free scan records and management-frame parsing.
//!
//! This is the maintained source-owned passive-scan frontier. Channel changes,
//! dwell timers, and RX ownership remain explicit responsibilities of the
//! radio owner; this module owns only bounded observations and selection. The
//! implementation is chip-independent and forms the maintained upper-stack
//! extraction.
//!
//! Received bytes reach this module; nothing here panics on them.
#![deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]

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
    match *he_operation_ie {
        [255, _, HE_OPERATION_EXTENSION_ID, parameters, ..] => Some(parameters & 0x07),
        _ => None,
    }
}

/// Recover the peer's HE Operation `ER-SU-Disable` bit.
///
/// Complete
/// `libnet80211.a[ieee80211_he.o]::ieee80211_parse_heopr`
/// logs complete-IE byte five bit zero as `ER-SU-Disable`, stores it at
/// peer-state bit 10 and passes it unchanged to `hal_he_set_ersu`.
pub fn he_extended_range_single_user_disabled(he_operation_ie: &[u8]) -> Option<bool> {
    match *he_operation_ie {
        [255, _, HE_OPERATION_EXTENSION_ID, _, _, parameters, ..] => Some(parameters & 0x01 != 0),
        _ => None,
    }
}

/// A value offered to a [`ScanRecord`] setter that its buffer cannot hold.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanRecordError {
    /// The value has `length` bytes; the record holds at most `capacity`.
    TooLong { length: usize, capacity: usize },
}

/// One bounded, owned observation from a beacon or probe response.
///
/// Each element buffer and the length that names its bytes are private:
/// only [`parse_management`] and the checked setters write them, so a stored
/// length never exceeds its buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanRecord {
    ssid: [u8; 32],
    ssid_len: u8,
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
    supported_rates: [u8; 8],
    supported_rates_len: u8,
    extended_supported_rates: [u8; EXTENDED_RATES_CAPACITY],
    extended_supported_rates_len: u8,
    /// One-byte ERP Information payload.  In particular, bit one is the
    /// infrastructure BSS Use Protection policy.
    pub erp_information: Option<u8>,
    ht_capability_ie: Option<[u8; HT_CAPABILITY_IE_LEN]>,
    ht_operation_ie: Option<[u8; HT_OPERATION_IE_LEN]>,
    he_capability_ie: [u8; HE_CAPABILITY_IE_CAPACITY],
    he_capability_ie_len: u8,
    he_operation_ie: [u8; HE_OPERATION_IE_CAPACITY],
    he_operation_ie_len: u8,
    wmm_ie: [u8; WMM_IE_CAPACITY],
    wmm_ie_len: u8,
    rsn_ie: [u8; RSN_IE_CAPACITY],
    rsn_ie_len: u8,
    rsnxe: [u8; RSNXE_CAPACITY],
    rsnxe_len: u8,
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
            ht_capability_ie: None,
            ht_operation_ie: None,
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
        front(&self.ssid, self.ssid_len)
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
        front(&self.supported_rates, self.supported_rates_len)
    }

    pub fn extended_supported_rates_bytes(&self) -> &[u8] {
        front(
            &self.extended_supported_rates,
            self.extended_supported_rates_len,
        )
    }

    pub const fn erp_information(&self) -> Option<u8> {
        self.erp_information
    }

    pub fn ht_capability_ie_bytes(&self) -> Option<&[u8; HT_CAPABILITY_IE_LEN]> {
        self.ht_capability_ie.as_ref()
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
        self.ht_operation_ie.as_ref()
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
        front(&self.he_capability_ie, self.he_capability_ie_len)
    }

    pub fn he_operation_ie_bytes(&self) -> &[u8] {
        front(&self.he_operation_ie, self.he_operation_ie_len)
    }

    pub fn wmm_ie_bytes(&self) -> &[u8] {
        front(&self.wmm_ie, self.wmm_ie_len)
    }

    pub fn wmm_parameters(&self) -> Option<crate::extensions::wmm::WmmParameterSet> {
        crate::extensions::wmm::parse_wmm_parameter_element(self.wmm_ie_bytes())
    }

    pub fn rsn_ie_bytes(&self) -> &[u8] {
        front(&self.rsn_ie, self.rsn_ie_len)
    }

    pub fn rsnxe_bytes(&self) -> &[u8] {
        front(&self.rsnxe, self.rsnxe_len)
    }

    /// Store the SSID `ssid`; an empty one is a hidden SSID.
    pub fn set_ssid(&mut self, ssid: &[u8]) -> Result<(), ScanRecordError> {
        replace(&mut self.ssid, &mut self.ssid_len, ssid)
    }

    /// Store the Supported Rates element's value.
    pub fn set_supported_rates(&mut self, rates: &[u8]) -> Result<(), ScanRecordError> {
        replace(
            &mut self.supported_rates,
            &mut self.supported_rates_len,
            rates,
        )
    }

    /// Store the Extended Supported Rates element's value.
    pub fn set_extended_supported_rates(&mut self, rates: &[u8]) -> Result<(), ScanRecordError> {
        replace(
            &mut self.extended_supported_rates,
            &mut self.extended_supported_rates_len,
            rates,
        )
    }

    /// Store the complete HT Capabilities element, or `None` for its absence.
    pub const fn set_ht_capability_ie(&mut self, element: Option<[u8; HT_CAPABILITY_IE_LEN]>) {
        self.ht_capability_ie = element;
    }

    /// Store the complete HT Operation element, or `None` for its absence.
    pub const fn set_ht_operation_ie(&mut self, element: Option<[u8; HT_OPERATION_IE_LEN]>) {
        self.ht_operation_ie = element;
    }

    /// Store the complete HE Capabilities element; empty for its absence.
    pub fn set_he_capability_ie(&mut self, element: &[u8]) -> Result<(), ScanRecordError> {
        replace(
            &mut self.he_capability_ie,
            &mut self.he_capability_ie_len,
            element,
        )
    }

    /// Store the complete HE Operation element; empty for its absence.
    pub fn set_he_operation_ie(&mut self, element: &[u8]) -> Result<(), ScanRecordError> {
        replace(
            &mut self.he_operation_ie,
            &mut self.he_operation_ie_len,
            element,
        )
    }

    /// Store the complete WMM element; empty for its absence.
    pub fn set_wmm_ie(&mut self, element: &[u8]) -> Result<(), ScanRecordError> {
        replace(&mut self.wmm_ie, &mut self.wmm_ie_len, element)
    }

    /// Store the complete RSN element; empty for its absence.
    pub fn set_rsn_ie(&mut self, element: &[u8]) -> Result<(), ScanRecordError> {
        replace(&mut self.rsn_ie, &mut self.rsn_ie_len, element)
    }

    /// Store the complete RSN Extension element; empty for its absence.
    pub fn set_rsnxe(&mut self, element: &[u8]) -> Result<(), ScanRecordError> {
        replace(&mut self.rsnxe, &mut self.rsnxe_len, element)
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
    let (header, mut elements) = frame.split_first_chunk::<36>()?;
    let [
        fc0,
        fc1,
        _duration0,
        _duration1,
        // Receiver and transmitter addresses.
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        _,
        b0,
        b1,
        b2,
        b3,
        b4,
        b5,
        _sequence0,
        _sequence1,
        t0,
        t1,
        t2,
        t3,
        t4,
        t5,
        t6,
        t7,
        i0,
        i1,
        c0,
        c1,
    ] = *header;
    let frame_control = u16::from_le_bytes([fc0, fc1]);
    let subtype = (frame_control >> 4) & 0x0f;
    if frame_control & 0x000c != 0 || !matches!(subtype, 5 | 8) {
        return None;
    }

    let bssid = Bssid::new(IndividualAddress::new([b0, b1, b2, b3, b4, b5])?)?;
    let mut record = ScanRecord::new(bssid, fallback_channel, rssi);
    record.timestamp =
        crate::tsf::TsfInstant::from_micros(u64::from_le_bytes([t0, t1, t2, t3, t4, t5, t6, t7]));
    record.beacon_interval_tu = u16::from_le_bytes([i0, i1]);
    record.capability_info = u16::from_le_bytes([c0, c1]);
    record.privacy = record.capability_info & 0x0010 != 0;

    while let Some((&element_header, rest)) = elements.split_first_chunk::<2>() {
        let [id, length] = element_header;
        let Some((value, rest)) = rest.split_at_checked(usize::from(length)) else {
            record.information_elements_truncated = true;
            break;
        };
        elements = rest;
        match (id, value) {
            (0, _) => {
                if let Some(length) = store_value(&mut record.ssid, value) {
                    record.ssid_len = length;
                }
            }
            (1, _) => {
                if let Some(length) = store_value(&mut record.supported_rates, value) {
                    record.supported_rates_len = length;
                }
            }
            (3, &[channel]) => record.channel = channel,
            (42, &[information]) => {
                // Multiple ERP elements are malformed, but protection must
                // fail closed while the broader scan record remains usable:
                // retain every observed Use Protection assertion.
                record.erp_information = Some(record.erp_information.unwrap_or(0) | information);
            }
            (45, _) => {
                if let Some(element) = exact_element(element_header, value) {
                    record.ht_capability_ie = Some(element);
                }
            }
            (48, _) => {
                record.rsn = true;
                record.rsn_ie_count = record.rsn_ie_count.saturating_add(1);
                match store_element(&mut record.rsn_ie, element_header, value) {
                    Some(length) => record.rsn_ie_len = length,
                    None => record.information_elements_truncated = true,
                }
            }
            (50, _) => {
                let copied = record
                    .extended_supported_rates
                    .iter_mut()
                    .zip(value)
                    .map(|(stored, rate)| *stored = *rate)
                    .count();
                // `copied` is at most EXTENDED_RATES_CAPACITY, which fits a u8.
                record.extended_supported_rates_len = copied as u8;
                record.information_elements_truncated |= copied != value.len();
            }
            (61, _) => {
                if let Some(element) = exact_element(element_header, value) {
                    record.ht_operation_ie = Some(element);
                }
            }
            (244, _) => match store_element(&mut record.rsnxe, element_header, value) {
                Some(length) => record.rsnxe_len = length,
                None => record.information_elements_truncated = true,
            },
            (255, [HE_CAPABILITIES_EXTENSION_ID, ..]) => {
                match store_element(&mut record.he_capability_ie, element_header, value) {
                    Some(length) => record.he_capability_ie_len = length,
                    None => record.information_elements_truncated = true,
                }
            }
            (255, [HE_OPERATION_EXTENSION_ID, ..]) => {
                match store_element(&mut record.he_operation_ie, element_header, value) {
                    Some(length) => record.he_operation_ie_len = length,
                    None => record.information_elements_truncated = true,
                }
            }
            (221, [0x00, 0x50, 0xf2, 0x01, ..]) => record.legacy_wpa = true,
            (221, [0x00, 0x50, 0xf2, 0x02, _, _, ..]) => {
                match store_element(&mut record.wmm_ie, element_header, value) {
                    Some(length) => record.wmm_ie_len = length,
                    None => record.information_elements_truncated = true,
                }
            }
            _ => {}
        }
    }
    // One trailing byte cannot form an IE header. Treat it as malformed,
    // rather than silently accepting it as absence of a later RSN/WPA IE.
    record.information_elements_truncated |= !elements.is_empty();
    Some(record)
}

/// The first `length` bytes of a record buffer: the bytes it holds.
#[expect(
    clippy::indexing_slicing,
    reason = "only the parser and the checked setters set the length, never past the buffer"
)]
fn front<const N: usize>(buffer: &[u8; N], length: u8) -> &[u8] {
    &buffer.as_slice()[..usize::from(length)]
}

/// Replace the bytes a record buffer holds by `value` and zero the rest, or
/// refuse a value longer than the buffer and change nothing.
fn replace<const N: usize>(
    buffer: &mut [u8; N],
    length: &mut u8,
    value: &[u8],
) -> Result<(), ScanRecordError> {
    let too_long = ScanRecordError::TooLong {
        length: value.len(),
        capacity: N,
    };
    // Every record buffer is shorter than 256 bytes, so a value that fits
    // its buffer has a `u8` length.
    let (front, rest) = buffer.split_at_mut_checked(value.len()).ok_or(too_long)?;
    let value_length = u8::try_from(value.len()).map_err(|_| too_long)?;
    front.copy_from_slice(value);
    rest.fill(0);
    *length = value_length;
    Ok(())
}

/// Copy an element's value to the front of `buffer`: its length, or `None`
/// when it does not fit and `buffer` is unchanged.
fn store_value<const N: usize>(buffer: &mut [u8; N], value: &[u8]) -> Option<u8> {
    let length = u8::try_from(value.len()).ok()?;
    let (front, _) = buffer.split_at_mut_checked(value.len())?;
    front.copy_from_slice(value);
    Some(length)
}

/// Copy a complete element, header and value, to the front of `buffer`: its
/// length, or `None` when it does not fit and `buffer` is unchanged.
fn store_element<const N: usize>(
    buffer: &mut [u8; N],
    header: [u8; 2],
    value: &[u8],
) -> Option<u8> {
    let length = u8::try_from(value.len().checked_add(2)?).ok()?;
    let (stored_header, rest) = buffer.split_first_chunk_mut::<2>()?;
    let (stored_value, _) = rest.split_at_mut_checked(value.len())?;
    *stored_header = header;
    stored_value.copy_from_slice(value);
    Some(length)
}

/// A complete element of exactly `N` bytes, or `None` for another length.
fn exact_element<const N: usize>(header: [u8; 2], value: &[u8]) -> Option<[u8; N]> {
    let mut element = [0; N];
    let (stored_header, stored_value) = element.split_first_chunk_mut::<2>()?;
    if stored_value.len() != value.len() {
        return None;
    }
    *stored_header = header;
    stored_value.copy_from_slice(value);
    Some(element)
}

#[cfg(test)]
mod tests;
