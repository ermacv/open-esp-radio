//! Security offload snapshots supplied to a backend and the header fields
//! it reports. Key selection and counter allocation belong to the MAC core.
//!
//! Ported from ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/openthread/src/port/esp_openthread_radio.c` (Copyright
//! Espressif Systems (Shanghai) Co Ltd, Apache-2.0) and modified by Vladislav
//! Ermachenkov; see `NOTICE`.
//!

use crate::AppliedSecurity;
use oer_ieee802154_wire::KeyIdMode;

/// The frame counter, key identifier and key the port gives one transmit
/// attempt of a secured frame (`otPlatRadioTransmit`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransmitSecurity {
    /// The frame counter written into the frame; `None` keeps the frame's.
    pub frame_counter: Option<u32>,
    /// The key index written into a key identifier mode 1 frame; `None`
    /// keeps the frame's.
    pub key_id: Option<u8>,
    /// The key the frame is secured with.
    pub key: [u8; 16],
}

impl TransmitSecurity {
    /// Write the frame counter and, in key identifier mode 1, the key index
    /// into the `[PHR, MAC...]` image of a secured frame, as
    /// `otMacFrameSetFrameCounter` and `otMacFrameSetKeyId` do. Returns the
    /// frame's key identifier mode, or `None` when the image has no
    /// auxiliary security header to write.
    pub fn apply(&self, image: &mut [u8]) -> Option<KeyIdMode> {
        oer_ieee802154_wire::security::write_security_header(image, self.frame_counter, self.key_id)
    }

    /// The fields [`Self::apply`] wrote into a frame of `mode`: none when
    /// the attempt kept the frame's counter.
    pub fn applied(&self, mode: KeyIdMode) -> Option<AppliedSecurity> {
        Some(AppliedSecurity {
            frame_counter: self.frame_counter?,
            key_id: match mode {
                KeyIdMode::Index => self.key_id,
                _ => None,
            },
        })
    }
}

impl AppliedSecurity {
    /// Write these fields into the `[PHR, MAC...]` image of the same secured
    /// frame, as the radio wrote them into its copy. Returns `false` when
    /// the image has no auxiliary security header to write.
    pub fn write(&self, image: &mut [u8]) -> bool {
        oer_ieee802154_wire::security::write_security_header(
            image,
            Some(self.frame_counter),
            self.key_id,
        )
        .is_some()
    }
}
