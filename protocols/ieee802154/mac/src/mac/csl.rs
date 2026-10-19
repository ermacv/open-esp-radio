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

use crate::mac::header::PhrFrame;

/// The CSL header IE element identifier (`CslIe::kHeaderIeId`).
pub const CSL_IE_ID: u8 = 0x1a;

/// Microseconds of the CSL period and phase unit: ten symbols
/// (`OT_US_PER_TEN_SYMBOLS`).
pub const CSL_UNIT_MICROS: u32 = 160;

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

/// The phase to the next CSL sample time, in units of ten symbols, as the
/// port's `get_csl_phase` computes it from the radio clock's low 32 bits:
/// the time to the sample instant modulo the period, rounded down, plus
/// one. `None` without a period.
pub const fn csl_phase(now: u32, sample_time: u32, period: u16) -> Option<u16> {
    if period == 0 {
        return None;
    }
    let period_us = period as u32 * CSL_UNIT_MICROS;
    let diff = (period_us - now % period_us + sample_time % period_us) % period_us;
    Some((diff / CSL_UNIT_MICROS + 1) as u16)
}

/// Write `period` and `phase` into the CSL IE of the `[PHR, MAC...]` image
/// of a frame, as `otMacFrameSetCslIe` does. Returns `false`, leaving the
/// image, for a frame without a CSL IE.
pub fn write_csl_ie(image: &mut [u8], period: u16, phase: u16) -> bool {
    let Some(offset) = PhrFrame::new(image).header_ie(CSL_IE_ID) else {
        return false;
    };
    let content = usize::from(offset) + 2;
    let Some(target) = image.get_mut(content..content + usize::from(CSL_IE_CONTENT)) else {
        return false;
    };
    target[..2].copy_from_slice(&phase.to_le_bytes());
    target[2..].copy_from_slice(&period.to_le_bytes());
    true
}

#[cfg(test)]
mod tests;
