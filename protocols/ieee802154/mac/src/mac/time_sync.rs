//! Thread network time synchronization as a radio serves it for OpenThread
//! (`OPENTHREAD_CONFIG_TIME_SYNC_ENABLE`): the Time IE of a transmitted
//! frame gets the time sync sequence and the network time when the frame's
//! SFD goes out (`ot_radio_transmit_sfd_done` of `esp_openthread_radio.c`
//! at ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`).
//!
//! Ported from ESP-IDF `7b9cc1ac79f865983f59bb8ff3ff43eb74ff1dbe`
//! `components/openthread/src/port/esp_openthread_radio.c` (Copyright
//! Espressif Systems (Shanghai) Co Ltd, Apache-2.0) and modified by Vladislav
//! Ermachenkov; see `NOTICE`.
//!

/// The Time IE of one transmission, as OpenThread describes it in the
/// frame's `otRadioIeInfo`.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct TimeSync {
    /// Offset of the Time IE content - the sequence byte, then the eight
    /// bytes of network time - from the first MAC byte (`mTimeIeOffset`).
    pub ie_offset: u8,
    /// The time sync sequence (`mTimeSyncSeq`).
    pub sequence: u8,
    /// The network time minus the radio clock, in microseconds
    /// (`mNetworkTimeOffset`).
    pub network_time_offset: i64,
}

/// Bytes of the Time IE content the radio writes: the sequence and the
/// network time.
const TIME_IE_CONTENT: usize = 1 + 8;

impl TimeSync {
    /// Write the sequence and the network time into the Time IE of the
    /// frame whose MAC bytes are `mac`: the radio time `now_micros` plus
    /// the network time offset, wrapping, little endian. Returns `false`,
    /// leaving the bytes, when the IE does not fit.
    pub fn write(&self, mac: &mut [u8], now_micros: u64) -> bool {
        let Some((_, content)) = mac.split_at_mut_checked(usize::from(self.ie_offset)) else {
            return false;
        };
        let Some((content, _)) = content.split_first_chunk_mut::<TIME_IE_CONTENT>() else {
            return false;
        };
        let [t0, t1, t2, t3, t4, t5, t6, t7] = (now_micros as i64)
            .wrapping_add(self.network_time_offset)
            .to_le_bytes();
        *content = [self.sequence, t0, t1, t2, t3, t4, t5, t6, t7];
        true
    }
}

#[cfg(test)]
mod tests;
