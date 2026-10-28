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
    // u16 periods fit in u32 microseconds; each remainder is below that
    // period, so the intermediate sum fits and the phase is <= period.
    let Some(period_us) = (period as u32).checked_mul(CSL_UNIT_MICROS) else {
        return None;
    };
    let Some(now_phase) = now.checked_rem(period_us) else {
        return None;
    };
    let Some(sample_phase) = sample_time.checked_rem(period_us) else {
        return None;
    };
    let Some(diff) = period_us.checked_sub(now_phase) else {
        return None;
    };
    let Some(diff) = diff.checked_add(sample_phase) else {
        return None;
    };
    let Some(diff) = diff.checked_rem(period_us) else {
        return None;
    };
    let Some(phase) = diff.checked_div(CSL_UNIT_MICROS) else {
        return None;
    };
    match phase.checked_add(1) {
        Some(phase) => Some(phase as u16),
        None => None,
    }
}

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
