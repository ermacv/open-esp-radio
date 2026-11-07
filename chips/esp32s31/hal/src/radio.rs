//! The radio's hardware: the shared domain and modem, the PHY's registers,
//! coexistence and each protocol's MAC. Atoms and owners only; algorithms
//! and lifecycles belong to the crates of `chips/esp32s31/radio/`.

pub mod bluetooth;
pub mod coex;
pub mod domain;
pub mod ieee80211;
pub mod ieee802154;
pub mod modem;
pub mod phy;
