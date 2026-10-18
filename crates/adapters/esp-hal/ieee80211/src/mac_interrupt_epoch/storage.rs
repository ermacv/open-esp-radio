//! The only static context address needed by context-free hard handlers.
//!
//! Registers, publication storage, driver state and wakes are owned by the
//! pinned installation. DriverScope's safety argument covers this slot and
//! both IRQ routes together; cancellation and forget are tested there.

#[cfg(feature = "esp32s31")]
oer_esp32s31_ieee80211_form::declare_scope_slot!(pub(super) static INSTALLATION);
