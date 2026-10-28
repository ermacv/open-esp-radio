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
    let invalid = |_| ResponseError::InvalidAdvertisement;
    let mut reader = oer_wire::Reader::new(beacon);
    let fixed = reader.array::<36>().map_err(invalid)?;
    let elements = reader.remaining();
    if fixed[..2] != [0x80, 0] || ssid(elements).is_none() {
        return Err(ResponseError::InvalidAdvertisement);
    }
    let mut required = 36_usize;
    while !reader.is_empty() {
        let [id, length] = reader.read::<2>().map_err(invalid)?;
        reader.take(usize::from(length)).map_err(invalid)?;
        if id != 5 {
            required = required.checked_add(2 + usize::from(length)).ok_or(
                ResponseError::OutputTooSmall {
                    required: usize::MAX,
                },
            )?;
        }
    }
    if output.len() < required {
        return Err(ResponseError::OutputTooSmall { required });
    }
    let mut fixed = *fixed;
    crate::encoding::write_array::<0, _, _>(&mut fixed, &[0x50, 0]);
    fixed[2..4].fill(0);
    crate::encoding::write_array::<4, _, _>(&mut fixed, &peer);
    crate::encoding::write_array::<22, _, _>(
        &mut fixed,
        &sequence.sequence_control().to_le_bytes(),
    );
    crate::encoding::write_array::<24, _, _>(&mut fixed, &timestamp_micros.to_le_bytes());
    let mut writer = oer_wire::Writer::new(output);
    let too_small = |_| ResponseError::OutputTooSmall { required };
    writer.write(&fixed).map_err(too_small)?;
    let mut reader = oer_wire::Reader::new(elements);
    while !reader.is_empty() {
        let header @ [id, length] = reader.read::<2>().map_err(invalid)?;
        let value = reader.take(usize::from(length)).map_err(invalid)?;
        if id != 5 {
            writer.write(&header).map_err(too_small)?;
            writer.write(value).map_err(too_small)?;
        }
    }
    Ok(writer.written())
}

#[cfg(test)]
mod tests;
