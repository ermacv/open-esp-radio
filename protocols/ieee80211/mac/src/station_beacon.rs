//! Bounded infrastructure-beacon observation for a connected station.
//!
//! This parser owns IEEE 802.11 fields only. It has no clock, executor,
//! hardware or sleep policy; the connected runtime decides how a received
//! observation changes its beacon-loss deadline and power state.
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
    let (header, elements) = mpdu
        .split_first_chunk::<FIXED_BEACON_LENGTH>()
        .ok_or(StaBeaconError::Truncated)?;
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
    let frame_control = u16::from_le_bytes([fc0, fc1]);
    if frame_control & FRAME_TYPE_AND_SUBTYPE_MASK != BEACON_FRAME_CONTROL {
        return Err(StaBeaconError::NotBeacon);
    }
    if [a0, a1, a2, a3, a4, a5] != expected_bssid || [b0, b1, b2, b3, b4, b5] != expected_bssid {
        return Err(StaBeaconError::ForeignBssid);
    }
    let interval_tu = u16::from_le_bytes([i0, i1]);
    if interval_tu == 0 {
        return Err(StaBeaconError::ZeroInterval);
    }

    let mut tim = None;
    let mut protection = StaBeaconProtection::default();
    let mut remaining = elements;
    while !remaining.is_empty() {
        let (&[id, length], rest) = remaining
            .split_first_chunk::<2>()
            .ok_or(StaBeaconError::MalformedInformationElement)?;
        let (body, rest) = rest
            .split_at_checked(usize::from(length))
            .ok_or(StaBeaconError::MalformedInformationElement)?;
        remaining = rest;
        if id == TIM_ELEMENT_ID {
            let &[dtim_count, dtim_period, bitmap_control, ref bitmap @ ..] = body else {
                return Err(StaBeaconError::MalformedTim);
            };
            if bitmap.is_empty() || dtim_period == 0 || tim.is_some() {
                return Err(StaBeaconError::MalformedTim);
            }
            let aid = usize::from(association_id & 0x3fff);
            let aid_byte = aid / 8;
            // Bitmap Offset is encoded in units of two octets in bits 7:1;
            // masking bit zero therefore already yields N1 in octets.
            let bitmap_offset = usize::from(bitmap_control & 0xfe);
            // The AID's bit within its octet is below eight.
            let aid_bit = 1_u8.wrapping_shl(u32::from(association_id & 7));
            let unicast_buffered = aid_byte
                .checked_sub(bitmap_offset)
                .and_then(|index| bitmap.get(index))
                .is_some_and(|byte| byte & aid_bit != 0);
            tim = Some(StaTimObservation {
                dtim_count,
                dtim_period,
                unicast_buffered,
                group_buffered: dtim_count == 0 && bitmap_control & 1 != 0,
            });
        }
        match (id, body) {
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
    }

    let channel_switch = crate::channel_switch::parse_channel_switch(elements).transpose();
    Ok(StaBeaconObservation {
        timestamp_tsf: crate::tsf::TsfInstant::from_micros(u64::from_le_bytes([
            t0, t1, t2, t3, t4, t5, t6, t7,
        ])),
        interval_tu,
        capability_information: u16::from_le_bytes([c0, c1]),
        tim,
        protection,
        channel_switch,
    })
}

#[cfg(test)]
mod tests;
