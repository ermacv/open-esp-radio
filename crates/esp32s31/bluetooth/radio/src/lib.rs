#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! The portable Bluetooth LE radio event contract (`oer-bluetooth-radio`)
//! over the ESP32-S31 scheduler executor and role instance pools.
//!
//! [`BluetoothRadio`] lowers every [`RadioRequest`] into the role pools of
//! [`BluetoothRadioMemory`]: a configured advertising set, scanner or
//! connection holds one pool instance, and each event fills that instance's
//! scheduler items. The items wait in a queue until the caller drives them
//! into the hardware list through the executor. Completed and released items
//! finish their event: receptions leave the receive chains as
//! [`RadioOutcome::Received`] and the event ends with
//! [`RadioOutcome::EventEnded`].
//!
//! The radio holds no register: [`BluetoothRadio::drive`] and
//! [`BluetoothRadio::advance`] return the executor's hardware actions, which
//! the caller performs with the HAL, and the caller reports finished lists
//! through [`BluetoothRadio::complete`]. The caller serializes every entry.
//!
//! Air windows become reservations that start the scheduler's preparation
//! lead before the anchor, as [`RadioTiming`] describes.
//!
//! The radio time extends the 32-bit controller clock. Its epoch starts
//! 2^32 microseconds above zero and ends as far below `u64::MAX`, so a
//! packet capture, which lies within the raw counter's half range of the
//! latest sample, always has a representable on-air start. A sample past
//! the last instant leaves the time unchanged and reports
//! [`OutsideEpoch`](oer_bluetooth_radio::OutsideEpoch): nothing timed may be
//! admitted afterwards, while the admitted work still settles.

#[cfg(any(test, all(feature = "test-support", not(target_arch = "riscv32"))))]
extern crate std;

#[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
mod coexistence;
#[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
mod radio;
#[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
mod trace;
#[cfg(any(test, all(feature = "test-support", not(target_arch = "riscv32"))))]
#[doc(hidden)]
pub mod validation;

#[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
pub use coexistence::CoexistenceProfile;
#[cfg(any(target_arch = "riscv32", test, feature = "test-support"))]
pub use radio::{
    BluetoothCapabilities, BluetoothRadio, BluetoothRadioConfig, BluetoothRadioMemory,
    BluetoothRadioSink, RadioFault, RadioFaulted, RadioStep,
};

pub use oer_bluetooth_radio::{RadioOutcome, RadioRequest, RadioTiming, RequestError};
