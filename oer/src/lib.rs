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
//! The Espressif family's application entry.
//!
//! An application selects its chip by feature (`esp32s31`), takes the
//! peripherals it needs once and builds drivers and services from them with
//! defaults. The library places their interrupts, executors and service
//! tasks and applies the record-and-reset policy on the image's behalf.
//!
//! - `AesEngine` (`esp32s31`): the AES accelerator as one synchronous
//!   AES-128 engine, owned by the image or by the Wi-Fi station;
//! - [`channel`]: a typed channel between the application's future and the
//!   second boot's;
//! - [`net`]: waiting on the IP service for an address or a link loss;
//! - `observe` (`observe` feature): what the library observes of its own
//!   work, for an image that measures it.

pub use oer_chips_family_app::{channel, net};
pub mod print;

/// What the library observes of its own work, for an image that measures
/// it: the family's counters and the chip's readers.
#[cfg(feature = "observe")]
pub mod observe {
    #[cfg(all(feature = "esp32s31", target_arch = "riscv32"))]
    pub use oer_chips_esp32s31_app::observed::*;
    pub use oer_chips_family_app::observe::*;
}

#[cfg(all(target_arch = "riscv32", not(feature = "esp32s31")))]
compile_error!("select the application's chip: the `esp32s31` feature");

#[cfg(all(feature = "esp32s31", target_arch = "riscv32"))]
pub use oer_chips_esp32s31_app::*;

#[macro_export]
macro_rules! entry {
    ($boot:path) => {
        $crate::entry!($boot, hart1 = $crate::__private::no_hart1);
    };
    ($boot:path, hart1 = $hart1:path) => {
        $crate::__private::entry!(
            __oer_main,
            $crate::__private::Port,
            exception = $crate::__private::exception
        );
        $crate::__hart1_entry!($hart1);

        fn __oer_main(handoff: $crate::__private::Handoff<$crate::__private::Port>) -> ! {
            $crate::__private::start(handoff, $boot, $hart1, $crate::__hart1_mailbox!())
        }
    };
}

/// Hart 1's mailbox and root, which call the second boot on hart 1: what
/// [`entry!`] expands to with the `hart1` feature. Not an API.
#[cfg(feature = "hart1")]
#[doc(hidden)]
#[macro_export]
macro_rules! __hart1_entry {
    ($hart1:path) => {
        $crate::__private::secondary_entry!(
            1,
            __OER_HART1: $crate::__private::Port,
            $crate::Hart1Parts => __oer_hart1_main
        );

        fn __oer_hart1_main(
            handoff: $crate::__private::SecondaryHandoff<$crate::__private::Port, 1>,
            parts: $crate::Hart1Parts,
        ) -> ! {
            $crate::hart1_main(handoff, parts, $hart1)
        }
    };
}

/// Without the `hart1` feature: nothing. Not an API.
#[cfg(not(feature = "hart1"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __hart1_entry {
    ($hart1:path) => {};
}

/// The mailbox `entry!` passes the library. Not an API.
#[cfg(feature = "hart1")]
#[doc(hidden)]
#[macro_export]
macro_rules! __hart1_mailbox {
    () => {
        &__OER_HART1
    };
}

/// Without the `hart1` feature: none. Not an API.
#[cfg(not(feature = "hart1"))]
#[doc(hidden)]
#[macro_export]
macro_rules! __hart1_mailbox {
    () => {
        ()
    };
}
