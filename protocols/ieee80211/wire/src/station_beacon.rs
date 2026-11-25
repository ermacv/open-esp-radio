//! Bounded infrastructure-beacon observation for a connected station.
//!
//! This parser owns IEEE 802.11 fields only. It has no clock, executor,
//! hardware or sleep policy; the connected runtime decides how a received
//! observation changes its beacon-loss deadline and power state.
//!
//! Received bytes reach this module; nothing here panics on them.

use crate::element::{ElementId, ElementIdExtension};
use crate::frame::FrameControlFields;
use crate::management::AidField;
use oer_protocols_ieee802_wire::Eui48;

const BEACON_FRAME_CONTROL: FrameControlFields = FrameControlFields::from_bytes([0x80, 0x00]);
const FIXED_BEACON_LENGTH: usize = 36;

use crate::protection::{ErpElements, ErpElementsError, HtProtectionMode};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaBeaconError {
    Truncated,
    NotBeacon,
    ForeignBssid,
    ZeroInterval,
    MalformedInformationElement,
    MalformedTim,
}

/// Whether the access point buffers traffic, as a TIM element indicates it
/// (IEEE Std 802.11-2020, 9.4.2.5).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BufferedTraffic {
    Buffered,
    Clear,
}

/// Traffic indication decoded for the connected station's Association ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaTimObservation {
    pub dtim_count: u8,
    pub dtim_period: u8,
    /// The partial virtual bitmap's bit of the station's AID.
    pub unicast: BufferedTraffic,
    /// Group traffic buffered, indicated in a DTIM beacon.
    pub group: BufferedTraffic,
}

/// Fixed connected-beacon fields that may outlive the borrowed MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaBeaconObservation {
    /// The beacon's Timestamp field: the access point's TSF.
    pub timestamp_tsf: crate::tsf::TsfTimestamp,
    pub interval_tu: u16,
    pub capability_information: crate::management::CapabilityInformation,
    pub tim: Option<StaTimObservation>,
    pub protection: StaBeaconProtection,
    /// The EDCA parameters of the beacon's WMM Parameter element, if it
    /// carries one that parses.
    pub edca: Option<crate::extensions::wmm::WmmParameterSet>,
    /// The BSS's announced move to another channel, if the beacon carries
    /// one. A malformed announcement is reported here and leaves the rest of
    /// the beacon valid: the station decides what it does without one.
    pub channel_switch: Option<
        Result<crate::channel_switch::ChannelSwitch, crate::channel_switch::ChannelSwitchError>,
    >,
}

/// HT and HE protection fields advertised by one beacon; its ERP elements
/// are [`StaBeaconFrame::erp`]'s, for the station to fold.
///
/// An absent HT Operation element carries no requirement. The HE TXOP
/// Duration RTS Threshold is `None` when the HE Operation element is absent
/// or encodes the disabled values zero or 1023.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StaBeaconProtection {
    pub ht: HtProtectionMode,
    pub he_txop_rts_threshold: Option<u16>,
}

impl StaBeaconProtection {
    /// A beacon without any HT or HE protection requirement.
    pub const UNPROTECTED: Self = Self {
        ht: HtProtectionMode::None,
        he_txop_rts_threshold: None,
    };
}

/// One parsed beacon: its fixed observation, and its ERP elements over the
/// borrowed MPDU. A damaged ERP element is reported here and leaves the rest
/// of the beacon valid: the station decides what it requires without one.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaBeaconFrame<'a> {
    pub observation: StaBeaconObservation,
    pub erp: Result<ErpElements<'a>, ErpElementsError>,
}

/// Parse one beacon from the associated BSS and locate the station's TIM bit.
pub fn parse_sta_beacon(
    mpdu: &[u8],
    expected_bssid: Eui48,
    association_id: AidField,
) -> Result<StaBeaconFrame<'_>, StaBeaconError> {
    let mut reader = oer_portable_wire::Reader::new(mpdu);
    let header = reader
        .array::<FIXED_BEACON_LENGTH>()
        .map_err(|_| StaBeaconError::Truncated)?;
    let elements = reader.remaining();
    let [
        fc0,
        fc1,
        _duration0,
        _duration1,
        // Receiver address.
        _,
        _,
        _,
        _,
        _,
        _,
        a0,
        a1,
        a2,
        a3,
        a4,
        a5,
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
    let frame_control = FrameControlFields::from_bytes([fc0, fc1]);
    if frame_control.type_and_subtype() != BEACON_FRAME_CONTROL.type_and_subtype() {
        return Err(StaBeaconError::NotBeacon);
    }
    if Eui48::new([a0, a1, a2, a3, a4, a5]) != expected_bssid
        || Eui48::new([b0, b1, b2, b3, b4, b5]) != expected_bssid
    {
        return Err(StaBeaconError::ForeignBssid);
    }
    let interval_tu = crate::management::BeaconIntervalField::from_bytes([i0, i1]).time_units();
    if interval_tu == 0 {
        return Err(StaBeaconError::ZeroInterval);
    }

    let mut tim = None;
    let mut protection = StaBeaconProtection::default();
    let mut edca = None;
    let mut remaining = elements;
    while !remaining.is_empty() {
        let element = remaining;
        let mut reader = oer_portable_wire::Reader::new(remaining);
        let &[id, length] = reader
            .array::<2>()
            .map_err(|_| StaBeaconError::MalformedInformationElement)?;
        let body = reader
            .take(usize::from(length))
            .map_err(|_| StaBeaconError::MalformedInformationElement)?;
        remaining = reader.remaining();
        let id = ElementId::new(id);
        if id == ElementId::VENDOR_SPECIFIC
            && let Some(element) = usize::from(length)
                .checked_add(2)
                .and_then(|whole| oer_portable_wire::Reader::new(element).take(whole).ok())
            && let Some(parameters) = crate::extensions::wmm::parse_wmm_parameter_element(element)
        {
            edca = Some(parameters);
        }
        if id == ElementId::TIM {
            let &[dtim_count, dtim_period, bitmap_control, ref bitmap @ ..] = body else {
                return Err(StaBeaconError::MalformedTim);
            };
            if bitmap.is_empty() || dtim_period == 0 || tim.is_some() {
                return Err(StaBeaconError::MalformedTim);
            }
            // The station's association ID, as the Association Response's
            // AID field gave it ([`crate::station::AssociationResponse`]).
            let aid = association_id.aid().get();
            let aid_byte = usize::from(aid / 8);
            // Bitmap Offset is encoded in units of two octets in bits 7:1;
            // the decoded offset is doubled to obtain N1 in octets.
            let bitmap_offset =
                usize::from(crate::beacon::TimBitmapControl::from_bytes([bitmap_control]).offset())
                    * 2;
            // The AID's bit within its octet is below eight.
            let aid_bit = 1_u8.wrapping_shl(u32::from(aid & 7));
            let unicast_buffered = aid_byte
                .checked_sub(bitmap_offset)
                .and_then(|index| bitmap.get(index))
                .is_some_and(|byte| byte & aid_bit != 0);
            tim = Some(StaTimObservation {
                dtim_count,
                dtim_period,
                unicast: if unicast_buffered {
                    BufferedTraffic::Buffered
                } else {
                    BufferedTraffic::Clear
                },
                group: if dtim_count == 0
                    && crate::beacon::TimBitmapControl::from_bytes([bitmap_control]).group()
                {
                    BufferedTraffic::Buffered
                } else {
                    BufferedTraffic::Clear
                },
            });
        }
        match (id, body) {
            (ElementId::HT_OPERATION, [_, _, information, ..]) if body.len() == 22 => {
                protection.ht = HtProtectionMode::from_field(*information);
            }
            // HE Operation Parameters: TXOP Duration RTS Threshold in bits
            // 13:4 of its first two octets.
            (ElementId::EXTENSION, [extension, low, high, ..])
                if ElementIdExtension::new(*extension) == ElementIdExtension::HE_OPERATION =>
            {
                let threshold = crate::he::HeOperationFields::from_bytes([*low, *high, 0, 0, 0, 0])
                    .rts_threshold();
                protection.he_txop_rts_threshold =
                    (!matches!(threshold, 0 | 0x03ff)).then_some(threshold);
            }
            _ => {}
        }
    }

    let channel_switch = crate::channel_switch::parse_channel_switch(elements).transpose();
    Ok(StaBeaconFrame {
        observation: StaBeaconObservation {
            timestamp_tsf: crate::tsf::TsfTimestamp::from_bytes([t0, t1, t2, t3, t4, t5, t6, t7]),
            interval_tu,
            capability_information: crate::management::CapabilityInformation::from_bytes([c0, c1]),
            tim,
            protection,
            edca,
            channel_switch,
        },
        erp: ErpElements::parse(elements),
    })
}

#[cfg(test)]
mod tests;
