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

use oer_ieee802154_port::TimeSync;

/// Fill the Time IE at the supplied SFD observation with the sequence and
/// wrapping network time. A refused write leaves the frame unchanged.
pub fn write_time_sync(sync: &TimeSync, mac: &mut [u8], now_micros: u64) -> bool {
    let network_time = (now_micros as i64).wrapping_add(sync.network_time_offset) as u64;
    oer_ieee802154_wire::write_time_ie(mac, sync.ie_offset, sync.sequence, network_time)
}

#[cfg(test)]
mod tests;
