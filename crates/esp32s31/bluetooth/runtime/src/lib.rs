#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Executor-independent Bluetooth LE radio runtime for the ESP32-S31.
//!
//! The radio is a port backend ([`BluetoothBackend`], an
//! [`oer_bluetooth_radio::LeRadioBackend`]) run under the shared port owner
//! ([`oer_bluetooth_port::run_port`]), with a driver that owns the radio role
//! and its hardware by value ([`BluetoothDriver`]). [`BluetoothRuntime`] is
//! the part the backend, the driver and the platform share: the outcome
//! queue, the request exchange, the poison and the wakes, behind signals,
//! atomics and short synchronous sections; nothing holds a lock across an
//! await.
//!
//! - [`BluetoothRuntime::install`] publishes the global receive chains, takes
//!   the first controller-time sample and returns the backend with the
//!   driver inside, so no call is refused as not installed.
//! - The driver, which `run_port` joins with the port's body, drives the
//!   scheduler: it reports finished events, inserts admitted events, starts
//!   the idle scheduler, carries list transactions through their hardware
//!   waits and samples the controller clock at least every
//!   [`TIME_REFRESH`], enabled or disabled. Between passes it serves the
//!   port's requests, each a message that owns its PDUs
//!   ([`oer_bluetooth_radio::OwnedRadioRequest`]).
//! - The owner's `enable` resumes the scheduler and its `disable` stops it
//!   and ends every admitted event with what the hardware recorded
//!   (`EventResult::Aborted` when a started item left no status), each
//!   within [`TRANSITION_BOUND`]. The uninstall retires the driver, which
//!   leaves the radio and hardware by value, or keeps them with why
//!   ([`BluetoothRetained`]) when the port was poisoned. Shared-PHY
//!   maintenance never pauses the port: it runs under the radio arbiter's
//!   lease with the vendor grant-protect brackets, and an event that loses
//!   RF to it ends as an ordinary outcome.
//!
//! A refused request is an answer to the Controller core, and a fault
//! poisons the port with its [`BluetoothFault`]. Submission and the clock
//! stay asynchronous because each takes a fresh controller-time latch,
//! which completes only after the hardware latched the time. The port's
//! capabilities
//! ([`BluetoothCapabilities`](oer_esp32s31_bluetooth_radio::BluetoothCapabilities))
//! state LE 1M roles and hardware Link Layer acknowledgement: the hardware
//! keeps SN and NESN, retransmits and answers with empty PDUs.
//!
//! Outcomes leave the driver as owned values through a bounded queue the
//! port's event consumer takes from; each admitted event reserves the slots
//! of its terminal outcomes. The platform calls
//! [`BluetoothRuntime::on_scheduler_wake`] when its scheduler interrupt
//! publishes a wake for the driver.
//!
//! [`BluetoothRadioHardware`] names every hardware operation; on the chip,
//! `LiveBluetoothHardware` joins the powered task endpoint of one Controller
//! epoch with the platform's interrupt-owner storage, and `run_modem_timer`
//! drives the source-127 timer task beside the radio.

#[cfg(test)]
extern crate std;

#[cfg(any(target_arch = "riscv32", test))]
mod hardware;
#[cfg(target_arch = "riscv32")]
mod modem_timer;
mod outcome;
#[cfg(any(target_arch = "riscv32", test))]
mod port;
#[cfg(any(target_arch = "riscv32", test))]
mod runtime;
#[cfg(any(target_arch = "riscv32", test))]
mod trace;

#[cfg(target_arch = "riscv32")]
pub use hardware::LiveBluetoothHardware;
#[cfg(any(target_arch = "riscv32", test))]
pub use hardware::{BluetoothRadioHardware, RxChainPublicationError};
#[cfg(target_arch = "riscv32")]
pub use modem_timer::{ModemTimerFault, run_modem_timer, settle_modem_timer};
pub use outcome::{BluetoothOutcome, BluetoothReceivedPdu, MAX_PDU_BYTES};
#[cfg(any(target_arch = "riscv32", test))]
pub use port::{BluetoothBackend, BluetoothClient, BluetoothEvents, BluetoothPhaseControl};
#[cfg(any(target_arch = "riscv32", test))]
pub use runtime::{
    BluetoothDriver, BluetoothFault, BluetoothInstallError, BluetoothRetained, BluetoothRuntime,
    BluetoothRuntimeFault, BluetoothTimeError, BluetoothUninstallError, HARDWARE_RECHECK,
    TIME_REFRESH, TRANSITION_BOUND,
};

#[cfg(test)]
mod tests;
