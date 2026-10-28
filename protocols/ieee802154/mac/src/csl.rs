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

/// Microseconds per ten-symbol CSL unit (`OT_US_PER_TEN_SYMBOLS`).
pub const CSL_UNIT_MICROS: u32 = 160;

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

#[cfg(test)]
mod tests;
