//! Auxiliary security header byte encoding from a supplied frame counter
//! and key identifier. This module neither selects keys nor allocates counters.
//!
//! Ported from ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/openthread/src/port/esp_openthread_radio.c` (Copyright
//! Espressif Systems (Shanghai) Co Ltd, Apache-2.0) and modified by Vladislav
//! Ermachenkov; see `NOTICE`.
//!

use crate::{KeyIdMode, PhrFrame};

const SECURITY_CONTROL_SIZE: usize = 1;
const FRAME_COUNTER_SIZE: usize = 4;
const KEY_ID_MODE_MASK: u8 = 0x18;
const KEY_ID_MODE_1: u8 = 0x08;

/// Write the frame counter and, in key identifier mode 1, the key index
/// into the auxiliary security header of a secured `[PHR, MAC...]` image.
pub fn write_security_header(
    image: &mut [u8],
    frame_counter: Option<u32>,
    key_id: Option<u8>,
) -> Option<KeyIdMode> {
    let frame = PhrFrame::new(image);
    if !frame.security_enabled() {
        return None;
    }
    let offset = usize::from(frame.security_header_offset()?);
    let (_, header) = image.split_at_mut_checked(offset)?;
    let (control, rest) = header.split_first_chunk_mut::<SECURITY_CONTROL_SIZE>()?;
    let [control] = *control;
    let mode_1 = control & KEY_ID_MODE_MASK == KEY_ID_MODE_1;
    let (counter, rest) = rest.split_first_chunk_mut::<FRAME_COUNTER_SIZE>()?;
    // Admit every field before writing either, preserving refusal atomicity.
    let index = if mode_1 {
        Some(rest.first_mut()?)
    } else {
        None
    };
    if let Some(value) = frame_counter {
        *counter = value.to_le_bytes();
    }
    if let Some(index) = index {
        if let Some(key_id) = key_id {
            *index = key_id;
        }
        Some(KeyIdMode::Index)
    } else {
        Some(match control & KEY_ID_MODE_MASK {
            0x00 => KeyIdMode::Implicit,
            0x10 => KeyIdMode::Source4,
            _ => KeyIdMode::Source8,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn security_fields_are_admitted_together_before_any_write() {
        let original = [
            22, 0x69, 0x98, 7, 0x34, 0x12, 2, 0, 1, 0, 0x0d, 0, 0, 0, 0, 0, 0xaa, 0, 0, 0, 0,
        ];
        let mut image = original;
        assert_eq!(
            write_security_header(&mut image, Some(0x0102_0304), Some(3)),
            Some(KeyIdMode::Index)
        );
        assert_eq!(&image[11..16], &[4, 3, 2, 1, 3]);
        for len in 0..16 {
            let mut image = original;
            assert_eq!(
                write_security_header(&mut image[..len], Some(7), Some(8)),
                None
            );
            assert_eq!(image, original);
        }
        let mut plain = original;
        plain[1] &= !0x08;
        let before = plain;
        assert_eq!(write_security_header(&mut plain, Some(7), Some(8)), None);
        assert_eq!(plain, before);
    }
}
