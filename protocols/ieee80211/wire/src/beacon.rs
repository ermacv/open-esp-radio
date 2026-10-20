//! Bounded, allocation-free AP beacon field ownership.
//!
//! Locate the fixed-size TIM element and derive its DTIM phase from the
//! supplied timestamp and the beacon's advertised interval. The caller owns
//! the clock relation; this codec does not read a backend or executor clock.

use crate::{
    ap::profile::Advertisement,
    channel::{Band, Channel},
    ht::{ht_capability_ie, ht_operation_ie},
    protection::ApBssProtection,
    security::ApSecurityPolicy,
    sequence::SequenceNumber,
    ssid::WifiSsid,
};

pub const AP_BEACON_CAPACITY: usize = 256;
pub const TIM_MAX_ASSOCIATION_ID: u16 = 2_007;
pub const TIM_MAX_VIRTUAL_BITMAP_OCTETS: usize = 251;

const MANAGEMENT_HEADER_LEN: usize = 24;
const BEACON_FIXED_BODY_LEN: usize = 12;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApBeaconBuildError {
    InvalidPrimaryChannel,
    InvalidDtimPeriod,
    /// A 5 GHz BSS advertises a DSSS or HR/DSSS rate (1, 2, 5.5 or
    /// 11 Mbit/s), which that band does not carry.
    DsssRateIn5Ghz,
    OutputTooSmall {
        required: usize,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TimBitmapError {
    InvalidCapacity {
        bitmap_octets: usize,
    },
    InvalidAssociationId(u16),
    AssociationIdOutsideCapacity {
        association_id: u16,
        bitmap_octets: usize,
    },
}

/// Valid unicast association identifier for an IEEE 802.11 TIM bitmap.
///
/// AID zero is represented by the multicast indication in Bitmap Control and
/// is therefore intentionally not constructible through this type.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(transparent)]
pub struct TimAssociationId(u16);

impl TimAssociationId {
    pub const fn new(value: u16) -> Result<Self, TimBitmapError> {
        if value == 0 || value > TIM_MAX_ASSOCIATION_ID {
            return Err(TimBitmapError::InvalidAssociationId(value));
        }
        Ok(Self(value))
    }

    pub const fn get(self) -> u16 {
        self.0
    }
}

/// Fixed-capacity owner for the complete virtual bitmap in one AP profile.
///
/// `OCTETS` is validated when the value is constructed. Setting an AID beyond
/// that profile fails explicitly instead of aliasing it through `aid & 7`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimVirtualBitmap<const OCTETS: usize> {
    octets: [u8; OCTETS],
}

impl<const OCTETS: usize> TimVirtualBitmap<OCTETS> {
    pub const fn try_new() -> Result<Self, TimBitmapError> {
        if OCTETS == 0 || OCTETS > TIM_MAX_VIRTUAL_BITMAP_OCTETS {
            return Err(TimBitmapError::InvalidCapacity {
                bitmap_octets: OCTETS,
            });
        }
        Ok(Self {
            octets: [0; OCTETS],
        })
    }

    pub fn set(
        &mut self,
        association_id: TimAssociationId,
        buffered: bool,
    ) -> Result<(), TimBitmapError> {
        let association_id = association_id.get();
        let octet = usize::from(association_id / 8);
        let Some(value) = self.octets.get_mut(octet) else {
            return Err(TimBitmapError::AssociationIdOutsideCapacity {
                association_id,
                bitmap_octets: OCTETS,
            });
        };
        let mask = 1_u8 << (association_id % 8);
        if buffered {
            *value |= mask;
        } else {
            *value &= !mask;
        }
        Ok(())
    }

    /// Derive canonical N1/N2 bounds for the Partial Virtual Bitmap field.
    /// N1 is even, so a set bit in odd octet 1 still retains octet 0.
    pub fn partial(&self) -> TimPartialVirtualBitmap<'_> {
        let first = self.octets.iter().position(|octet| *octet != 0);
        let last = self.octets.iter().rposition(|octet| *octet != 0);
        let (Some(first), Some(last)) = (first, last) else {
            return TimPartialVirtualBitmap {
                bitmap_offset: 0,
                octets: &self.octets[..1],
            };
        };
        let first = first & !1;
        TimPartialVirtualBitmap {
            bitmap_offset: first as u8,
            octets: &self.octets[first..=last],
        }
    }
}

/// Borrowed canonical Partial Virtual Bitmap and its absolute even N1 offset.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TimPartialVirtualBitmap<'bitmap> {
    bitmap_offset: u8,
    octets: &'bitmap [u8],
}

impl<'bitmap> TimPartialVirtualBitmap<'bitmap> {
    pub const fn bitmap_offset(self) -> u8 {
        self.bitmap_offset
    }

    pub const fn octets(self) -> &'bitmap [u8] {
        self.octets
    }
}

/// Build the exact Open or WPA2-Personal HT/WMM beacon without allocating.
///
/// Open clears Privacy and omits the RSN element. The ERP and HT Operation
/// elements carry `protection`; [`update_bss_protection`] replaces them in
/// place when the peer set changes. The caller owns timestamp/DTIM
/// progression through [`stamp`]; the two-byte TIM bitmap covers the public
/// AID 1..=15 range.
#[expect(
    clippy::too_many_arguments,
    reason = "the frame writer keeps the caller-owned local profile and independent 802.11 fields explicit"
)]
pub fn write_ht_beacon(
    profile: &Advertisement,
    output: &mut [u8],
    access_point: [u8; 6],
    ssid: &WifiSsid,
    channel: Channel,
    beacon_interval_tu: u16,
    dtim_period: u8,
    management_sequence: SequenceNumber,
    security: ApSecurityPolicy,
    protection: ApBssProtection,
) -> Result<usize, ApBeaconBuildError> {
    // The 2.4 GHz BSS carries the DSSS Parameter Set and the ERP element;
    // a 5 GHz one carries neither, nor any DSSS rate.
    let ghz2_4 = channel.band() == Band::Ghz2_4;
    if ghz2_4 && !(1..=13).contains(&channel.number()) {
        return Err(ApBeaconBuildError::InvalidPrimaryChannel);
    }
    if !ghz2_4
        && profile
            .legacy_rates
            .supported()
            .iter()
            .chain(profile.legacy_rates.extended())
            .any(|rate| matches!(rate & 0x7f, 2 | 4 | 11 | 22))
    {
        return Err(ApBeaconBuildError::DsssRateIn5Ghz);
    }
    if dtim_period == 0 {
        return Err(ApBeaconBuildError::InvalidDtimPeriod);
    }

    let wmm_parameter_ie = profile.wmm.element();
    let ht_capability = ht_capability_ie(profile.ht, channel);
    let ht_operation = ht_operation_ie(channel, protection.ht);
    let advertised = security.advertisement();
    let (rsn, rsnx) = (advertised.rsne, advertised.rsnxe);
    let mut fixed = [0; MANAGEMENT_HEADER_LEN + BEACON_FIXED_BODY_LEN];
    fixed[..2].copy_from_slice(&0x0080_u16.to_le_bytes());
    fixed[4..10].fill(0xff);
    fixed[10..16].copy_from_slice(&access_point);
    fixed[16..22].copy_from_slice(&access_point);
    fixed[22..24].copy_from_slice(&management_sequence.sequence_control().to_le_bytes());
    fixed[32..34].copy_from_slice(&beacon_interval_tu.to_le_bytes());
    let capabilities = profile.capabilities(security.link_protection());
    fixed[34..36].copy_from_slice(&capabilities.to_le_bytes());
    let dsss = [3, 1, channel.number()];
    let erp = [ERP_ELEMENT_ID, 1, protection.erp_information()];
    // Bitmap offset zero plus two octets covers AID 1..=15 without aliasing
    // AID 8..=15 onto the first byte. Bit zero of bitmap control remains the
    // independent DTIM multicast indication maintained by `stamp`.
    let tim = [5, 5, dtim_period - 1, dtim_period, 0, 0, 0];
    crate::encoding::FrameParts([
        fixed.as_slice(),
        &[0, ssid.as_bytes().len() as u8],
        ssid.as_bytes(),
        &[1, profile.legacy_rates.supported().len() as u8],
        profile.legacy_rates.supported(),
        if ghz2_4 { &dsss } else { &[] },
        &tim,
        if ghz2_4 { &erp } else { &[] },
        rsn,
        &[50, profile.legacy_rates.extended().len() as u8],
        profile.legacy_rates.extended(),
        &wmm_parameter_ie,
        &ht_capability,
        &ht_operation,
        rsnx,
    ])
    .write(output)
    .map_err(|required| ApBeaconBuildError::OutputTooSmall { required })
}

const ERP_ELEMENT_ID: u8 = 42;
const DSSS_PARAMETER_SET_ELEMENT_ID: u8 = 3;
const HT_OPERATION_ELEMENT_ID: u8 = 61;

/// Replace the ERP Information and HT Operation protection fields of one
/// beacon or probe-response template in place.
///
/// Every other byte, including the TIM that [`write_tim_partial_virtual_bitmap`]
/// resizes, is retained. A 2.4 GHz template (one with a DSSS Parameter Set)
/// needs both elements, a 5 GHz one the HT Operation; a template without
/// them is rejected unchanged.
pub fn update_bss_protection(
    frame: &mut [u8],
    protection: ApBssProtection,
) -> Result<(), ApBeaconProtectionError> {
    const FIXED_BEACON_LENGTH: usize = MANAGEMENT_HEADER_LEN + BEACON_FIXED_BODY_LEN;
    let mut erp = None;
    let mut ht_operation = None;
    let mut dsss = false;
    let mut offset = FIXED_BEACON_LENGTH;
    while offset + 2 <= frame.len() {
        let id = frame[offset];
        let length = usize::from(frame[offset + 1]);
        let value = offset + 2;
        if value + length > frame.len() {
            return Err(ApBeaconProtectionError::MalformedElement);
        }
        match id {
            ERP_ELEMENT_ID if length == 1 => erp = Some(value),
            DSSS_PARAMETER_SET_ELEMENT_ID => dsss = true,
            // Primary Channel, Information byte zero, then byte one.
            HT_OPERATION_ELEMENT_ID if length == 22 => ht_operation = Some(value + 2),
            _ => {}
        }
        offset = value + length;
    }
    let Some(ht_operation) = ht_operation else {
        return Err(ApBeaconProtectionError::MissingProtectionElement);
    };
    if dsss && erp.is_none() {
        return Err(ApBeaconProtectionError::MissingProtectionElement);
    }
    if let Some(erp) = erp {
        frame[erp] = protection.erp_information();
    }
    frame[ht_operation] = protection.ht.information_byte();
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApBeaconProtectionError {
    MalformedElement,
    MissingProtectionElement,
}

/// Locate the TIM element in one bounded AP beacon.
///
/// The strict profile qualifies management objects up to 1600 bytes. Expanding
/// the maximum element count keeps this parser finite without a data-dependent
/// control-flow cycle in either the submit or TX-done root.
#[allow(unused_assignments)]
pub fn dtim(bytes: &[u8]) -> Option<(usize, u8, u8)> {
    const FIXED_BEACON_LENGTH: usize = 24 + 8 + 2 + 2;

    if bytes.len() < FIXED_BEACON_LENGTH
        || u16::from_le_bytes([bytes[0], bytes[1]]) & 0x00fc != 0x0080
    {
        return None;
    }
    let mut offset = FIXED_BEACON_LENGTH;
    macro_rules! inspect_element {
        () => {{
            if offset + 2 > bytes.len() {
                return None;
            }
            let id = bytes[offset];
            let length = usize::from(bytes[offset + 1]);
            let end = offset + 2 + length;
            if end > bytes.len() {
                return None;
            }
            if id == 5 {
                if length < 4 {
                    return None;
                }
                let count = bytes[offset + 2];
                let period = bytes[offset + 3];
                return (period != 0 && count < period).then_some((offset, count, period));
            }
            offset = end;
        }};
    }

    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    inspect_element!();
    None
}

/// Replace the TIM Partial Virtual Bitmap while preserving every following
/// information element in the bounded beacon owner.
///
/// The returned length reflects canonical one- through 251-octet N1/N2
/// compression. Capacity and all existing TIM bounds are checked before the
/// frame is moved, so `None` leaves `storage` unchanged.
pub fn write_tim_partial_virtual_bitmap(
    storage: &mut [u8],
    frame_len: usize,
    partial: TimPartialVirtualBitmap<'_>,
) -> Option<usize> {
    let frame = storage.get(..frame_len)?;
    let (tim_offset, _, _) = dtim(frame)?;
    let current_body_len = usize::from(*frame.get(tim_offset + 1)?);
    if current_body_len < 4 || partial.octets.is_empty() {
        return None;
    }
    let current_end = tim_offset.checked_add(2 + current_body_len)?;
    if current_end > frame_len {
        return None;
    }
    let desired_body_len = 3_usize.checked_add(partial.octets.len())?;
    let desired_body_len_u8 = u8::try_from(desired_body_len).ok()?;
    let new_len = if desired_body_len >= current_body_len {
        frame_len.checked_add(desired_body_len - current_body_len)?
    } else {
        frame_len.checked_sub(current_body_len - desired_body_len)?
    };
    if new_len > storage.len() {
        return None;
    }

    let desired_end = tim_offset + 2 + desired_body_len;
    if desired_end != current_end {
        storage.copy_within(current_end..frame_len, desired_end);
    }

    storage[tim_offset + 1] = desired_body_len_u8;
    let group_indication = storage[tim_offset + 4] & 1;
    storage[tim_offset + 4] = group_indication | partial.bitmap_offset;
    storage[tim_offset + 5..desired_end].copy_from_slice(partial.octets);
    if new_len < frame_len {
        storage[new_len..frame_len].fill(0);
    }
    Some(new_len)
}

/// Replace the TSF, DTIM phase and group-traffic indication before HW submit.
pub fn stamp(bytes: &mut [u8], timestamp: u64, group_pending: bool) -> Option<(u8, u8)> {
    const FIXED_BEACON_LENGTH: usize = 24 + 8 + 2 + 2;
    if bytes.len() < FIXED_BEACON_LENGTH {
        return None;
    }
    bytes[24..32].copy_from_slice(&timestamp.to_le_bytes());

    let (tim_offset, _, period) = dtim(bytes)?;
    let interval_tu = u16::from_le_bytes([bytes[32], bytes[33]]);
    if interval_tu == 0 {
        return None;
    }
    let interval_us = u64::from(interval_tu) * 1_024;
    let phase = ((timestamp / interval_us) % u64::from(period)) as u8;
    let count = period - 1 - phase;
    bytes[tim_offset + 2] = count;

    // IEEE 802.11 TIM bitmap-control bit 0 announces buffered group traffic
    // only in a DTIM beacon. The queue itself remains Rust-owned and is
    // released by the corresponding TX-done edge.
    let bitmap_control = &mut bytes[tim_offset + 4];
    if count == 0 && group_pending {
        *bitmap_control |= 1;
    } else {
        *bitmap_control &= !1;
    }
    Some((count, period))
}

#[cfg(test)]
mod tests;
