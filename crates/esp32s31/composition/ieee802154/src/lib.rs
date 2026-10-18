#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]
#![deny(missing_docs)]

//! ESP32-S31 IEEE 802.15.4 as a client of the shared radio arbiter.
//!
//! [`start`] follows ESP-IDF's `esp_i_enable` over the concurrent
//! radio split: common radio power and the IEEE 802.15.4 module clocks, the
//! shared PHY domain (registered, woken or joined), the shared BTBB baseband
//! and transmit-on delay, the MAC reset and masked foundation, then the
//! interrupt owner, the MAC engine's `mac_init` inside the runtime, and
//! finally the CPU route of modem source 132. [`ISystem::stop`]
//! reverses every step. A step that fails before touching shared state rolls
//! the earlier steps back and returns the partition; a started PHY or clock
//! transaction that fails is fail-stop.
//!
//! The runtime is a process singleton: modem source 132 has one handler and
//! the MAC has one set of owners. [`start`] returns the running client and
//! the radio's port backend ([`ISystemBackend`]), which the
//! application's body runs under `run_port`; the port's end returns the
//! engine and the MAC owners ([`ISystemParts`]), which
//! [`ISystem::stop`] takes.

mod caps;
mod maintenance;
#[cfg(target_arch = "riscv32")]
mod system;

pub use caps::{Esp32s31Ieee802154, Esp32s31Ieee802154Multipan, MultipanInterfaces};
pub use maintenance::Ieee802154PhyMaintenance;
// The owner shape the backend serves: `run_port` and the owner's types, and
// the IEEE 802.15.4 port's traits, so an application drives the port
// through this crate (or the `oer` facade) alone.
pub use oer_ieee802154::owner;
pub use oer_ieee802154_port as port;

// Inputs of `start` and `IParked::new`, and the shared radio the
// client joins, so an application can construct it through this crate (or
// the `oer` facade) alone.
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_hal::{ieee802154::ll::Ieee802154MacOwners, root::RadioHardware};
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_radio_esp_hal::{EspHalRadioClocks, EspHalRadioPlatform};
pub use oer_espressif_ieee802154_engine::pib::Ieee802154PibDefaults;
// The coexistence levels a Thread role change maps its priority to
// (`ISystem::update_coexistence`).
pub use oer_esp32s31_hal::{coex::Ieee802154CoexLevel, ieee802154::coex::Ieee802154CoexConfig};

/// The shared ESP32-S31 radio IEEE 802.15.4 joins: the arbiter with the
/// esp-hal platform and clock sources. The application creates it once and
/// runs its periodic PHY tracking
/// ([`RadioSystem::run_tracking`](oer_esp32s31_radio_runtime::RadioSystem::run_tracking)).
#[cfg(target_arch = "riscv32")]
pub type SharedRadio = oer_esp32s31_radio_runtime::RadioSystem<
    EspHalRadioPlatform,
    EspHalRadioClocks,
    oer_time_embassy::EmbassyClock,
>;

/// The image hands source 132's route to the adapter once at boot.
#[cfg(target_arch = "riscv32")]
pub use oer_esp32s31_ieee802154_esp_hal::{
    EspHalIeee802154Source as Ieee802154InterruptSource, install as install_interrupt_route,
};
#[cfg(target_arch = "riscv32")]
pub use system::{
    IEEE802154_EVENT_CAPACITY, Ieee802154FailStop, Ieee802154MaintenanceError, Ieee802154Parked,
    Ieee802154StartError, Ieee802154StartFailure, Ieee802154StopError, Ieee802154StopFailure,
    Ieee802154System, Ieee802154SystemBackend, Ieee802154SystemClock, Ieee802154SystemParts,
    Ieee802154SystemRssi, Ieee802154SystemRuntime, ieee802154_interrupt, start,
};
