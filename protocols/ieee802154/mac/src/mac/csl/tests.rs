//! Expectations follow `get_csl_phase` of ESP-IDF's OpenThread port and
//! OpenThread's `Frame::SetCslIe` and `otMacFrameGenerateCslIeTemplate`.

use std::vec::Vec;

use super::{CSL_IE_TEMPLATE, CSL_UNIT_MICROS, csl_phase, write_csl_ie};

/// The template is OpenThread's: descriptor 0x0d04 (length 4, element
/// 0x1a), zero content.
#[test]
fn the_template_is_a_zeroed_csl_ie() {
    assert_eq!(CSL_IE_TEMPLATE, [0x04, 0x0d, 0, 0, 0, 0]);
}

/// The phase counts ten-symbol units to the next sample time, plus one.
#[test]
fn the_phase_counts_to_the_next_sample_time() {
    let period = 100; // 16 ms
    let period_us = u32::from(period) * CSL_UNIT_MICROS;
    // At the sample time the next one is a whole period away... modulo the
    // period, zero units, plus one.
    assert_eq!(csl_phase(5_000, 5_000, period), Some(1));
    // 1600 us before the sample time: ten units.
    assert_eq!(csl_phase(5_000 - 1_600, 5_000, period), Some(11));
    // Just after the sample time: almost a period.
    assert_eq!(
        csl_phase(5_001, 5_000, period),
        Some(((period_us - 1) / CSL_UNIT_MICROS + 1) as u16)
    );
    // The clock and sample time wrap in 32 bits.
    assert_eq!(
        csl_phase(u32::MAX, 5_000 + period_us, period),
        csl_phase(u32::MAX % period_us, 5_000, period)
    );
    assert_eq!(csl_phase(1, 2, 0), None);
}

/// `[PHR, MAC...]` of a 2015 data frame with a CSL IE, then a termination
/// IE, a payload byte and the FCS.
fn with_csl_ie(secured: bool) -> Vec<u8> {
    let mut mac = std::vec![
        0x41 | if secured { 0x08 } else { 0 },
        0xaa, // 2015, IE present, short addresses
        0x07, // sequence
        0x34,
        0x12, // PAN
        0x02,
        0x00, // destination
        0x01,
        0x00, // source
    ];
    if secured {
        mac.extend_from_slice(&[0x0d, 0, 0, 0, 0, 1]); // ENC-MIC-32, mode 1
    }
    mac.extend_from_slice(&CSL_IE_TEMPLATE);
    mac.extend_from_slice(&[0x00, 0x3f]); // HT1
    mac.push(0x99);
    if secured {
        mac.extend_from_slice(&[0; 4]);
    }
    let mut image = std::vec![mac.len() as u8 + 2];
    image.extend_from_slice(&mac);
    image.extend_from_slice(&[0, 0]);
    image
}

/// The period and phase land in the CSL IE, after a security header when
/// there is one; frames without a CSL IE are left.
#[test]
fn period_and_phase_go_into_the_csl_ie() {
    for (secured, content) in [(false, 12), (true, 18)] {
        let mut image = with_csl_ie(secured);
        assert!(write_csl_ie(&mut image, 0x0203, 0x0405));
        assert_eq!(image[content..content + 4], [0x05, 0x04, 0x03, 0x02]);
    }

    // A 2006 frame has no header IEs.
    let mut old = with_csl_ie(false);
    old[2] = 0x98;
    let before = old.clone();
    assert!(!write_csl_ie(&mut old, 1, 1));
    assert_eq!(old, before);

    // A frame whose IEs end before a CSL IE.
    let mut other = with_csl_ie(false);
    other[10..16].copy_from_slice(&[0x00, 0x3f, 0x04, 0x0d, 0, 0]);
    assert!(!write_csl_ie(&mut other, 1, 1));
}
