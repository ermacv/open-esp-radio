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

/// Write a Time IE's sequence and already computed network time, little
/// endian, at its MAC-byte offset. A refused write leaves the frame unchanged.
pub fn write_time_ie(mac: &mut [u8], ie_offset: u8, sequence: u8, network_time: u64) -> bool {
    let Some((_, content)) = mac.split_at_mut_checked(usize::from(ie_offset)) else {
        return false;
    };
    let Some((content, _)) = content.split_first_chunk_mut::<9>() else {
        return false;
    };
    let [t0, t1, t2, t3, t4, t5, t6, t7] = network_time.to_le_bytes();
    *content = [sequence, t0, t1, t2, t3, t4, t5, t6, t7];
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ready_network_time_is_written_atomically_at_the_mac_offset() {
        let mut mac = [0xee; 12];
        assert!(write_time_ie(&mut mac, 2, 7, 0x0102_0304_0506_0708));
        assert_eq!(mac, [0xee, 0xee, 7, 8, 7, 6, 5, 4, 3, 2, 1, 0xee]);
        let before = mac;
        assert!(!write_time_ie(&mut mac, 4, 1, 0));
        assert_eq!(mac, before);
        assert!(!write_time_ie(&mut mac, u8::MAX, 1, 0));
        assert_eq!(mac, before);
    }
}
