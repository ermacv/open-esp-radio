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
//!   group's;
//! - `observe` (`observe` feature): what the library observes of its own
//!   work, for an image that measures it.

pub use oer_chips_family_app::channel;
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
// The chip's placement of a task group, which `entry!` expands.
#[cfg(all(feature = "esp32s31", feature = "group", target_arch = "riscv32"))]
#[doc(hidden)]
pub use oer_chips_esp32s31_app::{__group_entry, __group_mailbox};

#[cfg(not(feature = "group"))]
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        $crate::__private::entry!(
            __oer_main,
            $crate::__private::Port,
            exception = $crate::__private::exception
        );

        fn __oer_main(handoff: $crate::__private::Handoff<$crate::__private::Port>) -> ! {
            $crate::__private::start(handoff, $main, $crate::__private::no_group, ())
        }
    };
    // Without the `group` feature a task group runs on the application's
    // executor, beside the application's future: its networks with it.
    ($main:path, group = $group:path) => {
        $crate::__private::entry!(
            __oer_main,
            $crate::__private::Port,
            exception = $crate::__private::exception
        );

        fn __oer_main(handoff: $crate::__private::Handoff<$crate::__private::Port>) -> ! {
            $crate::__private::start(handoff, $main, $group, ())
        }
    };
}

/// The application's entry with a task group on its own hart: what
/// [`entry!`] is with the `group` feature.
#[cfg(feature = "group")]
#[macro_export]
macro_rules! entry {
    ($main:path) => {
        compile_error!("the `group` feature needs a task group: `entry!(main, group = setup)`");
    };
    ($main:path, group = $group:path) => {
        $crate::__private::entry!(
            __oer_main,
            $crate::__private::Port,
            exception = $crate::__private::exception
        );
        $crate::__group_entry!($group);

        fn __oer_main(handoff: $crate::__private::Handoff<$crate::__private::Port>) -> ! {
            $crate::__private::start(handoff, $main, $group, $crate::__group_mailbox!())
        }
    };
}
