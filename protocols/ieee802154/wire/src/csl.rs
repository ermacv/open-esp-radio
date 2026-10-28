//! Coordinated Sampled Listening (IEEE 802.15.4-2015, Thread 1.2) as a
//! radio that claims `OT_RADIO_CAPS_RECEIVE_TIMING` serves it for
//! OpenThread: the CSL IE of the enhanced ACKs it generates and the period
//! and phase it writes into every CSL IE it sends (`esp_openthread_radio.c`
//! at ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`,
//! `mac_frame.cpp` of OpenThread).
//!
//! Ported from ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/openthread/src/port/esp_openthread_radio.c` (Copyright
//! Espressif Systems (Shanghai) Co Ltd, Apache-2.0) and modified by Vladislav
//! Ermachenkov; see `NOTICE`.
//!

use crate::PhrFrame;

/// The CSL header IE element identifier (`CslIe::kHeaderIeId`).
pub const CSL_IE_ID: u8 = 0x1a;

/// Bytes of the CSL IE content: phase, then period, both little endian.
const CSL_IE_CONTENT: u16 = 4;

/// A CSL header IE with zero phase and period, as
/// `otMacFrameGenerateCslIeTemplate` writes it for an enhanced ACK: the
/// descriptor (length 4, element 0x1a, header type), then the content the
/// radio fills when the ACK starts on the air.
pub const CSL_IE_TEMPLATE: [u8; 6] = {
    let descriptor = CSL_IE_CONTENT | (CSL_IE_ID as u16) << 7;
    let bytes = descriptor.to_le_bytes();
    [bytes[0], bytes[1], 0, 0, 0, 0]
};

/// Write `period` and `phase` into the CSL IE of the `[PHR, MAC...]` image
/// of a frame, as `otMacFrameSetCslIe` does. Returns `false`, leaving the
/// image, for a frame without a CSL IE.
pub fn write_csl_ie(image: &mut [u8], period: u16, phase: u16) -> bool {
    let Some(offset) = PhrFrame::new(image).header_ie(CSL_IE_ID) else {
        return false;
    };
    let Some((_, content)) = image.split_at_mut_checked(usize::from(offset)) else {
        return false;
    };
    let Some((_, content)) = content.split_first_chunk_mut::<2>() else {
        return false;
    };
    let Some((target, _)) = content.split_first_chunk_mut::<4>() else {
        return false;
    };
    let [phase_low, phase_high] = phase.to_le_bytes();
    let [period_low, period_high] = period.to_le_bytes();
    *target = [phase_low, phase_high, period_low, period_high];
    true
}

#[cfg(test)]
mod tests;
