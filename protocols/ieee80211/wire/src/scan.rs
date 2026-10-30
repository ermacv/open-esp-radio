//! Allocation-free scan records and management-frame parsing.
//!
//! A record describes one decoded beacon or probe response. Observation
//! retention and candidate selection belong to the station core; this module
//! only parses and stores bounded fields from one management frame.
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
use crate::ht::{
    HT_CAPABILITY_IE_LEN, HT_OPERATION_IE_LEN, HtPeerCapabilities, ht_peer_capabilities,
};
use crate::protection::{ErpElements, ErpElementsError};

pub const RSN_IE_CAPACITY: usize = 64;
pub const RSNXE_CAPACITY: usize = 16;
pub const EXTENDED_RATES_CAPACITY: usize = 16;
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
        [255, _, HE_OPERATION_EXTENSION_ID, parameters, ..] => {
            Some(crate::he::HeOperationFields::from_bytes([parameters, 0, 0, 0, 0, 0]).default_pe())
        }
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
        [255, _, HE_OPERATION_EXTENSION_ID, _, _, parameters, ..] => Some(
            crate::he::HeOperationFields::from_bytes([0, 0, parameters, 0, 0, 0]).er_su_disabled(),
        ),
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
    ssid_len: oer_wire::PrefixLength<32>,
    /// Individual infrastructure BSS address, validated while parsing.
    pub bssid: Bssid,
    /// The DS Parameter Set's Current Channel; `None` when the frame carries
    /// no such element. Which channel the BSS is on is the receiver's
    /// decision.
    pub ds_channel: Option<u8>,
    pub privacy: bool,
    pub rsn: bool,
    /// Number of complete RSN elements observed in this bounded record.
    pub rsn_ie_count: u8,
    pub legacy_wpa: bool,
    pub information_elements_truncated: bool,
    pub capability_info: u16,
    /// The access point's TSF when it sent this beacon or probe response.
    pub timestamp: crate::tsf::TsfTimestamp,
    pub beacon_interval_tu: u16,
    supported_rates: [u8; 8],
    supported_rates_len: oer_wire::PrefixLength<8>,
    extended_supported_rates: [u8; EXTENDED_RATES_CAPACITY],
    extended_supported_rates_len: oer_wire::PrefixLength<EXTENDED_RATES_CAPACITY>,
    ht_capability_ie: Option<[u8; HT_CAPABILITY_IE_LEN]>,
    ht_operation_ie: Option<[u8; HT_OPERATION_IE_LEN]>,
    he_capability_ie: [u8; HE_CAPABILITY_IE_CAPACITY],
    he_capability_ie_len: oer_wire::PrefixLength<HE_CAPABILITY_IE_CAPACITY>,
    he_operation_ie: [u8; HE_OPERATION_IE_CAPACITY],
    he_operation_ie_len: oer_wire::PrefixLength<HE_OPERATION_IE_CAPACITY>,
    wmm_ie: [u8; WMM_IE_CAPACITY],
    wmm_ie_len: oer_wire::PrefixLength<WMM_IE_CAPACITY>,
    rsn_ie: [u8; RSN_IE_CAPACITY],
    rsn_ie_len: oer_wire::PrefixLength<RSN_IE_CAPACITY>,
    rsnxe: [u8; RSNXE_CAPACITY],
    rsnxe_len: oer_wire::PrefixLength<RSNXE_CAPACITY>,
}

impl ScanRecord {
    /// A record's identity, before its information elements are decoded.
    /// Empty table slots use `None`, never this record.
    pub const fn new(bssid: Bssid) -> Self {
        Self {
            ssid: [0; 32],
            ssid_len: oer_wire::PrefixLength::ZERO,
            bssid,
            ds_channel: None,
            privacy: false,
            rsn: false,
            rsn_ie_count: 0,
            legacy_wpa: false,
            information_elements_truncated: false,
            capability_info: 0,
            timestamp: crate::tsf::TsfTimestamp::from_micros(0),
            beacon_interval_tu: 0,
            supported_rates: [0; 8],
            supported_rates_len: oer_wire::PrefixLength::ZERO,
            extended_supported_rates: [0; EXTENDED_RATES_CAPACITY],
            extended_supported_rates_len: oer_wire::PrefixLength::ZERO,
            ht_capability_ie: None,
            ht_operation_ie: None,
            he_capability_ie: [0; HE_CAPABILITY_IE_CAPACITY],
            he_capability_ie_len: oer_wire::PrefixLength::ZERO,
            he_operation_ie: [0; HE_OPERATION_IE_CAPACITY],
            he_operation_ie_len: oer_wire::PrefixLength::ZERO,
            wmm_ie: [0; WMM_IE_CAPACITY],
            wmm_ie_len: oer_wire::PrefixLength::ZERO,
            rsn_ie: [0; RSN_IE_CAPACITY],
            rsn_ie_len: oer_wire::PrefixLength::ZERO,
            rsnxe: [0; RSNXE_CAPACITY],
            rsnxe_len: oer_wire::PrefixLength::ZERO,
        }
    }

    /// Copy the bounded SSID value of another decoded observation.
    pub fn copy_ssid_from(&mut self, other: &Self) {
        self.ssid = other.ssid;
        self.ssid_len = other.ssid_len;
    }

    pub fn ssid_bytes(&self) -> &[u8] {
        front(&self.ssid, self.ssid_len)
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
            crate::ht::HtCapabilityInfo::from_bytes([capability[2], capability[3]]).short_gi20()
        })
    }

    /// Whether the AP advertises reception with a 400 ns guard interval on a
    /// 40 MHz HT channel.
    pub fn supports_ht_short_guard_interval_40mhz(&self) -> bool {
        self.ht_capability_ie_bytes().is_some_and(|capability| {
            crate::ht::HtCapabilityInfo::from_bytes([capability[2], capability[3]]).short_gi40()
        })
    }

    /// Return the AP's usable 40-MHz secondary-channel geometry.
    ///
    /// The HT Capabilities Supported Channel Width bit must be present, the
    /// HT Operation primary channel must be `primary_channel`, the channel the
    /// receiver decided the BSS is on, and the operation
    /// must permit any channel width. The secondary offset uses IEEE 802.11
    /// values one (above) and three (below).
    ///
    /// SOURCE(esp32s31): the complete IEs copied by this module from Beacon/Probe
    /// Response frames; the ESP32-S31 mapping from above/below to CBW 2/3 is
    /// independently recovered in
    /// `oer-esp32s31-pac/src/phy/frequency.rs::bss_tx_offset` from the
    /// complete rev0 ROM `phy_bb_bss_cbw40`.
    pub fn ht40_secondary_channel(&self, primary_channel: u8) -> Option<HtSecondaryChannel> {
        let capability = self.ht_capability_ie_bytes()?;
        let operation = self.ht_operation_ie_bytes()?;
        let capability_info = u16::from_le_bytes([capability[2], capability[3]]);
        if !crate::ht::HtCapabilityInfo::from_bytes(capability_info.to_le_bytes()).width40()
            || operation[2] != primary_channel
            || !crate::ht::HtOperationInfo::from_bytes([operation[3]]).width40()
        {
            return None;
        }
        match crate::ht::HtOperationInfo::from_bytes([operation[3]]).secondary() {
            1 if primary_channel <= 9 => Some(HtSecondaryChannel::Above),
            3 if primary_channel >= 5 => Some(HtSecondaryChannel::Below),
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

/// Decode one beacon or probe response into an owned bounded record.
pub fn parse_management(frame: &[u8]) -> Option<ScanRecord> {
    ScanFrame::parse(frame).map(ScanFrame::into_record)
}

/// A decoded beacon or probe response: its bounded record, and its ERP
/// elements checked when the view is made. A damaged ERP element is reported
/// here and leaves the record valid: the receiver decides what it requires
/// without one.
#[derive(Clone, Copy, Debug)]
pub struct ScanFrame<'a> {
    record: ScanRecord,
    erp: Result<ErpElements<'a>, ErpElementsError>,
}

impl<'a> ScanFrame<'a> {
    /// Decode `frame`; `None` for another frame or a malformed header.
    pub fn parse(frame: &'a [u8]) -> Option<Self> {
        let (record, elements) = decode_management(frame)?;
        Some(Self {
            record,
            erp: ErpElements::parse(elements),
        })
    }

    pub const fn record(&self) -> &ScanRecord {
        &self.record
    }

    pub const fn into_record(self) -> ScanRecord {
        self.record
    }

    /// The frame's ERP elements as checked by [`Self::parse`]: a damaged
    /// element or an ERP element of another length is its typed error.
    pub const fn erp_elements(&self) -> Result<ErpElements<'a>, ErpElementsError> {
        self.erp
    }
}

/// The record of a beacon or probe response and its information elements.
///
/// The fixed fields start where the decoded MAC header ends, so a frame
/// with the Order bit set carries its HT Control field before them. They
/// are checked as one array before interpreting the variable information
/// elements.
fn decode_management(frame: &[u8]) -> Option<(ScanRecord, &[u8])> {
    let header = crate::frame::DecodedHeader::decode(frame)?;
    let control = header.control();
    if control.is_data() || !matches!(control.subtype(), 5 | 8) {
        return None;
    }
    let mut reader = oer_wire::Reader::new(header.body());
    let fixed = reader.array::<12>().ok()?;
    let element_bytes = reader.remaining();
    let mut elements = oer_wire::Reader::new(element_bytes);
    let [t0, t1, t2, t3, t4, t5, t6, t7, i0, i1, c0, c1] = *fixed;
    let [b0, b1, b2, b3, b4, b5] = header.address3();

    let bssid = Bssid::new(IndividualAddress::new([b0, b1, b2, b3, b4, b5])?)?;
    let mut record = ScanRecord::new(bssid);
    record.timestamp = crate::tsf::TsfTimestamp::from_bytes([t0, t1, t2, t3, t4, t5, t6, t7]);
    record.beacon_interval_tu = u16::from_le_bytes([i0, i1]);
    record.capability_info = u16::from_le_bytes([c0, c1]);
    record.privacy =
        crate::management::CapabilityInformation::from_bytes(record.capability_info.to_le_bytes())
            .privacy();

    while let Ok(&element_header) = elements.array::<2>() {
        let [id, length] = element_header;
        let Ok(value) = elements.take(usize::from(length)) else {
            record.information_elements_truncated = true;
            break;
        };
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
            (3, &[channel]) => record.ds_channel = Some(channel),
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
                record.extended_supported_rates_len = oer_wire::PrefixLength::new(copied)?;
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
    Some((record, element_bytes))
}

/// The first `length` bytes of a record buffer: the bytes it holds.
fn front<const N: usize>(buffer: &[u8; N], length: oer_wire::PrefixLength<N>) -> &[u8] {
    length.borrow(buffer)
}

/// Replace the bytes a record buffer holds by `value` and zero the rest, or
/// refuse a value longer than the buffer and change nothing.
fn replace<const N: usize>(
    buffer: &mut [u8; N],
    length: &mut oer_wire::PrefixLength<N>,
    value: &[u8],
) -> Result<(), ScanRecordError> {
    let too_long = ScanRecordError::TooLong {
        length: value.len(),
        capacity: N,
    };
    let value_length = oer_wire::PrefixLength::new(value.len()).ok_or(too_long)?;
    buffer.fill(0);
    oer_wire::Writer::new(value_length.borrow_mut(buffer))
        .write(value)
        .map_err(|_| too_long)?;
    *length = value_length;
    Ok(())
}

/// Copy an element's value to the front of `buffer`: its length, or `None`
/// when it does not fit and `buffer` is unchanged.
fn store_value<const N: usize>(
    buffer: &mut [u8; N],
    value: &[u8],
) -> Option<oer_wire::PrefixLength<N>> {
    let length = oer_wire::PrefixLength::new(value.len())?;
    oer_wire::Writer::new(length.borrow_mut(buffer))
        .write(value)
        .ok()?;
    Some(length)
}

/// Copy a complete element, header and value, to the front of `buffer`: its
/// length, or `None` when it does not fit and `buffer` is unchanged.
fn store_element<const N: usize>(
    buffer: &mut [u8; N],
    header: [u8; 2],
    value: &[u8],
) -> Option<oer_wire::PrefixLength<N>> {
    let length = oer_wire::PrefixLength::new(value.len().checked_add(2)?)?;
    let (stored_header, stored_value) = length.borrow_mut(buffer).split_first_chunk_mut::<2>()?;
    *stored_header = header;
    oer_wire::Writer::new(stored_value).write(value).ok()?;
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
    oer_wire::Writer::new(stored_value).write(value).ok()?;
    Some(element)
}
