#![deny(
    clippy::unwrap_used,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented
)]
//! ESP32-S31 Bluetooth hardware engine.
//!
//! The HAL owns MMIO, controller-SRAM layouts and CPU/hardware ownership.
//! One owning Engine retains the register token and placed arena. This crate joins those contracts into
//! the radio engine: clocks, controller HAL and time, BLE PHY, interrupts,
//! the modem low-power timer, and the hardware scheduler with its timeline,
//! finished lists and single-item primitives, with detached role value builders.
//!
//! The shared single-item completion and timed preparation engines
//! implement the common hardware protocol; RX/recycle and packet policy
//! belong to the caller. An epoch begins from the HAL's clocked Bluetooth
//! client of the shared radio and ends in [`shutdown`], which returns it.
//! Initialization and scheduler RUN are not RF evidence.
//!
//! See the chip `FEATURES.md` for implemented scopes, unsupported operations
//! and the distinction between source coverage and hardware qualification.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod ble_phy;
pub mod coex_status;
pub mod coexistence;
pub mod connection_policy;
/// Controller HAL component initialization after clock setup.
#[cfg(any(target_arch = "riscv32", test))]
pub mod controller_hal;
/// Event-driven controller-time latch and scheduler-epoch projection.
pub mod controller_time;
pub mod interrupt;
/// Owned P1/P2 queues for the S31 DTM backend and its supervised lifecycle.
pub mod port;

/// The named settle of a scheduler diagnostic sample on `clock`.
pub(crate) fn diagnostic_settle<C: oer_time::Clock>(clock: &C) -> oer_time::settle::Settle<'_, C> {
    oer_time::settle::Settle::new(
        oer_esp32s31_hal::bluetooth::engine::scheduler::DIAGNOSTIC_SETTLE,
        clock,
    )
}
#[cfg(any(target_arch = "riscv32", test))]
pub mod low_power;
pub mod modem_lp_timer_queue;
/// Controller modem low-power timer task over the published timer owner.
#[cfg(target_arch = "riscv32")]
pub mod modem_timer;
/// Drain-before-retire rule of the modem low-power timer.
#[cfg(any(target_arch = "riscv32", test))]
pub mod modem_timer_retirement;
#[cfg(target_arch = "riscv32")]
pub mod radio;
pub mod resources;
pub mod roles;
pub mod runtime_resources;
pub mod scheduler;
/// Retirement and reset of a stopped Controller epoch.
#[cfg(target_arch = "riscv32")]
pub mod shutdown;
/// Controller-time preparation shared by timed scheduler admissions.
#[cfg(any(target_arch = "riscv32", test))]
pub mod timed_preparation;

#[cfg(target_arch = "riscv32")]
pub use ble_phy::AlwaysAwakeTimingReady;

#[cfg(any(target_arch = "riscv32", test))]
pub use controller_time::{
    ControllerDurationError, ControllerSchedulerEpoch, ControllerTimeSample,
};

#[cfg(test)]
mod test_clocks;
