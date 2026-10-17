//! Bounded infrastructure-beacon observation for a connected station.
//!
//! This parser owns IEEE 802.11 fields only. It has no clock, executor,
//! hardware or sleep policy; the connected runtime decides how a received
//! observation changes its beacon-loss deadline and power state.

const BEACON_FRAME_CONTROL: u16 = 0x0080;
const FRAME_TYPE_AND_SUBTYPE_MASK: u16 = 0x00fc;
const FIXED_BEACON_LENGTH: usize = 36;
const TIM_ELEMENT_ID: u8 = 5;
const ERP_ELEMENT_ID: u8 = 42;
const HT_OPERATION_ELEMENT_ID: u8 = 61;
const EXTENSION_ELEMENT_ID: u8 = 255;
const HE_OPERATION_EXTENSION_ID: u8 = 36;

use crate::protection::{ErpProtection, HtProtectionMode};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StaBeaconError {
    Truncated,
    NotBeacon,
    ForeignBssid,
    ZeroInterval,
    MalformedInformationElement,
    MalformedTim,
}

/// Traffic indication decoded for the connected station's Association ID.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaTimObservation {
    pub dtim_count: u8,
    pub dtim_period: u8,
    pub unicast_buffered: bool,
    pub group_buffered: bool,
}

/// Fixed connected-beacon fields that may outlive the borrowed MPDU.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaBeaconObservation {
    /// The beacon's Timestamp field: the access point's TSF.
    pub timestamp_tsf: crate::tsf::TsfInstant,
    pub interval_tu: u16,
    pub capability_information: u16,
    pub tim: Option<StaTimObservation>,
    pub protection: StaBeaconProtection,
    /// The BSS's announced move to another channel, if the beacon carries
    /// one. A malformed announcement is reported here and leaves the rest of
    /// the beacon valid: the station decides what it does without one.
    pub channel_switch: Option<
        Result<crate::channel_switch::ChannelSwitch, crate::channel_switch::ChannelSwitchError>,
    >,
}

/// BSS protection fields advertised by one beacon.
///
/// An absent ERP or HT Operation element carries no requirement. The HE
/// TXOP Duration RTS Threshold is `None` when the HE Operation element is
/// absent or encodes the disabled values zero or 1023.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StaBeaconProtection {
    pub erp: ErpProtection,
    pub ht: HtProtectionMode,
    pub he_txop_rts_threshold: Option<u16>,
}

impl StaBeaconProtection {
    /// A beacon without any protection requirement.
    pub const UNPROTECTED: Self = Self {
        erp: ErpProtection::NONE,
        ht: HtProtectionMode::None,
        he_txop_rts_threshold: None,
    };
}

/// Parse one beacon from the associated BSS and locate the station's TIM bit.
pub fn parse_sta_beacon(
    mpdu: &[u8],
    expected_bssid: [u8; 6],
    association_id: u16,
) -> Result<StaBeaconObservation, StaBeaconError> {
    if mpdu.len() < FIXED_BEACON_LENGTH {
        return Err(StaBeaconError::Truncated);
    }
    let frame_control = u16::from_le_bytes([mpdu[0], mpdu[1]]);
    if frame_control & FRAME_TYPE_AND_SUBTYPE_MASK != BEACON_FRAME_CONTROL {
        return Err(StaBeaconError::NotBeacon);
    }
    if mpdu[10..16] != expected_bssid || mpdu[16..22] != expected_bssid {
        return Err(StaBeaconError::ForeignBssid);
    }
    let interval_tu = u16::from_le_bytes([mpdu[32], mpdu[33]]);
    if interval_tu == 0 {
        return Err(StaBeaconError::ZeroInterval);
    }

    let mut timestamp = [0_u8; 8];
    timestamp.copy_from_slice(&mpdu[24..32]);
    let mut tim = None;
    let mut protection = StaBeaconProtection::default();
    let mut offset = FIXED_BEACON_LENGTH;
    while offset < mpdu.len() {
        let header = mpdu
            .get(offset..offset + 2)
            .ok_or(StaBeaconError::MalformedInformationElement)?;
        let length = usize::from(header[1]);
        let body_start = offset + 2;
        let body_end = body_start
            .checked_add(length)
            .filter(|end| *end <= mpdu.len())
            .ok_or(StaBeaconError::MalformedInformationElement)?;
        if header[0] == TIM_ELEMENT_ID {
            let body = &mpdu[body_start..body_end];
            if body.len() < 4 || body[1] == 0 || tim.is_some() {
                return Err(StaBeaconError::MalformedTim);
            }
            let bitmap_control = body[2];
            let aid = usize::from(association_id & 0x3fff);
            let aid_byte = aid / 8;
            // Bitmap Offset is encoded in units of two octets in bits 7:1;
            // masking bit zero therefore already yields N1 in octets.
            let bitmap_offset = usize::from(bitmap_control & 0xfe);
            let unicast_buffered = aid_byte
                .checked_sub(bitmap_offset)
                .and_then(|index| body.get(3 + index))
                .is_some_and(|byte| byte & (1 << (aid & 7)) != 0);
            tim = Some(StaTimObservation {
                dtim_count: body[0],
                dtim_period: body[1],
                unicast_buffered,
                group_buffered: body[0] == 0 && bitmap_control & 1 != 0,
            });
        }
        let body = &mpdu[body_start..body_end];
        match (header[0], body) {
            (ERP_ELEMENT_ID, [information]) => {
                protection.erp = ErpProtection::from_information(Some(*information));
            }
            (HT_OPERATION_ELEMENT_ID, [_, _, information, ..]) if body.len() == 22 => {
                protection.ht = HtProtectionMode::from_field(*information);
            }
            // HE Operation Parameters: TXOP Duration RTS Threshold in bits
            // 13:4 of its first two octets.
            (EXTENSION_ELEMENT_ID, [HE_OPERATION_EXTENSION_ID, low, high, ..]) => {
                let threshold = ((u16::from(*high) & 0x3f) << 4) | (u16::from(*low) >> 4);
                protection.he_txop_rts_threshold =
                    (!matches!(threshold, 0 | 0x03ff)).then_some(threshold);
            }
            _ => {}
        }
        offset = body_end;
    }

    let channel_switch =
        crate::channel_switch::parse_channel_switch(&mpdu[FIXED_BEACON_LENGTH..]).transpose();
    Ok(StaBeaconObservation {
        timestamp_tsf: crate::tsf::TsfInstant::from_micros(u64::from_le_bytes(timestamp)),
        interval_tu,
        capability_information: u16::from_le_bytes([mpdu[34], mpdu[35]]),
        tim,
        protection,
        channel_switch,
    })
}

#[cfg(test)]
mod tests;
