#![no_std]
#![deny(unsafe_code, clippy::undocumented_unsafe_blocks)]

//! The image's interrupt table (`oer-interrupt-table`) on the interrupt
//! matrix of an Espressif chip, as esp-hal drives it. A chip feature selects
//! the chip of esp-hal.
//!
//! Every image declares its peripheral interrupt sources once with the
//! staged runtime's `interrupt_table!` and hands the table to the runtime,
//! which [`adopt`]s it, silences each source of a hart's entries when it
//! installs that hart's interrupt stack ([`install_current_hart`]) and checks
//! again before it enables interrupts ([`verify_current_hart`]). An owner routes
//! its source with [`enable`] and its token, and silences it with [`disable`].
//!
//! esp-hal's `static-interrupts` feature leaves the table the only owner of
//! routes: [`adopt`] takes esp-hal's one routing capability, and a driver that
//! needs its interrupt (`into_async`) only requires the image's route, which
//! [`verify_current_hart`] checks is routed before interrupts are enabled.
//!
//! The table reads the matrix and the CLIC through our typed blocks, which
//! [`adopt`] claims from the interrupt matrix handoff; esp-hal still writes
//! the routes and, in its vectoring, the lines. [`verify_current_hart`]
//! checks every bound level's line and every bound route ([`lines`]) before
//! interrupts are enabled, and [`is_pending`] reads a core's pending sources.

pub mod lines;

#[cfg(any(feature = "esp32s31", feature = "esp32c5"))]
mod table;
#[cfg(any(feature = "esp32s31", feature = "esp32c5"))]
pub use table::*;
