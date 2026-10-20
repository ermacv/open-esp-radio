//! Probe discovery reuses the AP's current beacon advertisement, without TIM.
//!
//! Received probe requests reach `ssid` and [`matches_ssid`], which deny
//! every panicking construct; [`write_response`] builds a frame from the AP's
//! own advertisement and takes no received bytes.

use crate::sequence::SequenceNumber;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponseError {
    InvalidAdvertisement,
    OutputTooSmall { required: usize },
}

/// Validate the complete IE stream and require exactly one bounded SSID.
///
/// A received probe request's elements reach this function; it does not
/// panic on them.
#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
pub(super) fn ssid(mut elements: &[u8]) -> Option<&[u8]> {
    let mut found = None;
    while !elements.is_empty() {
        let mut reader = oer_wire::Reader::new(elements);
        let &[id, length] = reader.array::<2>().ok()?;
        let rest = reader.remaining();
        let mut reader = oer_wire::Reader::new(rest);
        let value = reader.take(usize::from(length)).ok()?;
        let rest = reader.remaining();
        if id == 0 {
            if found.is_some() || value.len() > 32 {
                return None;
            }
            found = Some(value);
        }
        elements = rest;
    }
    found
}

/// Whether a probe request's `requested` SSID, received bytes, selects the
/// advertisement `beacon`; it does not panic on either.
#[deny(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::arithmetic_side_effects
)]
pub fn matches_ssid(beacon: &[u8], requested: &[u8]) -> bool {
    let mut reader = oer_wire::Reader::new(beacon);
    if reader.array::<36>().is_err() {
        return false;
    }
    ssid(reader.remaining())
        .is_some_and(|advertised| requested.is_empty() || requested == advertised)
}

/// Copy the current advertisement directly to the response owner. The beacon
/// owner and its TIM/TBTT cursor are untouched. TSF uses the same clock as beacons.
pub fn write_response(
    beacon: &[u8],
    peer: [u8; 6],
    sequence: SequenceNumber,
    timestamp_micros: u64,
    output: &mut [u8],
) -> Result<usize, ResponseError> {
    let elements = beacon
        .get(36..)
        .ok_or(ResponseError::InvalidAdvertisement)?;
    if beacon[..2] != [0x80, 0] || ssid(elements).is_none() {
        return Err(ResponseError::InvalidAdvertisement);
    }
    let mut remaining = elements;
    let mut required = 36;
    while !remaining.is_empty() {
        let length = 2 + usize::from(remaining[1]);
        if remaining[0] != 5 {
            required += length;
        }
        remaining = &remaining[length..];
    }
    if output.len() < required {
        return Err(ResponseError::OutputTooSmall { required });
    }
    output[..36].copy_from_slice(&beacon[..36]);
    output[..2].copy_from_slice(&[0x50, 0]);
    output[2..4].fill(0);
    output[4..10].copy_from_slice(&peer);
    output[22..24].copy_from_slice(&sequence.sequence_control().to_le_bytes());
    output[24..32].copy_from_slice(&timestamp_micros.to_le_bytes());
    let mut offset = 36;
    remaining = elements;
    while !remaining.is_empty() {
        let length = 2 + usize::from(remaining[1]);
        if remaining[0] != 5 {
            output[offset..offset + length].copy_from_slice(&remaining[..length]);
            offset += length;
        }
        remaining = &remaining[length..];
    }
    Ok(offset)
}

#[cfg(test)]
mod tests;
