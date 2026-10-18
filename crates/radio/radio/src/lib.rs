#![no_std]
#![forbid(unsafe_code)]

//! Portable radio service policy and application-side ownership states.
//!
//! This crate defines requests, validates role combinations, and moves an
//! application between idle and active Wi-Fi typestates. It does not own PAC
//! registers, DMA descriptors, interrupt routes, clocks, an executor, or a
//! network stack. A chip composition implements the service port and retains
//! those resources in its actor; applications normally enter through that
//! composition rather than constructing this crate's internal machinery.
//!
//! With `wifi`, start at `wifi::WifiIdle`. A successful role start consumes
//! the idle capability, and only a successful stop returns a reusable one.
//! Rejected preparation returns the unchanged request and idle capability;
//! a fault after materialization does not. The executor-independent runtime
//! `oer-radio-supervisor` supplies the one-command mailbox and the role-epoch
//! actor that the concrete ESP32-S31 composition binds to an executor. This
//! crate depends on no executor or adapter.
//!
//! # Command and data planes
//!
//! The service port transports owned control requests and value reports. It
//! never transports hardware owners. Frames follow separate adapter and
//! stable-memory paths, so command completion alone says nothing about DMA
//! completion or packet-buffer reuse.
//!
//! # Cancellation
//!
//! Public role typestates are affine, so callers cannot interpret a dropped
//! start/stop wait in an executor binding as recovery of the consumed
//! capability.
//!
//! The ESP32-S31 application examples under `examples/` are the buildable
//! consumers. The concrete lifecycle, including hardware quarantine, is
//! documented by `oer-esp32s31-ieee80211-system`.

#[cfg(test)]
extern crate std;

#[cfg(feature = "wifi")]
pub mod wifi;
