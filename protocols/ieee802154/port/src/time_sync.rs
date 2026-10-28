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
