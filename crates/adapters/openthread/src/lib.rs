#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! The [`openthread`] crate's `Radio` over the running phase of any IEEE
//! 802.15.4 port on the owner shape ([`oer_i::owner`]), such as
//! the Espressif IEEE 802.15.4 runtime's. OpenThread starts from the transmit power, CCA
//! threshold and receive sensitivity of the port's capabilities. The
//! composition supplies what the port does not carry: the synchronous live
//! RSSI read, and the mapping of a role change's coexistence priority
//! ([`frames::role_txrx_priority`]) to its radio's coexistence levels.
//!
//! It builds on `openthread-radio`, the C-free trait crate of the
//! repository's fork of that crate (<https://github.com/ermacv/openthread>,
//! branch `oer/main`), whose trait also carries OpenThread's MAC
//! keys, frame counter and per-frame transmit information. The radio reports
//! the capabilities ESP-IDF's OpenThread port reports: hardware acknowledgement with its timeout, address filtering,
//! promiscuous mode, source matching for frame pending, energy scan,
//! transmission from sleep, transmit security and timed transmission and
//! reception for Coordinated Sampled Listening. As in the port,
//! OpenThread's `SubMac` runs CSMA-CA backoffs and retries itself; each of
//! its attempts reaches the radio as one transmission with or without a
//! CCA, which the radio secures with OpenThread's keys and a frame counter
//! of its own. 2015 frames are answered with enhanced ACKs, secured ones
//! with the same keys and counter.
//!
//! [`openthread`]: https://docs.rs/openthread

#[cfg(test)]
extern crate std;

pub mod frames;

mod radio;

pub use radio::{
    MonotonicRadioClock, OPEN_THREAD_RADIO_CAPABILITIES, OpenThreadRadio, PendingSlots, PortRssi,
};
