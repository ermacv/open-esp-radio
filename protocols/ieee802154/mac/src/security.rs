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

use oer_ieee802154_port::TransmitSecurity;
use oer_ieee802154_wire::{EnhancedAck, KeyIdMode};

/// The MAC keys of key identifier mode 1 and the MAC frame counter, as
/// ESP-IDF's OpenThread port keeps them for its driver callback
/// (`otPlatRadioSetMacKey`, `otPlatRadioSetMacFrameCounter` and
/// `enh_ack_set_security_addr_and_key` of `esp_openthread_radio.c` at ESP-IDF
/// `4d80c1b29aeabc8e735ffc6e1be9bb23e95d1d5d`).
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

    /// Replace the keys and optionally their frame counter in one update.
    /// With `None`, the next frame counter stays unchanged.
    pub fn set_keys(
        &mut self,
        key_id: u8,
        previous: [u8; 16],
        current: [u8; 16],
        next: [u8; 16],
        frame_counter: Option<u32>,
    ) {
        self.key_id = key_id;
        self.previous = previous;
        self.current = current;
        self.next = next;
        if let Some(counter) = frame_counter {
            self.frame_counter = counter;
        }
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
        } else if Some(key_index) == key_id.checked_sub(1) {
            Some(self.previous)
        } else if Some(key_index) == key_id.checked_add(1) {
            Some(self.next)
        } else {
            None
        }
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
