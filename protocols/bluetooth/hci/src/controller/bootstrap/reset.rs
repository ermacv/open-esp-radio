//! Event masks a Reset restores.

use super::*;
use crate::HciBuildError;

/// The event mask after Reset, `0x0000_1FFF_FFFF_FFFF` (Core Vol 4 Part E
/// 7.3.1): every event up to bit 44, without LE Meta.
///
/// # Errors
///
/// The standard codec rejects the reset-default encoding.
pub fn default_event_mask() -> Result<EventMask, HciBuildError> {
    decode_reset_mask(&0x0000_1fff_ffff_ffff_u64.to_le_bytes())
}

/// The LE event mask after Reset, `0x1F` (Core Vol 4 Part E 7.8.1):
/// connection complete, advertising report, connection update complete,
/// remote features complete and LTK request.
///
/// # Errors
///
/// The standard codec rejects the reset-default encoding.
pub fn default_le_event_mask() -> Result<LeEventMask, HciBuildError> {
    decode_reset_mask(&0x1f_u64.to_le_bytes())
}

fn decode_reset_mask<T: for<'a> FromHciBytes<'a>>(bytes: &[u8]) -> Result<T, HciBuildError> {
    T::from_hci_bytes_complete(bytes).map_err(HciBuildError::InvalidResetEventMask)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reset_mask_decode_reports_invalid_encoding_and_preserves_every_default_bit() {
        assert_eq!(
            decode_reset_mask::<EventMask>(&[0; 7]),
            Err(HciBuildError::InvalidResetEventMask(
                bt_hci::FromHciBytesError::InvalidSize
            ))
        );
        assert_eq!(
            decode_reset_mask::<LeEventMask>(&[0; 9]),
            Err(HciBuildError::InvalidResetEventMask(
                bt_hci::FromHciBytesError::InvalidSize
            ))
        );
        assert_eq!(
            default_event_mask().unwrap().into_inner(),
            0x0000_1fff_ffff_ffff_u64.to_le_bytes()
        );
        assert_eq!(
            default_le_event_mask().unwrap().into_inner(),
            0x1f_u64.to_le_bytes()
        );
    }
}
