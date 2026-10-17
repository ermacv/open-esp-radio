#![no_std]
#![forbid(unsafe_code)]

//! The ESP32-S31 binding of the Espressif coexistence policy.
//!
//! The policy itself — [`CoexCore`], [`CoexSchedule`], [`CoexScheduleExecutor`]
//! and the priority table — is the chip-neutral `oer-espressif-coex`, which
//! this crate re-exports. Register ownership deliberately lives outside both
//! crates: the radio arbiter owns the COEX timer bank, the event priority
//! table and the reviewed shared modem clock fields. [`CoexArbiterPorts`]
//! lends an arbiter lease to the core as its timer and clock ports, and
//! `oer_esp32s31_hal::coex::timer_clock` decodes the HAL's clock observation
//! for them. The radio
//! runtime composes both; Wi-Fi and Bluetooth LE publish their status to it,
//! and Wi-Fi reacts to its phases.

mod arbiter;

pub use arbiter::{CoexArbiterClock, CoexArbiterPorts, CoexArbiterTimer};
pub use oer_espressif_coex::{
    COEX_EVENT_COUNT, COEX_TIMER_COUNT, CoexClient, CoexClientRequest, CoexClockHardware,
    CoexClockSelector, CoexCore, CoexError, CoexEventDurations, CoexEventId, CoexExpiry, CoexPhase,
    CoexPhaseChange, CoexPhaseStep, CoexPhaseTimer, CoexPti, CoexPtiTable, CoexSchedule,
    CoexScheduleExecutor, CoexScheduleIdle, CoexScheme, CoexSchemeId, CoexStatus, CoexStatusType,
    CoexStatusWords, CoexTimerClock, CoexTimerHardware, CoexTimerIndex, NoRequestKind,
    program_timer, timer_index, wifi_status,
};

#[cfg(feature = "validation-probes")]
#[doc(hidden)]
pub mod validation;
