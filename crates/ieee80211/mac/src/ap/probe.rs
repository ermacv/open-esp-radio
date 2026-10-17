//! Probe discovery reuses the AP's current beacon advertisement, without TIM.

use crate::sequence::SequenceNumber;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResponseError {
    InvalidAdvertisement,
    OutputTooSmall { required: usize },
}

/// Validate the complete IE stream and require exactly one bounded SSID.
pub(super) fn ssid(mut elements: &[u8]) -> Option<&[u8]> {
    let mut found = None;
    while !elements.is_empty() {
        let length = usize::from(*elements.get(1)?);
        let value = elements.get(2..2 + length)?;
        if elements[0] == 0 {
            if found.is_some() || length > 32 {
                return None;
            }
            found = Some(value);
        }
        elements = &elements[2 + length..];
    }
    found
}

pub fn matches_ssid(beacon: &[u8], requested: &[u8]) -> bool {
    beacon
        .get(36..)
        .and_then(ssid)
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
