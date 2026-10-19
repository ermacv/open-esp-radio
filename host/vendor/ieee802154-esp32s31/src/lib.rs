//! Host differential stand for the ESP32-S31 IEEE 802.15.4 driver port.
//!
//! The pinned public ESP-IDF driver is compiled unmodified for the host (see
//! `build.rs`). Its register layer (`ieee802154_ll_*`), the closed PHY, BTBB
//! and coexistence calls, modem clocks, interrupt allocation, time and the
//! application callbacks are replaced by recorders. A [`Scenario`] drives the
//! public API and delivers interrupts through the handler the driver
//! registered; [`scenario::run`] returns the resulting boundary trace.
//! [`port::run`] drives the production engine through the same scenario and
//! [`compare`] reports `MATCH`, `DIFF` or `INCOMPLETE`.
//!
//! The compiled driver keeps its state in C globals. One process therefore
//! runs one scenario, which [`claim`] enforces; the stand's program
//! (`tools/vendor-ieee802154-esp32s31`) runs a named catalog scenario per
//! invocation.

pub mod catalog;
pub mod compare;
pub mod port;
pub mod record;
pub mod scenario;

use std::sync::atomic::{AtomicBool, Ordering};

pub use compare::{Verdict, compare};
pub use record::{Argument, Inputs, Record};
pub use scenario::{Scenario, Step};

/// The stand's package directory: its shard records this package's path
/// closure, and the program, a report package, stays out of it.
pub const DIRECTORY: &str = env!("CARGO_MANIFEST_DIR");

static CLAIMED: AtomicBool = AtomicBool::new(false);

/// Run `scenario` as the only scenario of this process.
///
/// # Panics
///
/// Panics if another scenario already ran in this process: the vendor driver
/// state cannot be reset in place.
pub fn claim(scenario: &Scenario) -> Vec<Record> {
    assert!(
        !CLAIMED.swap(true, Ordering::SeqCst),
        "the vendor driver runs one scenario per process"
    );
    scenario::run(scenario)
}
