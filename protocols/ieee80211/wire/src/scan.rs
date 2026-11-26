//! Allocation-free scan records and management-frame parsing.
//!
//! A record describes one decoded beacon or probe response. Observation
//! retention and candidate selection belong to the station core; this module
//! only parses and stores bounded fields from one management frame.
//!
//! Received bytes reach this module; nothing here panics on them.

use crate::address::Bssid;
use crate::element::{ElementId, ElementIdExtension};
use crate::ht::{
    HT_CAPABILITY_IE_LEN, HT_OPERATION_IE_LEN, HtPeerCapabilities, ht_peer_capabilities,
};
use crate::protection::{ErpElements, ErpElementsError};
use oer_protocols_ieee802_wire::IndividualAddress;

pub const RSN_IE_CAPACITY: usize = 64;
pub const RSNXE_CAPACITY: usize = 16;
pub const EXTENDED_RATES_CAPACITY: usize = 16;
pub const HE_CAPABILITY_IE_CAPACITY: usize = 64;
pub const HE_OPERATION_IE_CAPACITY: usize = 32;
pub const WMM_IE_CAPACITY: usize = 26;

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
pub fn he_default_packet_extension_duration(
    he_operation_ie: &[u8],
) -> Option<crate::he::DefaultPeDuration> {
    match *he_operation_ie {
        [_, _, _, parameters, ..] if named_he_operation(he_operation_ie) => {
            Some(crate::he::HeOperationFields::from_bytes([parameters, 0, 0, 0, 0, 0]).default_pe())
        }
        _ => None,
    }
}

fn named_he_operation(element: &[u8]) -> bool {
    ElementIdExtension::read(element) == Some(ElementIdExtension::HE_OPERATION)
}

/// Recover the peer's HE Operation `ER-SU-Disable` bit.
///
/// Complete
/// `libnet80211.a[ieee80211_he.o]::ieee80211_parse_heopr`
/// logs complete-IE byte five bit zero as `ER-SU-Disable`, stores it at
/// peer-state bit 10 and passes it unchanged to `hal_he_set_ersu`.
pub fn he_extended_range_single_user_disabled(he_operation_ie: &[u8]) -> Option<bool> {
    match *he_operation_ie {
        [_, _, _, _, _, parameters, ..] if named_he_operation(he_operation_ie) => Some(
            crate::he::HeOperationFields::from_bytes([0, 0, parameters, 0, 0, 0]).er_su_disabled(),
        ),
        _ => None,
    }
}

/// Whether a frame's information elements were read to their end, or one
/// was cut short by the frame or by the buffer that keeps it. Absence of an
/// element after a cut is no proof that the frame lacks it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElementList {
    Complete,
    Truncated,
}

/// Whether a frame carries an information element.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElementPresence {
    Absent,
    Present,
}

/// How often a frame carries an element it may carry once: not at all,
/// once with its record (header and value), or more than once.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ElementOccurrence<'a> {
    Absent,
    Once(&'a [u8]),
    Repeated,
}

impl<'a> ElementOccurrence<'a> {
    /// The occurrence after one more copy, `record`.
    #[must_use]
    pub const fn and(self, record: &'a [u8]) -> Self {
        match self {
            Self::Absent => Self::Once(record),
            Self::Once(_) | Self::Repeated => Self::Repeated,
        }
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
    ssid_len: oer_portable_wire::PrefixLength<32>,
    /// Individual infrastructure BSS address, validated while parsing.
    pub bssid: Bssid,
    /// The DS Parameter Set's Current Channel; `None` when the frame carries
    /// no such element. Which channel the BSS is on is the receiver's
    /// decision.
    pub ds_channel: Option<u8>,
    /// Number of RSN elements observed in this bounded record.
    pub rsn_ie_count: u8,
    /// A legacy WPA vendor element (OUI 00:50:f2, type 1).
    pub legacy_wpa: ElementPresence,
    pub information_elements: ElementList,
    pub capability_info: crate::management::CapabilityInformation,
    /// The access point's TSF when it sent this beacon or probe response.
    pub timestamp: crate::tsf::TsfInstant,
    /// The Beacon Interval field (IEEE Std 802.11-2020 9.4.1.3).
    pub beacon_interval: crate::tsf::TimeUnits,
    supported_rates: [u8; 8],
    supported_rates_len: oer_portable_wire::PrefixLength<8>,
    extended_supported_rates: [u8; EXTENDED_RATES_CAPACITY],
    extended_supported_rates_len: oer_portable_wire::PrefixLength<EXTENDED_RATES_CAPACITY>,
    ht_capability_ie: Option<[u8; HT_CAPABILITY_IE_LEN]>,
    ht_operation_ie: Option<[u8; HT_OPERATION_IE_LEN]>,
    he_capability_ie: [u8; HE_CAPABILITY_IE_CAPACITY],
    he_capability_ie_len: oer_portable_wire::PrefixLength<HE_CAPABILITY_IE_CAPACITY>,
    he_operation_ie: [u8; HE_OPERATION_IE_CAPACITY],
    he_operation_ie_len: oer_portable_wire::PrefixLength<HE_OPERATION_IE_CAPACITY>,
    wmm_ie: [u8; WMM_IE_CAPACITY],
    wmm_ie_len: oer_portable_wire::PrefixLength<WMM_IE_CAPACITY>,
    rsn_ie: [u8; RSN_IE_CAPACITY],
    rsn_ie_len: oer_portable_wire::PrefixLength<RSN_IE_CAPACITY>,
    rsnxe: [u8; RSNXE_CAPACITY],
    rsnxe_len: oer_portable_wire::PrefixLength<RSNXE_CAPACITY>,
}

impl ScanRecord {
    /// The Privacy subfield of the record's Capability Information.
    pub const fn privacy(&self) -> bool {
        self.capability_info.privacy()
    }

    /// Whether the frame carried an RSN element.
    pub const fn rsn(&self) -> bool {
        self.rsn_ie_count != 0
    }

    /// A record's identity, before its information elements are decoded.
    /// Empty table slots use `None`, never this record.
    pub const fn new(bssid: Bssid) -> Self {
        Self {
            ssid: [0; 32],
            ssid_len: oer_portable_wire::PrefixLength::ZERO,
            bssid,
            ds_channel: None,

            rsn_ie_count: 0,
            legacy_wpa: ElementPresence::Absent,
            information_elements: ElementList::Complete,
            capability_info: crate::management::CapabilityInformation::from_bytes([0; 2]),
            timestamp: crate::tsf::TsfInstant::EPOCH,
            beacon_interval: crate::tsf::TimeUnits::ZERO,
            supported_rates: [0; 8],
            supported_rates_len: oer_portable_wire::PrefixLength::ZERO,
            extended_supported_rates: [0; EXTENDED_RATES_CAPACITY],
            extended_supported_rates_len: oer_portable_wire::PrefixLength::ZERO,
            ht_capability_ie: None,
            ht_operation_ie: None,
            he_capability_ie: [0; HE_CAPABILITY_IE_CAPACITY],
            he_capability_ie_len: oer_portable_wire::PrefixLength::ZERO,
            he_operation_ie: [0; HE_OPERATION_IE_CAPACITY],
            he_operation_ie_len: oer_portable_wire::PrefixLength::ZERO,
            wmm_ie: [0; WMM_IE_CAPACITY],
            wmm_ie_len: oer_portable_wire::PrefixLength::ZERO,
            rsn_ie: [0; RSN_IE_CAPACITY],
            rsn_ie_len: oer_portable_wire::PrefixLength::ZERO,
            rsnxe: [0; RSNXE_CAPACITY],
            rsnxe_len: oer_portable_wire::PrefixLength::ZERO,
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
    /// `oer-chips-esp32s31-pac/src/phy/frequency.rs::bss_tx_offset` from the
    /// complete rev0 ROM `phy_bb_bss_cbw40`.
    pub fn secondary_channel(&self, primary_channel: u8) -> Option<HtSecondaryChannel> {
        let capability = self.ht_capability_ie_bytes()?;
        let operation = self.ht_operation_ie_bytes()?;
        let capability_info =
            crate::ht::HtCapabilityInfo::from_bytes([capability[2], capability[3]]);
        if !capability_info.width40()
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
    let mut reader = oer_portable_wire::Reader::new(header.body());
    let fixed = reader.array::<12>().ok()?;
    let element_bytes = reader.remaining();
    let mut elements = oer_portable_wire::Reader::new(element_bytes);
    let [t0, t1, t2, t3, t4, t5, t6, t7, i0, i1, c0, c1] = *fixed;

    let bssid = Bssid::new(IndividualAddress::new(header.address3())?)?;
    let mut record = ScanRecord::new(bssid);
    record.timestamp =
        crate::tsf::TimestampField::from_bytes([t0, t1, t2, t3, t4, t5, t6, t7]).instant();
    record.beacon_interval =
        crate::management::BeaconIntervalField::from_bytes([i0, i1]).interval();
    record.capability_info = crate::management::CapabilityInformation::from_bytes([c0, c1]);

    while let Ok(&element_header) = elements.array::<2>() {
        let [id, length] = element_header;
        let Ok(value) = elements.take(usize::from(length)) else {
            record.information_elements = ElementList::Truncated;
            break;
        };
        match (ElementId::new(id), value) {
            (ElementId::SSID, _) => {
                if let Some(length) = store_value(&mut record.ssid, value) {
                    record.ssid_len = length;
                }
            }
            (ElementId::SUPPORTED_RATES, _) => {
                if let Some(length) = store_value(&mut record.supported_rates, value) {
                    record.supported_rates_len = length;
                }
            }
            (ElementId::DSSS_PARAMETER_SET, &[channel]) => record.ds_channel = Some(channel),
            (ElementId::HT_CAPABILITIES, _) => {
                if let Some(element) = exact_element(element_header, value) {
                    record.ht_capability_ie = Some(element);
                }
            }
            (ElementId::RSN, _) => {
                record.rsn_ie_count = record.rsn_ie_count.saturating_add(1);
                match store_element(&mut record.rsn_ie, element_header, value) {
                    Some(length) => record.rsn_ie_len = length,
                    None => record.information_elements = ElementList::Truncated,
                }
            }
            (ElementId::EXTENDED_SUPPORTED_RATES, _) => {
                let copied = record
                    .extended_supported_rates
                    .iter_mut()
                    .zip(value)
                    .map(|(stored, rate)| *stored = *rate)
                    .count();
                record.extended_supported_rates_len = oer_portable_wire::PrefixLength::new(copied)?;
                if copied != value.len() {
                    record.information_elements = ElementList::Truncated;
                }
            }
            (ElementId::HT_OPERATION, _) => {
                if let Some(element) = exact_element(element_header, value) {
                    record.ht_operation_ie = Some(element);
                }
            }
            (ElementId::RSNX, _) => match store_element(&mut record.rsnxe, element_header, value) {
                Some(length) => record.rsnxe_len = length,
                None => record.information_elements = ElementList::Truncated,
            },
            (ElementId::EXTENSION, &[extension, ..])
                if ElementIdExtension::new(extension) == ElementIdExtension::HE_CAPABILITIES =>
            {
                match store_element(&mut record.he_capability_ie, element_header, value) {
                    Some(length) => record.he_capability_ie_len = length,
                    None => record.information_elements = ElementList::Truncated,
                }
            }
            (ElementId::EXTENSION, &[extension, ..])
                if ElementIdExtension::new(extension) == ElementIdExtension::HE_OPERATION =>
            {
                match store_element(&mut record.he_operation_ie, element_header, value) {
                    Some(length) => record.he_operation_ie_len = length,
                    None => record.information_elements = ElementList::Truncated,
                }
            }
            (ElementId::VENDOR_SPECIFIC, [0x00, 0x50, 0xf2, 0x01, ..]) => {
                record.legacy_wpa = ElementPresence::Present
            }
            (ElementId::VENDOR_SPECIFIC, [0x00, 0x50, 0xf2, 0x02, _, _, ..]) => {
                match store_element(&mut record.wmm_ie, element_header, value) {
                    Some(length) => record.wmm_ie_len = length,
                    None => record.information_elements = ElementList::Truncated,
                }
            }
            _ => {}
        }
    }
    // One trailing byte cannot form an IE header. Treat it as malformed,
    // rather than silently accepting it as absence of a later RSN/WPA IE.
    if !elements.is_empty() {
        record.information_elements = ElementList::Truncated;
    }
    Some((record, element_bytes))
}

/// The first `length` bytes of a record buffer: the bytes it holds.
fn front<const N: usize>(buffer: &[u8; N], length: oer_portable_wire::PrefixLength<N>) -> &[u8] {
    length.borrow(buffer)
}

/// Replace the bytes a record buffer holds by `value` and zero the rest, or
/// refuse a value longer than the buffer and change nothing.
fn replace<const N: usize>(
    buffer: &mut [u8; N],
    length: &mut oer_portable_wire::PrefixLength<N>,
    value: &[u8],
) -> Result<(), ScanRecordError> {
    let too_long = ScanRecordError::TooLong {
        length: value.len(),
        capacity: N,
    };
    let value_length = oer_portable_wire::PrefixLength::new(value.len()).ok_or(too_long)?;
    buffer.fill(0);
    oer_portable_wire::Writer::new(value_length.borrow_mut(buffer))
        .write(value)
        .map_err(|_| too_long)?;
    *length = value_length;
    Ok(())
}

/// Copy an element's value to the front of `buffer`: its length, or `None`
/// when it does not fit and `buffer` is unchanged.
pub(crate) fn store_value<const N: usize>(
    buffer: &mut [u8; N],
    value: &[u8],
) -> Option<oer_portable_wire::PrefixLength<N>> {
    let length = oer_portable_wire::PrefixLength::new(value.len())?;
    oer_portable_wire::Writer::new(length.borrow_mut(buffer))
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
) -> Option<oer_portable_wire::PrefixLength<N>> {
    let length = oer_portable_wire::PrefixLength::new(value.len().checked_add(2)?)?;
    let (stored_header, stored_value) = length.borrow_mut(buffer).split_first_chunk_mut::<2>()?;
    *stored_header = header;
    oer_portable_wire::Writer::new(stored_value)
        .write(value)
        .ok()?;
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
    oer_portable_wire::Writer::new(stored_value)
        .write(value)
        .ok()?;
    Some(element)
}

#[cfg(test)]
mod record_tests {
    use super::*;
    use oer_protocols_ieee802_wire::Eui48;

    fn record() -> ScanRecord {
        ScanRecord::new(
            Bssid::new(IndividualAddress::new(Eui48::new([2, 0, 0, 0, 0, 1])).unwrap()).unwrap(),
        )
    }

    /// A 40 MHz BSS needs both the HT capability's 40 MHz bit and the HT
    /// operation's, on its own primary channel; its secondary is the
    /// operation's offset.
    #[test]
    fn a_secondary_channel_needs_40_mhz_on_both_elements() {
        let with = |capability_info: u8, primary: u8, operation_info: u8| {
            let mut record = record();
            let mut capability = [0_u8; HT_CAPABILITY_IE_LEN];
            capability[..4].copy_from_slice(&[45, 26, capability_info, 0]);
            let mut operation = [0_u8; HT_OPERATION_IE_LEN];
            operation[..4].copy_from_slice(&[61, 22, primary, operation_info]);
            record.ht_capability_ie = Some(capability);
            record.ht_operation_ie = Some(operation);
            record.secondary_channel(6)
        };
        // Operation info: secondary offset in bits 0-1, width in bit 2.
        assert_eq!(with(0x02, 6, 0x05), Some(HtSecondaryChannel::Above));
        assert_eq!(with(0x02, 6, 0x07), Some(HtSecondaryChannel::Below));
        assert_eq!(with(0x00, 6, 0x05), None, "no 40 MHz capability");
        assert_eq!(with(0x02, 6, 0x01), None, "no 40 MHz operation");
        assert_eq!(with(0x02, 7, 0x05), None, "another primary channel");
    }

    #[test]
    fn privacy_is_the_capability_s_bit_and_rsn_an_element_counted() {
        let mut record = record();
        assert!(!record.privacy() && !record.rsn());
        record.capability_info =
            crate::management::CapabilityInformation::from_bytes([0; 2]).with_privacy(true);
        assert!(record.privacy() && !record.rsn());
        record.capability_info = crate::management::CapabilityInformation::from_bytes([0; 2]);
        record.rsn_ie_count = 1;
        assert!(!record.privacy() && record.rsn());
    }

    /// A beacon of `elements`.
    fn scanned(elements: &[u8]) -> ScanRecord {
        let mut frame = std::vec![0_u8; 36];
        frame[0] = 0x80;
        frame[4..10].fill(0xff);
        frame[10..16].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
        frame[16..22].copy_from_slice(&[2, 0, 0, 0, 0, 1]);
        frame.extend_from_slice(elements);
        *ScanFrame::parse(&frame).unwrap().record()
    }

    /// A complete element of `id` whose body is `body`.
    fn element(id: u8, body: &[u8]) -> std::vec::Vec<u8> {
        let mut element = std::vec![id, body.len() as u8];
        element.extend_from_slice(body);
        element
    }

    /// Each element is kept in its own place and leaves every other place
    /// empty.
    #[test]
    fn each_element_is_kept_in_its_place_alone() {
        let ht_capability = element(45, &[1; 26]);
        let ht_operation = element(61, &[2; 22]);
        let rsn = element(48, &[1, 0]);
        let rsnxe = element(244, &[0x20]);
        let he_capability = element(255, &[35, 3, 3]);
        let he_operation = element(255, &[36, 4, 4]);
        let wmm = element(221, &[0x00, 0x50, 0xf2, 0x02, 1, 1]);
        let wpa = element(221, &[0x00, 0x50, 0xf2, 0x01, 1, 0]);
        let all: [&[u8]; 13] = [
            &[0, 2, b'a', b'b'],
            &[1, 2, 0x82, 0x84],
            &[3, 1, 11],
            &ht_capability,
            &rsn,
            &[50, 1, 0x6c],
            &ht_operation,
            &rsnxe,
            &he_capability,
            &he_operation,
            &wpa,
            &wmm,
            // Another extension and another vendor are kept nowhere.
            &[255, 2, 37, 1, 221, 6, 0, 0x50, 0xf3, 2, 1, 1],
        ];
        let places = |record: &ScanRecord| {
            [
                record.ssid_bytes() == b"ab",
                record.supported_rates_bytes() == [0x82, 0x84],
                record.ds_channel == Some(11),
                record.ht_capability_ie_bytes().map(|ie| &ie[..]) == Some(&ht_capability[..]),
                record.rsn() && record.rsn_ie_bytes() == rsn,
                record.extended_supported_rates_bytes() == [0x6c],
                record.ht_operation_ie_bytes().map(|ie| &ie[..]) == Some(&ht_operation[..]),
                record.rsnxe_bytes() == rsnxe,
                record.he_capability_ie_bytes() == he_capability,
                record.he_operation_ie_bytes() == he_operation,
                record.legacy_wpa == ElementPresence::Present,
                record.wmm_ie_bytes() == wmm,
                false,
            ]
        };
        for (index, one) in all.iter().enumerate() {
            let record = scanned(one);
            assert_eq!(record.information_elements, ElementList::Complete);
            for (place, filled) in places(&record).into_iter().enumerate() {
                assert_eq!(filled, place == index && index != 12, "{index} {place}");
            }
            // And no other place holds anything.
            assert_eq!(
                record.he_capability_ie_bytes().is_empty(),
                index != 8,
                "{index}"
            );
            assert_eq!(
                record.he_operation_ie_bytes().is_empty(),
                index != 9,
                "{index}"
            );
            assert_eq!(record.wmm_ie_bytes().is_empty(), index != 11, "{index}");
        }
    }

    /// The HE Operation helpers read only an HE Operation element.
    #[test]
    fn he_operation_fields_are_read_from_its_element_alone() {
        assert_eq!(
            he_default_packet_extension_duration(&[255, 4, 36, 0b1010_1100, 0, 0]),
            Some(crate::he::DefaultPeDuration::Us16)
        );
        assert_eq!(
            he_default_packet_extension_duration(&[255, 4, 36, 0, 0, 0]),
            Some(crate::he::DefaultPeDuration::Us0)
        );
        for (parameters, disabled) in [(0, false), (1, true)] {
            assert_eq!(
                he_extended_range_single_user_disabled(&[255, 4, 36, 0, 0, parameters]),
                Some(disabled)
            );
        }
        for other in [[255, 4, 35, 4, 0, 1], [254, 4, 36, 4, 0, 1]] {
            assert_eq!(he_default_packet_extension_duration(&other), None);
            assert_eq!(he_extended_range_single_user_disabled(&other), None);
        }
    }
}
