#![no_std]
#![forbid(unsafe_code)]

//! Portable Trouble GATT application for the open ESP32-S31 Bluetooth
//! Controller.
//!
//! [`gatt`] serves one writable value over a plaintext connection; with the
//! `secure` feature, [`security`] adds LE Secure Connections numeric
//! comparison, bonding and encrypted access. The library owns no platform,
//! executor or controller: the Bluetooth HIL firmware composes it over its own
//! `trouble-host` stack.

pub mod gatt;

#[cfg(feature = "secure")]
pub mod security;

/// Local name published by the Trouble GATT application profile.
pub const TROUBLE_GATT_DEVICE_NAME: &[u8] = b"open-radio-gatt";
/// Vendor-specific primary service used by the writable smoke value.
pub const TROUBLE_GATT_SERVICE_UUID: u16 = 0xfff0;
/// One-byte read/write value exposed by the Trouble GATT service.
pub const TROUBLE_GATT_VALUE_UUID: u16 = 0xfff1;

/// Encode the complete legacy advertising and scan-response payloads used by
/// the Trouble GATT application.
///
/// Both destination buffers have the exact HCI legacy capacity, so the fixed
/// profile cannot be truncated or silently omit its local name.
pub fn encode_trouble_gatt_advertising(
    advertising: &mut [u8; 31],
    scan_response: &mut [u8; 31],
) -> (usize, usize) {
    advertising[..3].copy_from_slice(&[2, 0x01, 0x06]);
    let name_field_len = TROUBLE_GATT_DEVICE_NAME.len() + 1;
    scan_response[0] = name_field_len as u8;
    scan_response[1] = 0x09;
    scan_response[2..2 + TROUBLE_GATT_DEVICE_NAME.len()].copy_from_slice(TROUBLE_GATT_DEVICE_NAME);
    (3, 2 + TROUBLE_GATT_DEVICE_NAME.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trouble_gatt_profile_has_complete_bounded_legacy_payloads() {
        let mut advertising = [0_u8; 31];
        let mut scan_response = [0_u8; 31];
        let (advertising_len, scan_response_len) =
            encode_trouble_gatt_advertising(&mut advertising, &mut scan_response);

        assert_eq!(&advertising[..advertising_len], &[2, 0x01, 0x06]);
        assert_eq!(
            scan_response[0] as usize,
            TROUBLE_GATT_DEVICE_NAME.len() + 1
        );
        assert_eq!(scan_response[1], 0x09);
        assert_eq!(
            &scan_response[2..scan_response_len],
            TROUBLE_GATT_DEVICE_NAME
        );
        assert_eq!(TROUBLE_GATT_SERVICE_UUID, 0xfff0);
        assert_eq!(TROUBLE_GATT_VALUE_UUID, 0xfff1);
    }
}
