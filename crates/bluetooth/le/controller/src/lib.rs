#![no_std]
#![forbid(unsafe_code)]

//! Pure pieces of a Bluetooth LE Controller: no transport, executor, clock,
//! scheduler or hardware.
//!
//! - [`timing`]: connection-event geometry over the backend's
//!   [`ConnectionAllowances`](oer_bluetooth_radio::ConnectionAllowances)
//!   (first and recurring peripheral events, sleep-clock window widening) and
//!   legacy advertising channel geometry after the vendor item geometry, with
//!   [`EpochExhausted`](timing::EpochExhausted) when a result leaves the radio
//!   epoch;
//! - [`receive`]: authentication of received data channel PDUs before Link
//!   Layer and Host dispatch;
//! - [`diagnostic`]: the codec of the vendor HCI command `0xFC01` that armed
//!   one received-MIC corruption, and the corruption itself.
//!
//! No Controller core serves HCI commands or runs the Link Layer roles over a
//! radio request port yet.

#[cfg(test)]
extern crate std;

pub mod diagnostic;
pub mod receive;
pub mod timing;
