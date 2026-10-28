//! Bounded, allocation-free AP beacon field ownership.
//!
//! Locate the fixed-size TIM element and derive its DTIM phase from the
//! supplied timestamp and the beacon's advertised interval. The caller owns
//! the clock relation; this codec does not read a backend or executor clock.

use crate::frame::FrameControlFields;

use crate::{
    ap::profile::Advertisement,
    channel::{Band, Channel},
    ht::{ht_capability_ie, ht_operation_ie},
    protection::ApBssProtection,
    security::BssSecurityElements,
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
        if self.octets.iter().all(|octet| *octet == 0) {
            // The standard's canonical empty bitmap is one zero octet.
            return TimPartialVirtualBitmap {
                bitmap_offset: 0,
                octets: &[0],
            };
        }
        let mut octets = self.octets.as_slice();
        let mut bitmap_offset = 0_u8;
        // N1 is even. Each removed pair is part of the fixed <=251-octet
        // capacity, so the geometry counter never reaches u8::MAX.
        while let [0, 0, rest @ ..] = octets {
            octets = rest;
            bitmap_offset = bitmap_offset.saturating_add(2);
        }
        while let [rest @ .., 0] = octets {
            octets = rest;
        }
        TimPartialVirtualBitmap {
            bitmap_offset,
            octets,
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
    security: BssSecurityElements,
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
            .any(|rate| {
                matches!(
                    crate::phy::SupportedRate::from_bytes([*rate]).rate(),
                    2 | 4 | 11 | 22
                )
            })
    {
        return Err(ApBeaconBuildError::DsssRateIn5Ghz);
    }
    if dtim_period == 0 {
        return Err(ApBeaconBuildError::InvalidDtimPeriod);
    }

    let wmm_parameter_ie = profile.wmm.element();
    let ht_capability = ht_capability_ie(profile.ht, channel);
    let ht_operation = ht_operation_ie(channel, protection.ht);
    let (rsn, rsnx) = (security.rsne, security.rsnxe);
    let mut fixed = [0; MANAGEMENT_HEADER_LEN + BEACON_FIXED_BODY_LEN];
    crate::encoding::write_array::<0, _, _>(&mut fixed, &0x0080_u16.to_le_bytes());
    fixed[4..10].fill(0xff);
    crate::encoding::write_array::<10, _, _>(&mut fixed, &access_point);
    crate::encoding::write_array::<16, _, _>(&mut fixed, &access_point);
    crate::encoding::write_array::<22, _, _>(
        &mut fixed,
        &management_sequence.sequence_control().to_le_bytes(),
    );
    crate::encoding::write_array::<32, _, _>(&mut fixed, &beacon_interval_tu.to_le_bytes());
    let capabilities = profile.capabilities(security.link_protection());
    crate::encoding::write_array::<34, _, _>(&mut fixed, &capabilities.to_le_bytes());
    let dsss = [3, 1, channel.number()];
    let erp = [ERP_ELEMENT_ID, 1, protection.erp_information()];
    // Bitmap offset zero plus two octets covers AID 1..=15 without aliasing
    // AID 8..=15 onto the first byte. Bit zero of bitmap control remains the
    // independent DTIM multicast indication maintained by `stamp`.
    let tim = [
        5,
        5,
        dtim_period
            .checked_sub(1)
            .ok_or(ApBeaconBuildError::InvalidDtimPeriod)?,
        dtim_period,
        0,
        0,
        0,
    ];
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
    let mut reader = oer_wire::Reader::new(frame);
    reader
        .array::<FIXED_BEACON_LENGTH>()
        .map_err(|_| ApBeaconProtectionError::MalformedElement)?;
    while !reader.is_empty() {
        let [id, length] = reader
            .read::<2>()
            .map_err(|_| ApBeaconProtectionError::MalformedElement)?;
        let value = frame
            .len()
            .checked_sub(reader.remaining().len())
            .ok_or(ApBeaconProtectionError::MalformedElement)?;
        reader
            .take(usize::from(length))
            .map_err(|_| ApBeaconProtectionError::MalformedElement)?;
        match id {
            ERP_ELEMENT_ID if length == 1 => erp = Some(value),
            DSSS_PARAMETER_SET_ELEMENT_ID => dsss = true,
            HT_OPERATION_ELEMENT_ID if length == 22 => {
                ht_operation = Some(
                    value
                        .checked_add(2)
                        .ok_or(ApBeaconProtectionError::MalformedElement)?,
                )
            }
            _ => {}
        }
    }
    let Some(ht_operation) = ht_operation else {
        return Err(ApBeaconProtectionError::MissingProtectionElement);
    };
    if dsss && erp.is_none() {
        return Err(ApBeaconProtectionError::MissingProtectionElement);
    }
    if let Some(erp) = erp {
        *frame
            .get_mut(erp)
            .ok_or(ApBeaconProtectionError::MalformedElement)? = protection.erp_information();
    }
    *frame
        .get_mut(ht_operation)
        .ok_or(ApBeaconProtectionError::MalformedElement)? = protection.ht.information_byte();
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApBeaconProtectionError {
    MalformedElement,
    MissingProtectionElement,
}

/// Why an edit of a written beacon template failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BeaconTemplateError {
    /// The template's elements do not parse, or it carries no TIM.
    Malformed,
    /// The template's storage cannot hold the edit.
    Full,
}

/// Insert `elements` right after the TIM element of the `len`-byte beacon
/// template in `template`, moving the later elements up; returns the new
/// length.
///
/// SOURCE: IEEE Std 802.11-2020, 9.3.3.2, Table 9-34 (Beacon frame body:
/// the Channel Switch Announcement and later elements follow the TIM).
pub fn insert_after_tim(
    template: &mut [u8],
    len: usize,
    elements: &[u8],
) -> Result<usize, BeaconTemplateError> {
    let (tim, _, _) = dtim(template.get(..len).ok_or(BeaconTemplateError::Malformed)?)
        .ok_or(BeaconTemplateError::Malformed)?;
    let &[_, tim_length] = template
        .get(tim..)
        .and_then(|bytes| bytes.first_chunk::<2>())
        .ok_or(BeaconTemplateError::Malformed)?;
    let at = tim
        .checked_add(2)
        .and_then(|offset| offset.checked_add(usize::from(tim_length)))
        .ok_or(BeaconTemplateError::Malformed)?;
    let end = len
        .checked_add(elements.len())
        .filter(|end| *end <= template.len())
        .ok_or(BeaconTemplateError::Full)?;
    if at > len {
        return Err(BeaconTemplateError::Malformed);
    }
    for source in (at..len).rev() {
        let destination = source
            .checked_add(elements.len())
            .ok_or(BeaconTemplateError::Full)?;
        let byte = *template.get(source).ok_or(BeaconTemplateError::Malformed)?;
        *template
            .get_mut(destination)
            .ok_or(BeaconTemplateError::Full)? = byte;
    }
    for (destination, byte) in template.iter_mut().skip(at).zip(elements) {
        *destination = *byte;
    }
    Ok(end)
}

/// Remove every element whose ID is one of `ids` from the element list of
/// the `len`-byte beacon template, zeroing the freed tail; returns the new
/// length.
pub fn remove_elements(
    template: &mut [u8],
    len: usize,
    ids: &[u8],
) -> Result<usize, BeaconTemplateError> {
    const FIXED_BEACON_LENGTH: usize = MANAGEMENT_HEADER_LEN + BEACON_FIXED_BODY_LEN;
    let mut len = len;
    let mut offset = FIXED_BEACON_LENGTH;
    while let Some([id, length]) = template
        .get(offset..len)
        .and_then(|bytes| bytes.first_chunk::<2>())
        .copied()
    {
        let length = usize::from(length)
            .checked_add(2)
            .ok_or(BeaconTemplateError::Malformed)?;
        let end = offset
            .checked_add(length)
            .filter(|end| *end <= len)
            .ok_or(BeaconTemplateError::Malformed)?;
        if ids.contains(&id) {
            for (destination, source) in (offset..len).zip(end..len) {
                let byte = *template.get(source).ok_or(BeaconTemplateError::Malformed)?;
                *template
                    .get_mut(destination)
                    .ok_or(BeaconTemplateError::Malformed)? = byte;
            }
            let previous = len;
            len = len
                .checked_sub(length)
                .ok_or(BeaconTemplateError::Malformed)?;
            template
                .get_mut(len..previous)
                .ok_or(BeaconTemplateError::Malformed)?
                .fill(0);
        } else {
            offset = end;
        }
    }
    Ok(len)
}

/// Write `count` into the Channel Switch Count of the `len`-byte beacon
/// template's Channel Switch Announcement or Extended Channel Switch
/// Announcement; `None` when the template carries neither.
///
/// SOURCE: IEEE Std 802.11-2020, 9.4.2.18 (Channel Switch Announcement
/// element: Mode, New Channel Number, Count) and 9.4.2.52 (Extended Channel
/// Switch Announcement element: Mode, New Operating Class, New Channel
/// Number, Count).
pub fn write_channel_switch_count(template: &mut [u8], len: usize, count: u8) -> Option<()> {
    const FIXED_BEACON_LENGTH: usize = MANAGEMENT_HEADER_LEN + BEACON_FIXED_BODY_LEN;
    let mut remaining = template.get_mut(FIXED_BEACON_LENGTH..len)?;
    while let Some(([id, length], rest)) = remaining.split_first_chunk_mut::<2>() {
        let (body, tail) = rest.split_at_mut_checked(usize::from(*length))?;
        if matches!(
            (*id, *length),
            (crate::channel_switch::CHANNEL_SWITCH_ELEMENT_ID, 3)
                | (crate::channel_switch::EXTENDED_CHANNEL_SWITCH_ELEMENT_ID, 4)
        ) {
            *body.last_mut()? = count;
            return Some(());
        }
        remaining = tail;
    }
    None
}

/// Locate the TIM element in one bounded AP beacon.
///
/// The strict profile qualifies management objects up to 1600 bytes. Expanding
/// the maximum element count keeps this parser finite without a data-dependent
/// control-flow cycle in either the submit or TX-done root.
#[allow(unused_assignments)]
pub fn dtim(bytes: &[u8]) -> Option<(usize, u8, u8)> {
    const FIXED_BEACON_LENGTH: usize = 24 + 8 + 2 + 2;

    let mut reader = oer_wire::Reader::new(bytes);
    let fixed = reader.array::<FIXED_BEACON_LENGTH>().ok()?;
    if FrameControlFields::from_bytes([fixed[0], fixed[1]]).type_and_subtype() != 32 {
        return None;
    }
    let mut offset = FIXED_BEACON_LENGTH;
    macro_rules! inspect_element {
        () => {{
            let mut reader = oer_wire::Reader::new(bytes.get(offset..)?);
            let [id, length] = reader.read::<2>().ok()?;
            let element = reader.take(usize::from(length)).ok()?;
            if id == 5 {
                let [count, period, _, _] = oer_wire::Reader::new(element).read::<4>().ok()?;
                return (period != 0 && count < period).then_some((offset, count, period));
            }
            offset = offset.checked_add(2)?.checked_add(usize::from(length))?;
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
    let current_body_len = usize::from(*frame.get(tim_offset.checked_add(1)?)?);
    if current_body_len < 4 || partial.octets.is_empty() {
        return None;
    }
    let current_end = tim_offset.checked_add(2_usize.checked_add(current_body_len)?)?;
    if current_end > frame_len {
        return None;
    }
    let desired_body_len = 3_usize.checked_add(partial.octets.len())?;
    let desired_body_len_u8 = u8::try_from(desired_body_len).ok()?;
    let new_len = if desired_body_len >= current_body_len {
        frame_len.checked_add(desired_body_len.checked_sub(current_body_len)?)?
    } else {
        frame_len.checked_sub(current_body_len.checked_sub(desired_body_len)?)?
    };
    if new_len > storage.len() {
        return None;
    }

    let desired_end = tim_offset.checked_add(2)?.checked_add(desired_body_len)?;
    if desired_end != current_end {
        crate::encoding::move_within(storage, current_end..frame_len, desired_end).ok()?;
    }

    *storage.get_mut(tim_offset.checked_add(1)?)? = desired_body_len_u8;
    let group_indication =
        TimBitmapControl::from_bytes([*storage.get(tim_offset.checked_add(4)?)?]).group();
    *storage.get_mut(tim_offset.checked_add(4)?)? =
        TimBitmapControl::from_bytes([partial.bitmap_offset])
            .with_group(group_indication)
            .to_bytes()[0];
    oer_wire::Writer::new(storage.get_mut(tim_offset.checked_add(5)?..desired_end)?)
        .write(partial.octets)
        .ok()?;
    if new_len < frame_len {
        storage.get_mut(new_len..frame_len)?.fill(0);
    }
    Some(new_len)
}

/// Replace the TSF, DTIM phase and group-traffic indication before HW submit.
pub fn stamp(bytes: &mut [u8], timestamp: u64, group_pending: bool) -> Option<(u8, u8)> {
    const FIXED_BEACON_LENGTH: usize = 24 + 8 + 2 + 2;
    if bytes.len() < FIXED_BEACON_LENGTH {
        return None;
    }
    oer_wire::Writer::new(bytes.get_mut(24..32)?)
        .write(&timestamp.to_le_bytes())
        .ok()?;

    let (tim_offset, _, period) = dtim(bytes)?;
    let interval_tu = oer_wire::Reader::new(bytes.get(32..)?).u16_le().ok()?;
    if interval_tu == 0 {
        return None;
    }
    let interval_us = u64::from(interval_tu).checked_mul(1_024)?;
    let phase = timestamp
        .checked_div(interval_us)?
        .checked_rem(u64::from(period))? as u8;
    let count = period.checked_sub(1)?.checked_sub(phase)?;
    *bytes.get_mut(tim_offset.checked_add(2)?)? = count;

    // IEEE 802.11 TIM bitmap-control bit 0 announces buffered group traffic
    // only in a DTIM beacon. The queue itself remains Rust-owned and is
    // released by the corresponding TX-done edge.
    let bitmap_control = bytes.get_mut(tim_offset.checked_add(4)?)?;
    *bitmap_control = TimBitmapControl::from_bytes([*bitmap_control])
        .with_group(count == 0 && group_pending)
        .to_bytes()[0];
    Some((count, period))
}

oer_wire::bitfield! {
    /// SOURCE: IEEE Std 802.11-2020, 9.4.2.5 (TIM element).
    pub struct TimBitmapControl[1] (little) {
        pub group, with_group: bool = 0..1;
        pub offset, with_offset: u8 = 1..8;
    }
}

#[cfg(test)]
mod tests;
