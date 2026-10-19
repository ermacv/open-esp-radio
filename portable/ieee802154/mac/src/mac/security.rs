//! MAC security state of ESP-IDF's OpenThread port
//! (`esp_openthread_radio.c` at ESP-IDF
//! `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`), which claims
//! `OT_RADIO_CAPS_TRANSMIT_SEC`: the radio, not the stack, gives each
//! secured transmission its frame counter and key.
//!
//! Ported from ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/openthread/src/port/esp_openthread_radio.c` (Copyright
//! Espressif Systems (Shanghai) Co Ltd, Apache-2.0) and modified by Vladislav
//! Ermachenkov; see `NOTICE`.
//!

use crate::mac::enhanced_ack::{EnhancedAck, KeyIdMode};
use crate::mac::header::PhrFrame;
use crate::radio::event::AppliedSecurity;

const SECURITY_CONTROL_SIZE: usize = 1;
const FRAME_COUNTER_SIZE: usize = 4;
const KEY_ID_MODE_MASK: u8 = 0x18;
const KEY_ID_MODE_1: u8 = 0x08;

/// The MAC keys of key identifier mode 1 and the MAC frame counter, as
/// ESP-IDF's OpenThread port keeps them for its driver callback
/// (`otPlatRadioSetMacKey`, `otPlatRadioSetMacFrameCounter` and
/// `enh_ack_set_security_addr_and_key` of `esp_openthread_radio.c` at ESP-IDF
/// `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`).
///
/// The frame counter is the stack's single MAC frame counter; transmissions
/// the stack secures take theirs from [`Self::take_frame_counter`] too.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MacKeys {
    key_id: u8,
    previous: [u8; 16],
    current: [u8; 16],
    next: [u8; 16],
    frame_counter: u32,
}

impl MacKeys {
    /// Zeroed keys of key index zero and frame counter zero: what an
    /// interface without keys starts from when a key or frame counter
    /// update reaches it, as ESP-IDF's OpenThread port starts from its
    /// zeroed statics.
    pub const ZEROED: Self = Self::new(0, [0; 16], [0; 16], [0; 16], 0);

    /// Keys of `key_id` and its neighbours, and the next frame counter.
    pub const fn new(
        key_id: u8,
        previous: [u8; 16],
        current: [u8; 16],
        next: [u8; 16],
        frame_counter: u32,
    ) -> Self {
        Self {
            key_id,
            previous,
            current,
            next,
            frame_counter,
        }
    }

    /// Replace the keys (`otPlatRadioSetMacKey`).
    pub fn set_keys(&mut self, key_id: u8, previous: [u8; 16], current: [u8; 16], next: [u8; 16]) {
        self.key_id = key_id;
        self.previous = previous;
        self.current = current;
        self.next = next;
    }

    /// Replace the frame counter (`otPlatRadioSetMacFrameCounter`).
    pub fn set_frame_counter(&mut self, frame_counter: u32) {
        self.frame_counter = frame_counter;
    }

    /// Raise the frame counter (`otPlatRadioSetMacFrameCounterIfLarger`).
    pub fn set_frame_counter_if_larger(&mut self, frame_counter: u32) {
        self.frame_counter = self.frame_counter.max(frame_counter);
    }

    /// The next frame counter.
    pub const fn frame_counter(&self) -> u32 {
        self.frame_counter
    }

    /// Take the next frame counter.
    pub fn take_frame_counter(&mut self) -> u32 {
        let frame_counter = self.frame_counter;
        self.frame_counter = frame_counter.wrapping_add(1);
        frame_counter
    }

    /// Secure `ack` as the port does: take the next frame counter into it,
    /// then select the key of its key index. Only key identifier mode 1
    /// with a nonzero index of the current key or a neighbour is accepted;
    /// a refused ACK has still consumed its frame counter, as in the port.
    /// An unsecured ACK needs no key.
    pub fn secure(&mut self, ack: &mut EnhancedAck) -> Option<[u8; 16]> {
        let security = ack.security()?;
        ack.set_frame_counter(self.take_frame_counter());
        let key_index = match (security.key_id_mode, security.key_index) {
            (KeyIdMode::Index, Some(index)) if index != 0 => index,
            _ => return None,
        };
        // The port compares in `int`, so the neighbours do not wrap.
        let (key_index, key_id) = (i16::from(key_index), i16::from(self.key_id));
        if key_index == key_id {
            Some(self.current)
        } else if key_index == key_id - 1 {
            Some(self.previous)
        } else if key_index == key_id + 1 {
            Some(self.next)
        } else {
            None
        }
    }
}

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
        write_security_header(image, self.frame_counter, self.key_id)
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
        write_security_header(image, Some(self.frame_counter), self.key_id).is_some()
    }
}

/// Write the frame counter and, in key identifier mode 1, the key index
/// into the auxiliary security header of a secured `[PHR, MAC...]` image.
fn write_security_header(
    image: &mut [u8],
    frame_counter: Option<u32>,
    key_id: Option<u8>,
) -> Option<KeyIdMode> {
    let frame = PhrFrame::new(image);
    if !frame.security_enabled() {
        return None;
    }
    let offset = usize::from(frame.security_header_offset()?);
    let control = *image.get(offset)?;
    let mode_1 = control & KEY_ID_MODE_MASK == KEY_ID_MODE_1;
    let key_index = offset + SECURITY_CONTROL_SIZE + FRAME_COUNTER_SIZE;
    if image.len() <= key_index - 1 + usize::from(mode_1) {
        return None;
    }
    if let Some(counter) = frame_counter {
        image[offset + SECURITY_CONTROL_SIZE..key_index].copy_from_slice(&counter.to_le_bytes());
    }
    if mode_1 {
        if let Some(key_id) = key_id {
            image[key_index] = key_id;
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

impl MacKeys {
    /// The security of one transmit attempt (`otPlatRadioTransmit`): unless
    /// the attempt retransmits the frame, a new frame counter and the
    /// current key identifier; the current key either way. Every CCA
    /// attempt of a first transmission is a transmit of its own and takes a
    /// new counter.
    pub fn transmit_security(&mut self, retransmission: bool) -> TransmitSecurity {
        self.transmit_security_with_csl(retransmission, false)
    }

    /// [`Self::transmit_security`] of a CSL receiver: with `csl`, a
    /// retransmission takes a new frame counter too, as the port does while
    /// its CSL period is set, since the frame's CSL IE phase changes; it
    /// keeps its key index either way.
    pub fn transmit_security_with_csl(
        &mut self,
        retransmission: bool,
        csl: bool,
    ) -> TransmitSecurity {
        TransmitSecurity {
            frame_counter: (!retransmission || csl).then(|| self.take_frame_counter()),
            key_id: (!retransmission).then_some(self.key_id),
            key: self.current,
        }
    }
}

#[cfg(test)]
mod tests;
