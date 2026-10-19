//! ESP32-S31 Bluetooth hardware engine.
//!
//! PAC/HAL owns MMIO, and the separate memory crate owns controller-SRAM
//! layouts and CPU/hardware ownership. This crate joins those contracts into
//! the radio engine: clocks, controller HAL and time, BLE PHY, interrupts,
//! the modem low-power timer, and the hardware scheduler with its timeline,
//! finished lists and single-item primitives. It knows no Link Layer role.
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

/// The named settle of a scheduler diagnostic sample on `clock`.
pub(crate) fn diagnostic_settle(
    clock: &dyn oer_time::Clock,
) -> oer_esp32s31_hal::settle::Settle<'_> {
    oer_esp32s31_hal::settle::Settle::new(
        oer_esp32s31_bluetooth_memory::scheduler::DIAGNOSTIC_SETTLE,
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
pub mod phy;
pub mod resources;
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

pub mod memory;
