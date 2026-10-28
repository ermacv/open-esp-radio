#![no_std]
#![forbid(unsafe_code)]
#![deny(
    clippy::panic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects,
    clippy::disallowed_macros,
    clippy::disallowed_methods
)]

//! Executor-neutral coexistence policy of the Espressif chips, recovered from
//! esp-coex-lib.
//!
//! Register ownership deliberately lives outside this crate. Each chip's
//! radio arbiter owns its COEX timer bank, its [`CoexPtiTable`] and its
//! shared modem clock fields; this crate is the policy that drives them
//! through its ports ([`CoexTimerHardware`], [`CoexClockHardware`]).
//! [`CoexCore`] programs event requests on the timer bank; [`CoexSchedule`]
//! is the recovered time-slice schedule that selects a scheme from the
//! radios' status and steps its phases; [`CoexScheduleExecutor`] adds its
//! phase timer. A chip crate binds the ports to its HAL.
//!
//! The portable client vocabulary is `oer-radio-coex`: [`CoexClient`] and
//! [`CoexStatusType`] convert from it into the vendor's request kinds and
//! status words, and each protocol's operation maps to its table row
//! ([`wifi_event`], [`ieee802154_event`]); the Bluetooth LE controller's lanes
//! stay with its driver.

#[cfg(test)]
extern crate std;

mod clock;
mod core;
mod executor;
mod model;
mod operation;
mod pti;
mod schedule;
mod timer;

pub use clock::{CoexClockHardware, CoexClockSelector, CoexTimerClock};
pub use core::{CoexCore, CoexStatus};
pub use executor::{CoexExpiry, CoexPhaseChange, CoexPhaseTimer, CoexScheduleExecutor};
pub use model::{
    COEX_TIMER_COUNT, CoexClient, CoexClientRequest, CoexError, CoexEventDurations, CoexTimerIndex,
    NoRequestKind, timer_index,
};
pub use operation::{ieee802154_event, wifi_event};
pub use pti::{COEX_EVENT_COUNT, CoexEventId, CoexPti, CoexPtiTable, wifi_events};
pub use schedule::{
    CoexPhase, CoexPhaseStep, CoexSchedule, CoexScheduleIdle, CoexScheme, CoexSchemeId,
    CoexStatusType, CoexStatusWords, EXTERNAL_COEX_STATUS, IEEE802154_COEX_STATUS, wifi_status,
};
pub use timer::{CoexTimerHardware, program_timer};

#[cfg(test)]
mod tests;
