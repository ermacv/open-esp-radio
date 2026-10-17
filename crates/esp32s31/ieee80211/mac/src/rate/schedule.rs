//! The transmit-rate schedule arenas of the ESP32-S31 rate control.
//!
//! The arenas, their record walk and publication budget are the Espressif
//! family's policy data, recovered from the pinned ESP32-S31 `libpp.a`; they
//! live in `oer-espressif-ieee80211-policy` and are re-exported here for the
//! chip's rate control and retry owners.

pub use oer_espressif_ieee80211_policy::rate_schedule::{
    RATE_SCHEDULE_RECORD_SIZE, RateScheduleKind, RateScheduleRecordState, RateScheduleRef,
    schedule_publication_limit, schedule_rate_after_failures, schedule_state,
};
