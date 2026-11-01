#![cfg_attr(not(test), no_std)]
#![forbid(unsafe_code)]
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented,
    clippy::indexing_slicing
)]
//! The ESP32-S31 side of an image's platform over the Espressif staged
//! runtime (`oer-espressif-staged-runtime`) and executor
//! (`oer-espressif-executor`), whose fatal paths and owned blocks are the
//! HAL's port ([`oer_esp32s31_hal::platform::Port`]).
//!
//! - `Esp32s31`: the executor's chip binding (chip target only), whose statics `bind!`
//!   declares here once: the storage of the port's owned blocks, the one
//!   waker vtable. Images name it in every executor type
//!   (`Executor<Esp32s31, 0>`) and declare their Embassy time driver.
//!
//! It also holds the chip's retained fault records, which need both the
//! HAL's types and the runtime's retained memory and fatal path, so neither
//! of them can hold them:
//!
//! - [`phy_fault`]: the PHY transaction seat, its retained fault record, the
//!   reset the HAL's `phy_fault_reporter!` names and the next boot's report,
//!   which the image passes to `adoption::run`;
//! - `wifi_fault` (feature `wifi-fault`): the Wi-Fi MAC's fatal assertion
//!   record, its reset and the next boot's report;
//! - [`exception`]: the exception hook every image passes to `entry!`,
//!   which retains what the interrupt controller showed after the trap
//!   beside the runtime's trap-time frame, and the next boot's report.
//!
//! [`sampler`] is the program-counter sampler on TIMG1 T0 an image may
//! select, reporting through its observer.
//!
//! [`secondary`] launches hart 1 during boot, under its own deadline over the
//! system timer, before the image starts its executor, and hands it its
//! interrupt grant and the image's parts for it.

pub mod crypto;
pub mod exception;
pub mod phy_fault;

/// The ESP32-S31's executor binding over the HAL's platform port; the
/// executor runs on the chip target only.
#[cfg(target_arch = "riscv32")]
pub struct Esp32s31 {
    _private: (),
}
#[cfg(target_arch = "riscv32")]
oer_espressif_executor::bind!(Esp32s31, oer_esp32s31_hal::platform::Port);

#[cfg(target_arch = "riscv32")]
impl oer_espressif_staged_runtime::adoption::Binding for Esp32s31 {
    type Platform = oer_esp32s31_hal::platform::Port;
    type Driver = oer_esp32s31_hal::root::RuntimePort;
    type Hardware = oer_esp32s31_hal::root::Hardware;
    type Interrupts = oer_esp32s31_hal::platform::Hart0Grant;
    type Controller = oer_esp32s31_hal::platform::InterruptController;

    fn split(parts: oer_esp32s31_hal::root::PlatformParts) -> (Self::Hardware, Self::Interrupts) {
        (parts.hardware, parts.interrupts.grant)
    }

    fn controller(
        interrupts: Self::Interrupts,
    ) -> Result<Self::Controller, oer_espressif_platform_port::ForeignHart> {
        interrupts.into_controller()
    }
}

/// Shared retained/reset path selected only by staged Wi-Fi images.
#[cfg(feature = "wifi-fault")]
pub mod wifi_fault;

pub mod reset_causes;
pub mod sampler;
pub mod secondary;

#[cfg(target_arch = "riscv32")]
pub mod pilot;
#[cfg(target_arch = "riscv32")]
pub mod platform;

#[cfg(target_arch = "riscv32")]
pub mod console;
